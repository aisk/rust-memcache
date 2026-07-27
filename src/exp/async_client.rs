//! Tokio client over the semantic layer.

use std::borrow::Cow;
use std::collections::HashMap;
use std::future::Future;
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use tokio::net::{ToSocketAddrs, lookup_host};
use tokio::sync::{OwnedSemaphorePermit, Semaphore};

use crate::error::{ClientError, MemcacheError, ServerError};

use super::async_connection::AsyncMetaConnection;
use super::client::{Config, MetaClientBuilder, is_disconnect, jump_hash, pool_exhausted};
use super::core::{self, Operation};
use super::meta_api::{ArithmeticMode, build_debug, build_noop, parse_debug_result, parse_meta_result};
use super::meta_command::{MetaCommand, MetaResponse, ReturnCode};
use super::operation::{Arithmetic, Delete, Get, Op, Set};
use super::request::Request;
use super::result::OpResult;
use super::value::ToValue;

/// Bound a transport future by a timeout; `None` means unbounded. A
/// timeout surfaces as an io error and so poisons the connection like any
/// other transport failure.
async fn timed<T>(
    timeout: Option<Duration>,
    future: impl Future<Output = Result<T, MemcacheError>>,
) -> Result<T, MemcacheError> {
    match timeout {
        Some(duration) => match tokio::time::timeout(duration, future).await {
            Ok(result) => result,
            Err(_) => Err(std::io::Error::from(std::io::ErrorKind::TimedOut).into()),
        },
        None => future.await,
    }
}

/// One server: its resolved addresses and a stack of idle connections.
/// The mutex is only held to pop/push, never across I/O. The semaphore
/// caps checked-out connections: every checkout holds a permit for as
/// long as the connection is in flight, so dropping or returning the
/// connection frees the slot either way.
struct AsyncServer {
    addrs: Vec<SocketAddr>,
    idle: Mutex<Vec<(AsyncMetaConnection, Instant)>>,
    slots: Arc<Semaphore>,
}

impl AsyncServer {
    fn new(addrs: Vec<SocketAddr>, config: &Config) -> AsyncServer {
        let cap = config.max_connections.unwrap_or(Semaphore::MAX_PERMITS);
        AsyncServer {
            addrs,
            idle: Mutex::new(Vec::new()),
            slots: Arc::new(Semaphore::new(cap.min(Semaphore::MAX_PERMITS))),
        }
    }

    /// Check out a connection with its in-flight permit; the flag is true
    /// when it came from the pool rather than a fresh dial.
    async fn checkout(
        &self,
        config: &Config,
    ) -> Result<(AsyncMetaConnection, OwnedSemaphorePermit, bool), MemcacheError> {
        let permit = match self.slots.clone().try_acquire_owned() {
            Ok(permit) => permit,
            // At the connection cap: wait for a put_back or drop.
            Err(_) => {
                let acquire = async { Ok(self.slots.clone().acquire_owned().await.expect("semaphore closed")) };
                match timed(config.timeouts.connect, acquire).await {
                    Ok(permit) => permit,
                    Err(_) => return Err(pool_exhausted()),
                }
            }
        };
        loop {
            let popped = self.idle.lock().unwrap().pop();
            let Some((connection, since)) = popped else { break };
            if config.idle_timeout.is_some_and(|limit| since.elapsed() >= limit) {
                // Too old to trust: an idle connection may have been torn
                // down by a middlebox or server restart.
                continue;
            }
            return Ok((connection, permit, true));
        }
        let connection = timed(
            config.timeouts.connect,
            AsyncMetaConnection::connect(self.addrs.as_slice()),
        )
        .await?;
        Ok((connection, permit, false))
    }

    fn put_back(&self, connection: AsyncMetaConnection, permit: OwnedSemaphorePermit, config: &Config) {
        let mut idle = self.idle.lock().unwrap();
        if idle.len() < config.max_idle {
            idle.push((connection, Instant::now()));
        }
        drop(idle);
        // Explicitly: returning the connection frees its in-flight slot.
        drop(permit);
    }
}

