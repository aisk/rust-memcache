//! Tokio client over the semantic layer.

use std::borrow::Cow;
use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};

use tokio::net::{ToSocketAddrs, lookup_host};

use crate::error::{ClientError, MemcacheError, ServerError};

use super::async_connection::AsyncMetaConnection;
use super::client::{DEFAULT_MAX_IDLE, default_hash_function, jump_hash};
use super::core::{self, Operation};
use super::meta_api::{ArithmeticMode, build_debug, build_noop, parse_debug_result, parse_meta_result};
use super::meta_command::{MetaCommand, ReturnCode};
use super::operation::{Arithmetic, Delete, Get, Op, Set};
use super::request::Request;
use super::result::OpResult;
use super::value::ToValue;

/// One server: its resolved addresses and a stack of idle connections.
/// The mutex is only held to pop/push, never across I/O.
struct AsyncServer {
    addrs: Vec<SocketAddr>,
    idle: Mutex<Vec<AsyncMetaConnection>>,
}

impl AsyncServer {
    async fn checkout(&self) -> Result<AsyncMetaConnection, MemcacheError> {
        let reused = self.idle.lock().unwrap().pop();
        if let Some(connection) = reused {
            return Ok(connection);
        }
        AsyncMetaConnection::connect(self.addrs.as_slice()).await
    }

    fn put_back(&self, connection: AsyncMetaConnection, max_idle: usize) {
        let mut idle = self.idle.lock().unwrap();
        if idle.len() < max_idle {
            idle.push(connection);
        }
    }
}

/// The async counterpart of [`MetaClient`](super::MetaClient); the same
/// request-builder surface and pooling behavior over tokio connections.
/// Cheap to clone and shareable across tasks.
///
/// ```no_run
/// # use memcache::exp::AsyncMetaClient;
/// # async fn example() -> Result<(), memcache::MemcacheError> {
/// let client = AsyncMetaClient::connect("127.0.0.1:11211").await?;
/// client.set("foo", "bar").ttl(60).send().await?;
/// let result = client.get("foo").send().await?;
/// # Ok(())
/// # }
/// ```
#[derive(Clone)]
pub struct AsyncMetaClient {
    servers: Arc<Vec<AsyncServer>>,
    hash_function: fn(&[u8]) -> u64,
    max_idle: usize,
}

impl AsyncMetaClient {
    pub async fn connect<A: ToSocketAddrs>(addr: A) -> Result<AsyncMetaClient, MemcacheError> {
        AsyncMetaClient::connect_multiple([addr]).await
    }

    /// Connect to several servers; keys are distributed across them by the
    /// hash function. One connection per server is dialed up front so
    /// connection errors surface here.
    pub async fn connect_multiple<A: ToSocketAddrs>(
        addrs: impl IntoIterator<Item = A>,
    ) -> Result<AsyncMetaClient, MemcacheError> {
        let mut servers = Vec::new();
        for addr in addrs {
            let resolved: Vec<SocketAddr> = lookup_host(addr).await?.collect();
            if resolved.is_empty() {
                return Err(ClientError::Error(Cow::Borrowed("address resolved to no socket addresses")).into());
            }
            let server = AsyncServer {
                addrs: resolved,
                idle: Mutex::new(Vec::new()),
            };
            let connection = AsyncMetaConnection::connect(server.addrs.as_slice()).await?;
            server.idle.lock().unwrap().push(connection);
            servers.push(server);
        }
        if servers.is_empty() {
            return Err(ClientError::Error(Cow::Borrowed("at least one server address is required")).into());
        }
        Ok(AsyncMetaClient {
            servers: Arc::new(servers),
            hash_function: default_hash_function,
            max_idle: DEFAULT_MAX_IDLE,
        })
    }

    /// Replace the function that hashes keys; the server is then picked by
    /// jump consistent hash over that value. The default hashes with
    /// `DefaultHasher`. Configure before cloning: clones share connections
    /// but not this setting.
    pub fn with_hash_function(mut self, hash_function: fn(&[u8]) -> u64) -> AsyncMetaClient {
        self.hash_function = hash_function;
        self
    }

    /// Cap how many idle connections each server retains (default 8).
    /// Concurrency above the cap dials extra connections, which are dropped
    /// when returned. Configure before cloning: clones share connections
    /// but not this setting.
    pub fn with_max_idle(mut self, max_idle: usize) -> AsyncMetaClient {
        self.max_idle = max_idle;
        self
    }

    fn connection_index(&self, key: &[u8]) -> usize {
        jump_hash((self.hash_function)(key), self.servers.len())
    }

