//! Tokio client over the semantic layer.

use std::borrow::Cow;
use std::collections::HashMap;
use std::future::Future;
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use std::task::Poll;
use std::time::Duration;

use tokio::net::{ToSocketAddrs, lookup_host};

use crate::error::{ClientError, MemcacheError, ServerError};

use super::async_connection::AsyncMetaConnection;
use super::client::{MetaClientBuilder, Timeouts, jump_hash};
use super::core::{self, Operation};
use super::meta_api::{ArithmeticMode, build_debug, build_noop, parse_debug_result, parse_meta_result};
use super::meta_command::{MetaCommand, ReturnCode};
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

/// Await every future concurrently and collect their outputs in order. A
/// tiny fixed-purpose join (every pending future is re-polled on each wake)
/// so the crate needs neither a futures dependency nor a spawned task; fine
/// for the handful of per-server exchanges a batch produces.
async fn join_all<F: Future>(futures: Vec<F>) -> Vec<F::Output> {
    let mut futures: Vec<_> = futures.into_iter().map(|future| Some(Box::pin(future))).collect();
    let mut outputs: Vec<Option<F::Output>> = futures.iter().map(|_| None).collect();
    std::future::poll_fn(|context| {
        let mut ready = true;
        for (slot, output) in futures.iter_mut().zip(outputs.iter_mut()) {
            if let Some(future) = slot {
                match future.as_mut().poll(context) {
                    Poll::Ready(value) => {
                        *output = Some(value);
                        *slot = None;
                    }
                    Poll::Pending => ready = false,
                }
            }
        }
        if ready { Poll::Ready(()) } else { Poll::Pending }
    })
    .await;
    outputs.into_iter().map(|output| output.unwrap()).collect()
}

/// One server: its resolved addresses and a stack of idle connections.
/// The mutex is only held to pop/push, never across I/O.
struct AsyncServer {
    addrs: Vec<SocketAddr>,
    idle: Mutex<Vec<AsyncMetaConnection>>,
}

impl AsyncServer {
    async fn checkout(&self, timeouts: &Timeouts) -> Result<AsyncMetaConnection, MemcacheError> {
        // Idle connections may have been closed by the server or a
        // middlebox while pooled; probe and discard instead of handing a
        // dead connection to the caller.
        loop {
            let Some(connection) = self.idle.lock().unwrap().pop() else {
                break;
            };
            if connection.is_reusable() {
                return Ok(connection);
            }
        }
        timed(timeouts.connect, AsyncMetaConnection::connect(self.addrs.as_slice())).await
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
    hash_function: fn(&[u8]) -> u64,
    max_idle: usize,
    timeouts: Timeouts,
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
            servers.push(AsyncServer {
                addrs: resolved,
                idle: Mutex::new(Vec::new()),
            });
        }
        if servers.is_empty() {
            return Err(ClientError::Error(Cow::Borrowed("at least one server address is required")).into());
        }
        Ok(AsyncMetaClient {
            servers: Arc::new(servers),
            hash_function: self.hash_function,
            max_idle: self.max_idle,
            timeouts: self.timeouts,
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
    /// distributed across them by jump consistent hash, so the list order
    /// is part of the routing contract: append or drop servers at the
    /// tail to move the minimal share of keys. Addresses are resolved
    /// here, but connections are dialed lazily, so a down server surfaces
    /// at the first operation; [`noop`](Self::noop) verifies connectivity
    /// eagerly.
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
        jump_hash((self.hash_function)(key), self.servers.len())
    }

    /// Read a key.
    pub fn get(&self, key: impl AsRef<[u8]>) -> Request<'_, AsyncMetaClient, Get> {
        Request::new(self, Get::new(key))
    }