/// The async counterpart of [`MetaClient`](super::MetaClient); the same
/// request-builder surface and pooling behavior over tokio connections.
/// Cheap to clone and shareable across tasks; clones share the connection
/// pools and configuration, which is set on
/// [`MetaClientBuilder`](super::MetaClientBuilder) before connecting and
/// stays fixed for the client's lifetime.
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
    config: Config,
}

impl MetaClientBuilder {
    /// Connect to one server with this configuration; the async
    /// counterpart of [`connect`](Self::connect).
    pub async fn connect_async<A: ToSocketAddrs>(self, addr: A) -> Result<AsyncMetaClient, MemcacheError> {
        self.connect_multiple_async([addr]).await
    }

    /// Connect to several servers with this configuration; the async
    /// counterpart of [`connect_multiple`](Self::connect_multiple).
    pub async fn connect_multiple_async<A: ToSocketAddrs>(
        self,
        addrs: impl IntoIterator<Item = A>,
    ) -> Result<AsyncMetaClient, MemcacheError> {
        let mut servers = Vec::new();
        for addr in addrs {
            let resolved: Vec<SocketAddr> = lookup_host(addr).await?.collect();
            if resolved.is_empty() {
                return Err(ClientError::Error(Cow::Borrowed("address resolved to no socket addresses")).into());
            }
            servers.push(AsyncServer::new(resolved, &self.config));
        }
        if servers.is_empty() {
            return Err(ClientError::Error(Cow::Borrowed("at least one server address is required")).into());
        }
        Ok(AsyncMetaClient {
            servers: Arc::new(servers),
            config: self.config,
        })
    }
}

impl AsyncMetaClient {
    /// Connect to one server with the default configuration; use
    /// [`builder`](Self::builder) to change it.
    pub async fn connect<A: ToSocketAddrs>(addr: A) -> Result<AsyncMetaClient, MemcacheError> {
        AsyncMetaClient::connect_multiple([addr]).await
    }

    /// Connect to several servers with the default configuration; keys are
    /// distributed across them by the hash function. Addresses are resolved
    /// here, but connections are dialed lazily, so a down server surfaces
    /// at the first operation.
    pub async fn connect_multiple<A: ToSocketAddrs>(
        addrs: impl IntoIterator<Item = A>,
    ) -> Result<AsyncMetaClient, MemcacheError> {
        AsyncMetaClient::builder().connect_multiple_async(addrs).await
    }

    /// Start a [`MetaClientBuilder`](super::MetaClientBuilder) to configure
    /// hashing, pooling and timeouts before connecting.
    pub fn builder() -> MetaClientBuilder {
        MetaClientBuilder::new()
    }

    fn connection_index(&self, key: &[u8]) -> usize {
        jump_hash((self.config.hash_function)(key), self.servers.len())
    }

