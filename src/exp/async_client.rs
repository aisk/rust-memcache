//! Tokio client over the semantic layer.

use std::collections::HashMap;
use std::future::Future;
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use std::task::Poll;
use std::time::{Duration, Instant};

use tokio::net::{ToSocketAddrs, lookup_host};
use tokio::sync::{OwnedSemaphorePermit, Semaphore};

use super::async_connection::AsyncMetaConnection;
use super::client::{Engine, MetaClientBuilder, route};
use super::core::{self, Batch, Failure, Operation};
use super::error::{Error, Result};
use super::meta_api::{
    ArithmeticMode, MetaCommandResult, build_debug, build_noop, parse_debug_result, parse_meta_result,
};
use super::meta_command::{MetaCommand, MetaResponse, ReturnCode};
use super::operation::{Arithmetic, Delete, Get, Op, Set};
use super::request::Request;
use super::result::OpResult;
use super::router::Router;

/// Bound a transport future by a timeout; `None` means unbounded. A
/// timeout surfaces as an io error and so poisons the connection like any
/// other transport failure.
async fn timed<T>(timeout: Option<Duration>, future: impl Future<Output = Result<T>>) -> Result<T> {
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

/// One server: its resolved addresses, a stack of idle connections and
/// the connection slots. The mutex is only held to pop/push, never across
/// I/O.
struct AsyncServer {
    addrs: Vec<SocketAddr>,
    idle: Mutex<Vec<(AsyncMetaConnection, Instant)>>,
    slots: Option<Arc<Semaphore>>,
}

/// A connection checked out of an [`AsyncServer`]. Dropping it (including
/// when the future holding it is cancelled mid-exchange) discards the
/// connection and frees its slot; [`put_back`](Self::put_back) returns it
/// to the idle stack instead.
struct AsyncCheckout<'a> {
    server: &'a AsyncServer,
    connection: Option<AsyncMetaConnection>,
    pooled: bool,
    _slot: Option<OwnedSemaphorePermit>,
}

impl AsyncCheckout<'_> {
    fn connection(&mut self) -> &mut AsyncMetaConnection {
        self.connection.as_mut().expect("checkout holds a connection")
    }

    fn put_back(mut self, engine: &Engine) {
        if let Some(connection) = self.connection.take() {
            let mut idle = self.server.idle.lock().unwrap();
            if idle.len() < engine.max_idle {
                idle.push((connection, Instant::now()));
            }
        }
    }
}

impl AsyncServer {
    fn new(addrs: Vec<SocketAddr>, engine: &Engine) -> AsyncServer {
        AsyncServer {
            addrs,
            idle: Mutex::new(Vec::new()),
            slots: engine.max_connections.map(|cap| Arc::new(Semaphore::new(cap))),
        }
    }

    async fn dial(&self, engine: &Engine) -> Result<AsyncMetaConnection> {
        timed(
            engine.connect_timeout,
            AsyncMetaConnection::connect(self.addrs.as_slice()),
        )
        .await
    }

    async fn checkout(&self, engine: &Engine) -> Result<AsyncCheckout<'_>> {
        let slot = match &self.slots {
            Some(slots) => Some(
                timed(engine.connect_timeout, async {
                    Arc::clone(slots)
                        .acquire_owned()
                        .await
                        .map_err(|_| Error::protocol("connection pool closed"))
                })
                .await
                .map_err(|error| match error {
                    Error::Io(error) if error.kind() == std::io::ErrorKind::TimedOut => {
                        Error::Io(std::io::Error::new(error.kind(), "connection pool exhausted"))
                    }
                    other => other,
                })?,
            ),
            None => None,
        };
        let mut checkout = AsyncCheckout {
            server: self,
            connection: None,
            pooled: true,
            _slot: slot,
        };
        // Idle connections may have been closed by the server or a
        // middlebox while pooled: skip the old ones, probe the rest and
        // discard instead of handing a dead connection to the caller.
        loop {
            let Some((connection, since)) = self.idle.lock().unwrap().pop() else {
                break;
            };
            if since.elapsed() <= engine.max_idle_age && connection.is_reusable() {
                checkout.connection = Some(connection);
                return Ok(checkout);
            }
        }
        checkout.connection = Some(self.dial(engine).await?);
        checkout.pooled = false;
        Ok(checkout)
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
/// # use memcache::exp::{AsyncMetaClient, Ttl};
/// # async fn example() -> memcache::exp::Result<()> {
/// let client = AsyncMetaClient::connect("127.0.0.1:11211").await?;
/// client.set("foo", "bar").ttl(Ttl::secs(60)).send().await?;
/// let result = client.get("foo").send().await?;
/// # Ok(())
/// # }
/// ```
#[derive(Clone)]
pub struct AsyncMetaClient {
    servers: Arc<Vec<AsyncServer>>,
    ids: Arc<Vec<SocketAddr>>,
    router: Arc<dyn Router>,
    engine: Engine,
}

impl MetaClientBuilder {
    /// Connect to one server with this configuration; the async
    /// counterpart of [`connect`](Self::connect).
    pub async fn connect_async<A: ToSocketAddrs>(self, addr: A) -> Result<AsyncMetaClient> {
        self.connect_multiple_async([addr]).await
    }