    /// Store a value under a key. The value is encoded via
    /// [`ToValue`](super::ToValue), which also picks the stored client
    /// flags: [`FLAG_STR`](super::FLAG_STR) for strings,
    /// [`FLAG_INT`](super::FLAG_INT) for integers and
    /// [`FLAG_BYTES`](super::FLAG_BYTES) (zero) otherwise. Other clients
    /// may not share these conventions; override with
    /// [`client_flags`](Request::client_flags).
    pub fn set(&self, key: impl AsRef<[u8]>, value: impl ToValue) -> Request<'_, AsyncMetaClient, Set> {
        Request::new(self, Set::new(key, value))
    }

    /// Delete a key.
    pub fn delete(&self, key: impl AsRef<[u8]>) -> Request<'_, AsyncMetaClient, Delete> {
        Request::new(self, Delete::new(key))
    }

    /// Increment a counter (delta defaults to 1).
    pub fn increment(&self, key: impl AsRef<[u8]>) -> Request<'_, AsyncMetaClient, Arithmetic> {
        Request::new(self, Arithmetic::new(key))
    }

    /// Decrement a counter (delta defaults to 1); saturates at zero.
    pub fn decrement(&self, key: impl AsRef<[u8]>) -> Request<'_, AsyncMetaClient, Arithmetic> {
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
        let mut connection = server.checkout(&self.timeouts).await?;
        // A failed exchange leaves the stream in an unknown state, so the
        // connection is dropped instead of returned to the pool.
        let response = timed(self.timeouts.io, connection.execute(&command)).await?;
        server.put_back(connection, self.max_idle);
        operation.parse(parse_meta_result(response)?)
    }

    /// Run several operations, split per server and pipelined; the
    /// per-server groups are exchanged concurrently, so a multi-server
    /// batch costs about one round trip in total.
    ///
    /// All operations are validated before anything is written; a validation
    /// failure is the outer error and guarantees nothing executed. After
    /// that, every operation gets its own entry in input order: a transport
    /// failure fails the operations of that server's group (their entries
    /// are `Err`, and whether they took effect on the server is unknown)
    /// while the remaining groups still execute. Semantic outcomes (miss,
    /// CAS mismatch, ...) are not errors; they show up inside [`OpResult`].
    /// A batch is not a transaction.
    pub async fn run_batch(
        &self,
        operations: impl IntoIterator<Item = Op>,
    ) -> Result<Vec<Result<OpResult, MemcacheError>>, MemcacheError> {
        let operations: Vec<Op> = operations.into_iter().collect();
        self.run_all(&operations).await
    }

    /// Run several operations of one kind with typed results - a batch
    /// without the [`Op`]/[`OpResult`] wrapping. The multiget:
    /// `client.run_many(keys.iter().map(Get::new))`. Execution and failure
    /// semantics are those of [`run_batch`](Self::run_batch).
    pub async fn run_many<O: Operation>(
        &self,
        operations: impl IntoIterator<Item = O>,
    ) -> Result<Vec<Result<O::Output, MemcacheError>>, MemcacheError> {
        let operations: Vec<O> = operations.into_iter().collect();
        self.run_all(&operations).await
    }

    async fn run_all<O: Operation>(
        &self,
        operations: &[O],
    ) -> Result<Vec<Result<O::Output, MemcacheError>>, MemcacheError> {
        let mut plan = core::plan(operations, self.servers.len(), |key| self.connection_index(key))?;
        let mut outputs: Vec<Option<Result<O::Output, MemcacheError>>> = (0..operations.len()).map(|_| None).collect();
        // One exchange future per non-empty server group, run concurrently:
        // the batch takes one round trip total, not one per server.
        let exchanges = plan
            .groups
            .iter()
            .enumerate()
            .filter(|(_, indices)| !indices.is_empty())
            .map(|(server, indices)| {
                let commands: Vec<MetaCommand> = indices
                    .iter()
                    .map(|&index| plan.commands[index].take().unwrap())
                    .collect();
                let server = &self.servers[server];
                async move {
                    let mut connection = server.checkout(&self.timeouts).await?;
                    let responses = timed(self.timeouts.io, connection.execute_batch(&commands)).await?;
                    server.put_back(connection, self.max_idle);
                    Ok::<_, MemcacheError>(responses)
                }
            })
            .collect();
        let results = join_all(exchanges).await;
        let groups = plan.groups.iter().filter(|indices| !indices.is_empty());
        for (indices, result) in groups.zip(results) {
            match result {
                Ok(responses) => {
                    for (&index, response) in indices.iter().zip(responses) {
                        outputs[index] =
                            Some(parse_meta_result(response).and_then(|wire| operations[index].parse(wire)));
                    }
                }
                Err(error) => {
                    for &index in indices {
                        outputs[index] = Some(Err(core::duplicate_error(&error)));
                    }
                }
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
            let mut connection = server.checkout(&self.timeouts).await?;
            let response = timed(self.timeouts.io, connection.execute(&build_noop())).await?;
            server.put_back(connection, self.max_idle);
            if response.rc != ReturnCode::Mn {
                return Err(ServerError::BadResponse("unexpected no-op response".into()).into());
            }
        }
        Ok(())
    }

    /// Fetch `me` debug fields for a key; `None` on a miss.
    pub async fn debug(&self, key: impl AsRef<[u8]>) -> Result<Option<HashMap<String, String>>, MemcacheError> {
        let key = key.as_ref().to_vec();
        let server = &self.servers[self.connection_index(&key)];
        let command = build_debug(key)?;
        let mut connection = server.checkout(&self.timeouts).await?;
        let response = timed(self.timeouts.io, connection.execute(&command)).await?;
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

#[cfg(test)]
mod tests {
    use std::io::{BufRead, BufReader, Write};
    use std::net::TcpListener;

    use super::*;

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
        assert!(client.delete("foo").send().await.unwrap().applied());
        handle.join().unwrap();
    }

    #[tokio::test]
    async fn stale_pooled_connection_is_discarded() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let handle = std::thread::spawn(move || {
            // First connection: serve one operation, then close while the
            // connection sits in the pool.
            let (stream, _) = listener.accept().unwrap();
            let mut reader = BufReader::new(stream);
            let mut line = Vec::new();
            reader.read_until(b'\n', &mut line).unwrap();
            reader.get_mut().write_all(b"HD\r\n").unwrap();
            drop(reader);
            // The next operation must arrive on a fresh connection.
            let (stream, _) = listener.accept().unwrap();
            let mut reader = BufReader::new(stream);
            let mut line = Vec::new();
            reader.read_until(b'\n', &mut line).unwrap();
            reader.get_mut().write_all(b"HD\r\n").unwrap();
        });

        let client = AsyncMetaClient::connect(addr).await.unwrap();
        assert!(client.delete("foo").send().await.unwrap().applied());
        // Give the server's FIN time to arrive before the next checkout.
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert!(client.delete("foo").send().await.unwrap().applied());
        handle.join().unwrap();
    }

    #[tokio::test]
    async fn run_batch_exchanges_servers_concurrently() {
        use std::sync::mpsc;

        fn first_byte(key: &[u8]) -> u64 {
            key[0] as u64
        }
        let char_for = |bucket: usize| (b'0'..=b'z').find(|&byte| jump_hash(byte as u64, 2) == bucket).unwrap() as char;

        let (sender, receiver) = mpsc::channel();

        // Server 0 answers only after server 1 has seen its request. With
        // sequential group execution the batch would deadlock into the io
        // timeout; concurrent exchanges satisfy the gate.
        let listener0 = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr0 = listener0.local_addr().unwrap();
        let gate0 = std::thread::spawn(move || {
            let (stream, _) = listener0.accept().unwrap();
            let mut reader = BufReader::new(stream);
            let mut line = Vec::new();
            reader.read_until(b'\n', &mut line).unwrap();
            receiver.recv_timeout(Duration::from_secs(5)).unwrap();
            reader.get_mut().write_all(b"EN\r\n").unwrap();
        });

        let listener1 = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr1 = listener1.local_addr().unwrap();
        let gate1 = std::thread::spawn(move || {
            let (stream, _) = listener1.accept().unwrap();
            let mut reader = BufReader::new(stream);
            let mut line = Vec::new();
            reader.read_until(b'\n', &mut line).unwrap();
            sender.send(()).unwrap();
            reader.get_mut().write_all(b"EN\r\n").unwrap();
        });

        let client = AsyncMetaClient::builder()
            .hash_function(first_byte)
            .connect_multiple_async([addr0, addr1])
            .await
            .unwrap();
        let key0 = format!("{}a", char_for(0));
        let key1 = format!("{}b", char_for(1));
        let results = client
            .run_batch(vec![Get::new(&*key0).into(), Get::new(&*key1).into()])
            .await
            .unwrap();
        assert!(results.iter().all(|result| result.is_ok()));
        gate0.join().unwrap();
        gate1.join().unwrap();
    }
}
