//! Tokio client over the semantic layer.

use std::borrow::Cow;
use std::collections::HashMap;

use tokio::net::ToSocketAddrs;

use crate::error::{ClientError, MemcacheError, ServerError};

use super::async_connection::AsyncMetaConnection;
use super::client::{default_hash_function, jump_hash};
use super::core::Operation;
use super::meta_api::{ArithmeticMode, build_debug, build_noop, parse_debug_result, parse_meta_result};
use super::meta_command::{MetaCommand, ReturnCode};
use super::operation::{Arithmetic, Delete, Get, Op, Set};
use super::request::Request;
use super::result::OpResult;
use super::value::ToValue;

/// The async counterpart of [`MetaClient`](super::MetaClient); the same
/// request-builder surface over tokio connections.
///
/// ```no_run
/// # use memcache::exp::AsyncMetaClient;
/// # async fn example() -> Result<(), memcache::MemcacheError> {
/// let mut client = AsyncMetaClient::connect("127.0.0.1:11211").await?;
/// client.set("foo", "bar").ttl(60).send().await?;
/// let result = client.get("foo").send().await?;
/// # Ok(())
/// # }
/// ```
pub struct AsyncMetaClient {
    connections: Vec<AsyncMetaConnection>,
    hash_function: fn(&[u8]) -> u64,
}

impl AsyncMetaClient {
    pub async fn connect<A: ToSocketAddrs>(addr: A) -> Result<AsyncMetaClient, MemcacheError> {
        Ok(AsyncMetaClient::from_connection(
            AsyncMetaConnection::connect(addr).await?,
        ))
    }

    /// Connect to several servers; keys are distributed across them by the
    /// hash function.
    pub async fn connect_multiple<A: ToSocketAddrs>(
        addrs: impl IntoIterator<Item = A>,
    ) -> Result<AsyncMetaClient, MemcacheError> {
        let mut connections = Vec::new();
        for addr in addrs {
            connections.push(AsyncMetaConnection::connect(addr).await?);
        }
        if connections.is_empty() {
            return Err(ClientError::Error(Cow::Borrowed("at least one server address is required")).into());
        }
        Ok(AsyncMetaClient {
            connections,
            hash_function: default_hash_function,
        })
    }

    pub fn from_connection(connection: AsyncMetaConnection) -> AsyncMetaClient {
        AsyncMetaClient {
            connections: vec![connection],
            hash_function: default_hash_function,
        }
    }

    /// Replace the function that hashes keys; the server is then picked by
    /// jump consistent hash over that value. The default hashes with
    /// `DefaultHasher`.
    pub fn with_hash_function(mut self, hash_function: fn(&[u8]) -> u64) -> AsyncMetaClient {
        self.hash_function = hash_function;
        self
    }

    fn connection_index(&self, key: &[u8]) -> usize {
        jump_hash((self.hash_function)(key), self.connections.len())
    }

    /// Read a key.
    pub fn get(&mut self, key: impl Into<Vec<u8>>) -> Request<'_, AsyncMetaClient, Get> {
        Request::new(self, Get::new(key))
    }

    /// Store a value under a key; the value is encoded via
    /// [`ToValue`](super::ToValue).
    pub fn set(&mut self, key: impl Into<Vec<u8>>, value: impl ToValue) -> Request<'_, AsyncMetaClient, Set> {
        Request::new(self, Set::new(key, value))
    }

    /// Delete a key.
    pub fn delete(&mut self, key: impl Into<Vec<u8>>) -> Request<'_, AsyncMetaClient, Delete> {
        Request::new(self, Delete::new(key))
    }

    /// Increment a counter (delta defaults to 1).
    pub fn increment(&mut self, key: impl Into<Vec<u8>>) -> Request<'_, AsyncMetaClient, Arithmetic> {
        Request::new(self, Arithmetic::new(key))
    }

    /// Decrement a counter (delta defaults to 1); saturates at zero.
    pub fn decrement(&mut self, key: impl Into<Vec<u8>>) -> Request<'_, AsyncMetaClient, Arithmetic> {
        let operation = Arithmetic {
            mode: ArithmeticMode::Decrement,
            ..Arithmetic::new(key)
        };
        Request::new(self, operation)
    }

    /// Run a standalone operation value; [`send`](Request::send) is sugar
    /// for this.
    pub async fn run<O: Operation>(&mut self, operation: O) -> Result<O::Output, MemcacheError> {
        let command = operation.prepare()?;
        let index = self.connection_index(operation.key());
        let wire = parse_meta_result(self.connections[index].execute(&command).await?)?;
        operation.parse(wire)
    }

    /// Run several operations, split per server and pipelined with one
    /// round trip per server.
    ///
    /// All commands are validated before anything is written and executed
    /// independently in order per server; one operation's semantic outcome
    /// (miss, CAS mismatch, ...) shows up in its own result and does not
    /// stop the rest. This is not a transaction.
    pub async fn run_batch(
        &mut self,
        operations: impl IntoIterator<Item = Op>,
    ) -> Result<Vec<OpResult>, MemcacheError> {
        let operations: Vec<Op> = operations.into_iter().collect();
        self.run_all(&operations).await
    }

    async fn run_all<O: Operation>(&mut self, operations: &[O]) -> Result<Vec<O::Output>, MemcacheError> {
        // Validate every operation before writing the first byte, so a bad
        // option never leaves half a batch on the wire.
        let mut prepared: Vec<Option<MetaCommand>> = Vec::with_capacity(operations.len());
        for operation in operations {
            prepared.push(Some(operation.prepare()?));
        }
        let mut groups: Vec<Vec<usize>> = vec![Vec::new(); self.connections.len()];
        for (index, operation) in operations.iter().enumerate() {
            groups[self.connection_index(operation.key())].push(index);
        }
        let mut outputs: Vec<Option<O::Output>> = (0..operations.len()).map(|_| None).collect();
        for (server, indices) in groups.iter().enumerate() {
            if indices.is_empty() {
                continue;
            }
            let commands: Vec<MetaCommand> = indices.iter().map(|&index| prepared[index].take().unwrap()).collect();
            let responses = self.connections[server].execute_batch(&commands).await?;
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
    pub async fn noop(&mut self) -> Result<(), MemcacheError> {
        for connection in &mut self.connections {
            let response = connection.execute(&build_noop()).await?;
            if response.rc != ReturnCode::Mn {
                return Err(ServerError::BadResponse("unexpected no-op response".into()).into());
            }
        }
        Ok(())
    }

    /// Fetch `me` debug fields for a key; `None` on a miss.
    pub async fn debug(&mut self, key: impl Into<Vec<u8>>) -> Result<Option<HashMap<String, String>>, MemcacheError> {
        let key = key.into();
        let index = self.connection_index(&key);
        let response = self.connections[index].execute(&build_debug(key)?).await?;
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