    /// Connect to several servers with this configuration; the async
    /// counterpart of [`connect_multiple`](Self::connect_multiple).
    pub async fn connect_multiple_async<A: ToSocketAddrs>(
        self,
        addrs: impl IntoIterator<Item = A>,
    ) -> Result<AsyncMetaClient> {
        let mut servers = Vec::new();
        for addr in addrs {
            let resolved: Vec<SocketAddr> = lookup_host(addr).await?.collect();
            if resolved.is_empty() {
                return Err(Error::Usage("address resolved to no socket addresses"));
            }
            servers.push(AsyncServer::new(resolved, &self.engine));
        }
        if servers.is_empty() {
            return Err(Error::Usage("at least one server address is required"));
        }
        let ids = servers.iter().map(|server| server.addrs[0]).collect();
        Ok(AsyncMetaClient {
            servers: Arc::new(servers),
            ids: Arc::new(ids),
            router: self.router,
            engine: self.engine,
        })
    }
}

impl AsyncMetaClient {
    /// Connect to one server with the default configuration; use
    /// [`builder`](Self::builder) to change it.
    pub async fn connect<A: ToSocketAddrs>(addr: A) -> Result<AsyncMetaClient> {
        AsyncMetaClient::connect_multiple([addr]).await
    }

    /// Connect to several servers with the default configuration; keys are
    /// distributed across them by rendezvous hashing (see
    /// [`Router`](super::Router)). Addresses are resolved here, but
    /// connections are dialed lazily, so a down server surfaces at the
    /// first operation; [`noop`](Self::noop) verifies connectivity eagerly.
    pub async fn connect_multiple<A: ToSocketAddrs>(addrs: impl IntoIterator<Item = A>) -> Result<AsyncMetaClient> {
        AsyncMetaClient::builder().connect_multiple_async(addrs).await
    }

    /// Start a [`MetaClientBuilder`](super::MetaClientBuilder) to configure
    /// hashing, pooling and timeouts before connecting.
    pub fn builder() -> MetaClientBuilder {
        MetaClientBuilder::new()
    }

    fn connection_index(&self, key: &[u8]) -> usize {
        route(self.router.as_ref(), key, &self.ids)
    }

    /// Check out a connection, run one command under the io deadline and
    /// return the connection to the pool. A failed exchange drops the
    /// connection; a pooled connection that fails before anything was
    /// written is replaced by a fresh dial and the command retried once.
    ///
    /// Cancellation safe in the sense that matters: dropping the future
    /// mid-exchange drops the checked-out connection instead of returning
    /// it to the pool, since it may carry half a request or an unread
    /// response.
    async fn execute_on(&self, server: usize, command: &MetaCommand) -> std::result::Result<MetaResponse, Failure> {
        let server = &self.servers[server];
        let mut checkout = server.checkout(&self.engine).await.map_err(Failure::before_write)?;
        loop {
            match timed(self.engine.io_timeout, checkout.connection().execute(command)).await {
                Ok(response) => {
                    checkout.put_back(&self.engine);
                    return Ok(response);
                }
                Err(error) => {
                    let written = checkout.connection().written();
                    if checkout.pooled && written == 0 && matches!(error, Error::Io(_)) {
                        checkout.connection = Some(server.dial(&self.engine).await.map_err(Failure::before_write)?);
                        checkout.pooled = false;
                        continue;
                    }
                    return Err(Failure { error, written });
                }
            }
        }
    }

    /// One command on its server, attributed: a request written but not
    /// answered is [`Error::Ambiguous`] when it has side effects.
    async fn execute(&self, key: &[u8], command: &MetaCommand) -> Result<MetaResponse> {
        let index = self.connection_index(key);
        self.execute_on(index, command)
            .await
            .map_err(|failure| failure.attribute(key, failure.written > 0 && command.has_side_effect()))
    }