    /// Read a key.
    pub fn get(&self, key: impl Into<Vec<u8>>) -> Request<'_, AsyncMetaClient, Get> {
        Request::new(self, Get::new(key))
    }

    /// Store a value under a key; the value is encoded via
    /// [`ToValue`](super::ToValue).
    pub fn set(&self, key: impl Into<Vec<u8>>, value: impl ToValue) -> Request<'_, AsyncMetaClient, Set> {
        Request::new(self, Set::new(key, value))
    }

    /// Delete a key.
    pub fn delete(&self, key: impl Into<Vec<u8>>) -> Request<'_, AsyncMetaClient, Delete> {
        Request::new(self, Delete::new(key))
    }

    /// Increment a counter (delta defaults to 1).
    pub fn increment(&self, key: impl Into<Vec<u8>>) -> Request<'_, AsyncMetaClient, Arithmetic> {
        Request::new(self, Arithmetic::new(key))
    }

    /// Decrement a counter (delta defaults to 1); saturates at zero.
    pub fn decrement(&self, key: impl Into<Vec<u8>>) -> Request<'_, AsyncMetaClient, Arithmetic> {
        let operation = Arithmetic {
            mode: ArithmeticMode::Decrement,
            ..Arithmetic::new(key)
        };
        Request::new(self, operation)
    }

    /// Run a standalone operation value; [`send`](Request::send) is sugar
    /// for this.
    pub async fn run<O: Operation>(&self, operation: O) -> Result<O::Output, MemcacheError> {
        let command = operation.prepare()?;
        let server = &self.servers[self.connection_index(operation.key())];
        let mut connection = server.checkout().await?;
        // A failed exchange leaves the stream in an unknown state, so the
        // connection is dropped instead of returned to the pool.
        let response = connection.execute(&command).await?;
        server.put_back(connection, self.max_idle);
        operation.parse(parse_meta_result(response)?)
    }

    /// Run several operations, split per server and pipelined with one
    /// round trip per server.
    ///
    /// All commands are validated before anything is written and executed
    /// independently in order per server; one operation's semantic outcome
    /// (miss, CAS mismatch, ...) shows up in its own result and does not
    /// stop the rest. This is not a transaction.
    pub async fn run_batch(&self, operations: impl IntoIterator<Item = Op>) -> Result<Vec<OpResult>, MemcacheError> {
        let operations: Vec<Op> = operations.into_iter().collect();
        self.run_all(&operations).await
    }

    async fn run_all<O: Operation>(&self, operations: &[O]) -> Result<Vec<O::Output>, MemcacheError> {
        let mut plan = core::plan(operations, self.servers.len(), |key| self.connection_index(key))?;
        let mut outputs: Vec<Option<O::Output>> = (0..operations.len()).map(|_| None).collect();
        for (server, indices) in plan.groups.iter().enumerate() {
            if indices.is_empty() {
                continue;
            }
            let commands: Vec<MetaCommand> = indices
                .iter()
                .map(|&index| plan.commands[index].take().unwrap())
                .collect();
            let server = &self.servers[server];
            let mut connection = server.checkout().await?;
            let responses = connection.execute_batch(&commands).await?;
            server.put_back(connection, self.max_idle);
            for (&index, response) in indices.iter().zip(responses) {
                outputs[index] = Some(operations[index].parse(parse_meta_result(response)?)?);
            }
        }
        Ok(outputs
            .into_iter()
            .map(|output| output.expect("batch executor left an operation unresolved"))
            .collect())
    }

    /// Round-trip an `mn` no-op on every server; useful as a connection
    /// health check.
    pub async fn noop(&self) -> Result<(), MemcacheError> {
        for server in self.servers.iter() {
            let mut connection = server.checkout().await?;
            let response = connection.execute(&build_noop()).await?;
            server.put_back(connection, self.max_idle);
            if response.rc != ReturnCode::Mn {
                return Err(ServerError::BadResponse("unexpected no-op response".into()).into());
            }
        }
        Ok(())
    }

    /// Fetch `me` debug fields for a key; `None` on a miss.
    pub async fn debug(&self, key: impl Into<Vec<u8>>) -> Result<Option<HashMap<String, String>>, MemcacheError> {
        let key = key.into();
        let server = &self.servers[self.connection_index(&key)];
        let command = build_debug(key)?;
        let mut connection = server.checkout().await?;
        let response = connection.execute(&command).await?;
        server.put_back(connection, self.max_idle);
        parse_debug_result(&response)
    }
}

impl<'a, O: Operation> Request<'a, AsyncMetaClient, O> {
    /// Execute the request and return its typed result.
    pub async fn send(self) -> Result<O::Output, MemcacheError> {
        let Request { client, operation } = self;
        client.run(operation).await
    }
}