    /// Check out a connection, write the payload, read `responses` framed
    /// responses (all under one whole-exchange deadline) and return the
    /// connection to the pool.
    ///
    /// A pooled connection that fails the write with a disconnect error
    /// was torn down while idle; the server cannot have executed anything,
    /// so the exchange retries once on a freshly dialed connection. Any
    /// other failure leaves the stream in an unknown state and drops the
    /// connection (and its slot permit) instead of returning it.
    async fn exchange(
        &self,
        server: usize,
        payload: &[u8],
        responses: usize,
    ) -> Result<Vec<MetaResponse>, MemcacheError> {
        let server = &self.servers[server];
        let (mut connection, permit, reused) = server.checkout(&self.config).await?;
        let result = timed(self.config.timeouts.io, async {
            if let Err(error) = connection.write_payload(payload).await {
                if !(reused && is_disconnect(&error)) {
                    return Err(error);
                }
                // The dead connection's permit carries over to the redial.
                connection = timed(
                    self.config.timeouts.connect,
                    AsyncMetaConnection::connect(server.addrs.as_slice()),
                )
                .await?;
                connection.write_payload(payload).await?;
            }
            let mut collected = Vec::with_capacity(responses);
            for _ in 0..responses {
                collected.push(connection.receive().await?);
            }
            Ok(collected)
        })
        .await;
        match result {
            Ok(collected) => {
                server.put_back(connection, permit, &self.config);
                Ok(collected)
            }
            // The connection and its permit drop here.
            Err(error) => Err(error),
        }
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
        let payload = command.encode()?;
        let index = self.connection_index(operation.key());
        let response = self
            .exchange(index, &payload, 1)
            .await?
            .pop()
            .expect("exchange returned no response");
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
            let mut payload = Vec::new();
            for command in &commands {
                command.encode_into(&mut payload)?;
            }
            let responses = self.exchange(server, &payload, commands.len()).await?;
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
        let payload = build_noop().encode()?;
        for server in 0..self.servers.len() {
            let response = self
                .exchange(server, &payload, 1)
                .await?
                .pop()
                .expect("exchange returned no response");
            if response.rc != ReturnCode::Mn {
                return Err(ServerError::BadResponse("unexpected no-op response".into()).into());
            }
        }
        Ok(())
    }

    /// Fetch `me` debug fields for a key; `None` on a miss.
    pub async fn debug(&self, key: impl Into<Vec<u8>>) -> Result<Option<HashMap<String, String>>, MemcacheError> {
        let key = key.into();
        let index = self.connection_index(&key);
        let payload = build_debug(key)?.encode()?;
        let response = self
            .exchange(index, &payload, 1)
            .await?
            .pop()
            .expect("exchange returned no response");
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

#[cfg(test)]
mod tests {
    use std::io::{BufRead, BufReader, Write};
    use std::net::TcpListener;

    use super::*;

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn max_connections_bounds_concurrent_dials() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let accepted = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let counter = accepted.clone();
        std::thread::spawn(move || {
            while let Ok((stream, _)) = listener.accept() {
                counter.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                std::thread::spawn(move || {
                    let mut reader = BufReader::new(stream);
                    let mut line = Vec::new();
                    while reader.read_until(b'\n', &mut line).map(|n| n > 0).unwrap_or(false) {
                        if reader.get_mut().write_all(b"HD\r\n").is_err() {
                            break;
                        }
                        line.clear();
                    }
                });
            }
        });

        let client = AsyncMetaClient::builder()
            .max_connections(Some(2))
            .connect_async(addr)
            .await
            .unwrap();
        let tasks: Vec<_> = (0..4)
            .map(|_| {
                let client = client.clone();
                tokio::spawn(async move {
                    for _ in 0..5 {
                        assert!(client.delete("foo").send().await.unwrap().stored());
                    }
                })
            })
            .collect();
        for task in tasks {
            task.await.unwrap();
        }
        assert!(accepted.load(std::sync::atomic::Ordering::SeqCst) <= 2);
    }

    #[tokio::test]
    async fn io_timeout_poisons_connection() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let handle = std::thread::spawn(move || {
            // First connection: read the request but never respond, so the
            // exchange times out and the connection is poisoned.
            let (stream, _) = listener.accept().unwrap();
            let mut reader = BufReader::new(stream);
            let mut line = Vec::new();
            reader.read_until(b'\n', &mut line).unwrap();
            // Second connection: respond normally.
            let (stream, _) = listener.accept().unwrap();
            let mut reader = BufReader::new(stream);
            let mut line = Vec::new();
            reader.read_until(b'\n', &mut line).unwrap();
            reader.get_mut().write_all(b"HD\r\n").unwrap();
        });

        let client = AsyncMetaClient::builder()
            .io_timeout(Some(Duration::from_millis(100)))
            .connect_async(addr)
            .await
            .unwrap();
        assert!(client.delete("foo").send().await.is_err());
        assert!(client.delete("foo").send().await.unwrap().stored());
        handle.join().unwrap();
    }
}