    /// One batch on one server, with opaque tokens checked; see the
    /// blocking client.
    async fn execute_batch(&self, server: usize, batch: &Batch) -> (Vec<MetaResponse>, Option<Failure>) {
        let server = &self.servers[server];
        let mut checkout = match server.checkout(&self.engine).await {
            Ok(checkout) => checkout,
            Err(error) => return (Vec::new(), Some(Failure::before_write(error))),
        };
        loop {
            let exchange = checkout.connection().execute_payload(&batch.payload, batch.len());
            let (responses, error) = match timed(self.engine.io_timeout, async { Ok(exchange.await) }).await {
                Ok(outcome) => outcome,
                Err(error) => (Vec::new(), Some(error)),
            };
            // A response out of order means every pairing is suspect:
            // drop them all, the attribution below says which commands
            // may have landed.
            let (responses, error) = match error {
                Some(error) => (responses, Some(error)),
                None => match core::check_batch_order(&responses) {
                    Ok(()) => (responses, None),
                    Err(error) => (Vec::new(), Some(error)),
                },
            };
            match error {
                None => {
                    checkout.put_back(&self.engine);
                    return (responses, None);
                }
                Some(error) => {
                    let written = checkout.connection().written();
                    if checkout.pooled && written == 0 && matches!(error, Error::Io(_)) {
                        match server.dial(&self.engine).await {
                            Ok(connection) => {
                                checkout.connection = Some(connection);
                                checkout.pooled = false;
                                continue;
                            }
                            Err(error) => return (Vec::new(), Some(Failure::before_write(error))),
                        }
                    }
                    return (responses, Some(Failure { error, written }));
                }
            }
        }
    }

    /// Run one raw command for `key` on its server; the scenario layer's
    /// single-key exchange. Transport errors are attributed to `key`.
    pub(crate) async fn exchange(&self, key: &[u8], command: &MetaCommand) -> Result<MetaCommandResult> {
        command.validate()?;
        parse_meta_result(self.execute(key, command).await?)
    }

    /// Run raw commands grouped per server, the groups exchanged
    /// concurrently, and return one result per command in input order.
    /// Every group runs even when another fails; a failed group's commands
    /// each carry the group's error, attributed per command.
    pub(crate) async fn exchange_many(&self, commands: Vec<(Vec<u8>, MetaCommand)>) -> Vec<Result<MetaCommandResult>> {
        let mut groups: Vec<Vec<usize>> = vec![Vec::new(); self.servers.len()];
        let mut outputs: Vec<Option<Result<MetaCommandResult>>> = (0..commands.len()).map(|_| None).collect();
        for (index, (key, command)) in commands.iter().enumerate() {
            match command.validate() {
                Ok(()) => groups[self.connection_index(key)].push(index),
                Err(error) => outputs[index] = Some(Err(error)),
            }
        }
        let mut batches = Vec::new();
        for (server, indices) in groups.iter().enumerate() {
            if indices.is_empty() {
                continue;
            }
            match Batch::new(indices.iter().map(|&index| &commands[index].1)) {
                Ok(batch) => batches.push((server, indices, batch)),
                Err(error) => {
                    for &index in indices {
                        outputs[index] = Some(Err(error.duplicate()));
                    }
                }
            }
        }
        let exchanges = batches
            .iter()
            .map(|(server, _, batch)| self.execute_batch(*server, batch))
            .collect();
        for ((_, indices, batch), (responses, failure)) in batches.iter().zip(join_all(exchanges).await) {
            for (position, &index) in indices.iter().enumerate() {
                outputs[index] = Some(match (responses.get(position), &failure) {
                    (Some(response), _) => parse_meta_result(response.clone()),
                    (None, Some(failure)) => Err(batch.attribute(position, &commands[index].0, failure)),
                    (None, None) => Err(Error::protocol("batch response missing")),
                });
            }
        }
        outputs.into_iter().map(|output| output.unwrap()).collect()
    }

    /// Read a key.
    pub fn get(&self, key: impl AsRef<[u8]>) -> Request<'_, AsyncMetaClient, Get> {
        Request::new(self, Get::new(key))
    }

    /// Store raw bytes under a key. The protocol layer does no
    /// serialization; set the stored client flags with
    /// [`client_flags`](Request::client_flags).
    pub fn set(&self, key: impl AsRef<[u8]>, value: impl AsRef<[u8]>) -> Request<'_, AsyncMetaClient, Set> {
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
    pub async fn run<O: Operation>(&self, operation: O) -> Result<O::Output> {
        let command = operation.prepare()?;
        command.validate()?;
        let response = self.execute(operation.key(), &command).await?;
        operation.parse(parse_meta_result(response)?)
    }

    /// Run several operations, split per server and pipelined; the
    /// per-server groups are exchanged concurrently, so a multi-server
    /// batch costs about one round trip in total.
    ///
    /// All operations are validated before anything is written; a validation
    /// failure is the outer error and guarantees nothing executed. After
    /// that, every operation gets its own entry in input order: a transport
    /// failure fails the unanswered operations of that server's group
    /// (those already answered keep their results) while the remaining
    /// groups still execute. A failed operation that was written in full
    /// and has side effects is [`Error::Ambiguous`]; the rest were not
    /// applied. Semantic outcomes (miss, CAS mismatch, ...) are not errors;
    /// they show up inside [`OpResult`]. A batch is not a transaction.
    pub async fn run_batch(&self, operations: impl IntoIterator<Item = Op>) -> Result<Vec<Result<OpResult, Error>>> {
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
    ) -> Result<Vec<Result<O::Output, Error>>> {
        let operations: Vec<O> = operations.into_iter().collect();
        self.run_all(&operations).await
    }

    async fn run_all<O: Operation>(&self, operations: &[O]) -> Result<Vec<Result<O::Output, Error>>> {
        let plan = core::plan(operations, self.servers.len(), |key| self.connection_index(key))?;
        let mut outputs: Vec<Option<Result<O::Output>>> = (0..operations.len()).map(|_| None).collect();
        let mut batches = Vec::new();
        for (server, indices) in plan.groups.iter().enumerate() {
            if !indices.is_empty() {
                batches.push((
                    server,
                    indices,
                    Batch::new(indices.iter().map(|&index| &plan.commands[index]))?,
                ));
            }
        }
        // One exchange future per non-empty server group, run concurrently:
        // the batch takes one round trip total, not one per server.
        let exchanges = batches
            .iter()
            .map(|(server, _, batch)| self.execute_batch(*server, batch))
            .collect();
        for ((_, indices, batch), (responses, failure)) in batches.iter().zip(join_all(exchanges).await) {
            for (position, &index) in indices.iter().enumerate() {
                outputs[index] = Some(match (responses.get(position), &failure) {
                    (Some(response), _) => {
                        parse_meta_result(response.clone()).and_then(|wire| operations[index].parse(wire))
                    }
                    (None, Some(failure)) => Err(batch.attribute(position, operations[index].key(), failure)),
                    (None, None) => Err(Error::protocol("batch response missing")),
                });
            }
        }
        Ok(outputs
            .into_iter()
            .map(|output| output.expect("batch executor left an operation unresolved"))
            .collect())
    }

    /// Round-trip an `mn` no-op on every server; useful as a connection
    /// health check.
    pub async fn noop(&self) -> Result<()> {
        let noop = build_noop();
        for server in 0..self.servers.len() {
            let response = self
                .execute_on(server, &noop)
                .await
                .map_err(|failure| failure.attribute(b"", false))?;
            if response.rc != ReturnCode::Mn {
                return Err(Error::protocol("unexpected no-op response"));
            }
        }
        Ok(())
    }

    /// Fetch `me` debug fields for a key; `None` on a miss.
    pub async fn debug(&self, key: impl AsRef<[u8]>) -> Result<Option<HashMap<String, String>>> {
        let key = key.as_ref().to_vec();
        let command = build_debug(&key)?;
        let response = self.execute(&key, &command).await?;
        parse_debug_result(&response)
    }
}

impl<'a, O: Operation> Request<'a, AsyncMetaClient, O> {
    /// Execute the request and return its typed result.
    pub async fn send(self) -> Result<O::Output> {
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

        use super::super::client::tests::{FirstByte, char_for};

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
            reader.get_mut().write_all(b"EN O0\r\n").unwrap();
        });

        let listener1 = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr1 = listener1.local_addr().unwrap();
        let gate1 = std::thread::spawn(move || {
            let (stream, _) = listener1.accept().unwrap();
            let mut reader = BufReader::new(stream);
            let mut line = Vec::new();
            reader.read_until(b'\n', &mut line).unwrap();
            sender.send(()).unwrap();
            reader.get_mut().write_all(b"EN O0\r\n").unwrap();
        });

        let client = AsyncMetaClient::builder()
            .router(FirstByte)
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
