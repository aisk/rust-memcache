//! Blocking client over the semantic layer.

use std::collections::HashMap;
use std::io;
use std::net::{SocketAddr, TcpStream, ToSocketAddrs};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

use super::connection::MetaConnection;
use super::core::{self, Batch, Failure, Operation};
use super::error::{Error, Result};
use super::meta_api::{
    ArithmeticMode, MetaCommandResult, build_debug, build_noop, parse_debug_result, parse_meta_result,
};
use super::meta_command::{MetaCommand, MetaResponse, ReturnCode};
use super::operation::{Arithmetic, Delete, Get, Op, Set};
use super::request::Request;
use super::result::OpResult;
use super::router::{Rendezvous, Router, ServerAddress};

pub(crate) const DEFAULT_MAX_IDLE: usize = 8;

/// Route a key with a [`Router`], clamping a misbehaving result into
/// range.
pub(crate) fn route(router: &dyn Router, key: &[u8], servers: &[String]) -> usize {
    router.route(key, servers).min(servers.len() - 1)
}

pub(crate) fn resolve<A: ToSocketAddrs>(addr: A) -> Result<Vec<SocketAddr>> {
    let addrs: Vec<SocketAddr> = addr.to_socket_addrs()?.collect();
    if addrs.is_empty() {
        return Err(Error::Usage("address resolved to no socket addresses"));
    }
    Ok(addrs)
}

/// Cache operations normally complete in milliseconds, so one second is
/// already a generous bound; a hung cache server should fail fast rather
/// than stall its callers.
pub(crate) const DEFAULT_TIMEOUT: Duration = Duration::from_secs(1);

/// Idle connections older than this are not reused: middleboxes and
/// server restarts kill quiet connections, and a dead one costs a failed
/// request. Matches the Go client.
pub(crate) const DEFAULT_MAX_IDLE_AGE: Duration = Duration::from_secs(90);

/// Engine settings shared by both clients.
#[derive(Clone, Copy)]
pub(crate) struct Engine {
    pub(crate) connect_timeout: Option<Duration>,
    pub(crate) io_timeout: Option<Duration>,
    pub(crate) max_idle: usize,
    pub(crate) max_idle_age: Duration,
    pub(crate) max_connections: Option<usize>,
}

impl Default for Engine {
    fn default() -> Engine {
        Engine {
            connect_timeout: Some(DEFAULT_TIMEOUT),
            io_timeout: Some(DEFAULT_TIMEOUT),
            max_idle: DEFAULT_MAX_IDLE,
            max_idle_age: DEFAULT_MAX_IDLE_AGE,
            max_connections: None,
        }
    }
}

/// Dial one of a server's addresses under one shared deadline, matching
/// the tokio client's behavior.
pub(crate) fn dial(addrs: &[SocketAddr], engine: &Engine) -> Result<MetaConnection> {
    let stream = match engine.connect_timeout {
        Some(duration) => {
            let deadline = Instant::now() + duration;
            let mut last_error = None;
            let mut connected = None;
            for addr in addrs {
                let remaining = deadline
                    .checked_duration_since(Instant::now())
                    .filter(|remaining| !remaining.is_zero());
                let Some(remaining) = remaining else {
                    break;
                };
                match TcpStream::connect_timeout(addr, remaining) {
                    Ok(stream) => {
                        connected = Some(stream);
                        break;
                    }
                    Err(error) => last_error = Some(error),
                }
            }
            // `addrs` is never empty, so a miss always has an error.
            connected.ok_or_else(|| last_error.unwrap_or_else(|| io::Error::from(io::ErrorKind::TimedOut)))?
        }
        None => TcpStream::connect(addrs)?,
    };
    stream.set_nodelay(true)?;
    let mut connection = MetaConnection::from_stream(stream);
    connection.set_io_timeout(engine.io_timeout);
    Ok(connection)
}

/// One server: its resolved addresses, a stack of idle connections and
/// the count of connections in use.
struct Server {
    addrs: Vec<SocketAddr>,
    idle: Mutex<Vec<(MetaConnection, Instant)>>,
    active: Mutex<usize>,
    released: Condvar,
}

/// A connection checked out of a [`Server`]. Dropping it discards the
/// connection and frees its slot; [`put_back`](Self::put_back) returns it
/// to the idle stack instead.
struct Checkout<'a> {
    server: &'a Server,
    connection: Option<MetaConnection>,
    /// Whether the connection came from the idle stack (and so may have
    /// died unnoticed) rather than a fresh dial.
    pooled: bool,
}

impl Checkout<'_> {
    fn connection(&mut self) -> &mut MetaConnection {
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

impl Drop for Checkout<'_> {
    fn drop(&mut self) {
        *self.server.active.lock().unwrap() -= 1;
        self.server.released.notify_one();
    }
}

impl Server {
    fn new(addrs: Vec<SocketAddr>) -> Server {
        Server {
            addrs,
            idle: Mutex::new(Vec::new()),
            active: Mutex::new(0),
            released: Condvar::new(),
        }
    }

    /// Take a slot, waiting while the server is at `max_connections`; the
    /// wait is bounded by the connect timeout.
    fn acquire(&self, engine: &Engine) -> Result<()> {
        let mut active = self.active.lock().unwrap();
        if let Some(cap) = engine.max_connections {
            let deadline = engine.connect_timeout.map(|timeout| Instant::now() + timeout);
            while *active >= cap {
                active = match deadline {
                    Some(deadline) => {
                        let remaining = deadline.saturating_duration_since(Instant::now());
                        if remaining.is_zero() {
                            return Err(io::Error::new(io::ErrorKind::TimedOut, "connection pool exhausted").into());
                        }
                        self.released.wait_timeout(active, remaining).unwrap().0
                    }
                    None => self.released.wait(active).unwrap(),
                };
            }
        }
        *active += 1;
        Ok(())
    }

    fn checkout(&self, engine: &Engine) -> Result<Checkout<'_>> {
        self.acquire(engine)?;
        let mut checkout = Checkout {
            server: self,
            connection: None,
            pooled: true,
        };
        // Idle connections may have been closed by the server or a
        // middlebox while pooled: skip the old ones, probe the rest and
        // discard instead of handing a dead connection to the caller.
        loop {
            let Some((mut connection, since)) = self.idle.lock().unwrap().pop() else {
                break;
            };
            if since.elapsed() <= engine.max_idle_age && connection.is_reusable() {
                checkout.connection = Some(connection);
                return Ok(checkout);
            }
        }
        checkout.connection = Some(dial(&self.addrs, engine)?);
        checkout.pooled = false;
        Ok(checkout)
    }
}

/// Configures a [`MetaClient`] (or the tokio `AsyncMetaClient`) before
/// connecting.
///
/// The configuration is fixed once `connect*` builds the client: clones of
/// the client share both the connection pools and these settings, so two
/// clones can never route the same key differently or pool differently.
///
/// ```no_run
/// # use memcache::exp::MetaClient;
/// # use std::time::Duration;
/// let client = MetaClient::builder()
///     .max_idle(16)
///     .io_timeout(Some(Duration::from_millis(200)))
///     .connect("127.0.0.1:11211")
///     .unwrap();
/// ```
#[derive(Clone)]
pub struct MetaClientBuilder {
    pub(crate) router: Arc<dyn Router>,
    pub(crate) engine: Engine,
}

impl MetaClientBuilder {
    pub fn new() -> MetaClientBuilder {
        MetaClientBuilder {
            router: Arc::new(Rendezvous::new()),
            engine: Engine::default(),
        }
    }

    /// Replace the key hash used by the default [`Rendezvous`] router.
    pub fn hash_function(mut self, hash_function: fn(&[u8]) -> u64) -> MetaClientBuilder {
        self.router = Arc::new(Rendezvous::with_hash_function(hash_function));
        self
    }

    /// Replace the [`Router`] that picks the server for a key (default
    /// [`Rendezvous`]).
    pub fn router(mut self, router: impl Router) -> MetaClientBuilder {
        self.router = Arc::new(router);
        self
    }

    /// Cap how many idle connections each server retains (default 8).
    /// Concurrency above the cap dials extra connections, which are dropped
    /// when returned.
    pub fn max_idle(mut self, max_idle: usize) -> MetaClientBuilder {
        self.engine.max_idle = max_idle;
        self
    }

    /// Do not reuse a connection that sat idle longer than this (default
    /// 90 seconds); quiet connections get killed by middleboxes and server
    /// restarts, and a dead one costs a failed request.
    pub fn max_idle_age(mut self, age: Duration) -> MetaClientBuilder {
        self.engine.max_idle_age = age;
        self
    }

    /// Cap the connections in use per server (default unlimited). At the
    /// cap, checkout waits for a connection to be returned, up to the
    /// connect timeout, then fails with a timed out io error. The cap
    /// bounds the blast radius of a latency spike: without it every
    /// stalled caller dials one more connection.
    pub fn max_connections(mut self, max_connections: Option<usize>) -> MetaClientBuilder {
        self.engine.max_connections = max_connections.filter(|&cap| cap > 0);
        self
    }

    /// Limit how long dialing a server may take (default 1 second; `None`
    /// removes the limit).
    pub fn connect_timeout(mut self, timeout: Option<Duration>) -> MetaClientBuilder {
        self.engine.connect_timeout = timeout;
        self
    }

    /// Limit how long an exchange may take (default 1 second; `None`
    /// removes the limit). Both clients apply it to a whole command or
    /// batch exchange - request write plus response reads - not to
    /// individual socket operations. A timeout poisons the connection like
    /// any other transport error; one that strikes after a side-effecting
    /// request was written surfaces as [`Error::Ambiguous`].
    pub fn io_timeout(mut self, timeout: Option<Duration>) -> MetaClientBuilder {
        self.engine.io_timeout = timeout;
        self
    }

    /// Connect to one server with this configuration.
    pub fn connect<A: ToSocketAddrs + ServerAddress>(self, addr: A) -> Result<MetaClient> {
        self.connect_multiple([addr])
    }

    /// Connect to several servers with this configuration; keys are
    /// distributed across them by the [`Router`] (rendezvous hashing by
    /// default, so adding or removing a server anywhere in the list only
    /// moves that server's keys). Addresses are resolved here, but
    /// connections are dialed lazily, so a down server surfaces at the
    /// first operation; `noop()` verifies connectivity eagerly.
    pub fn connect_multiple<A: ToSocketAddrs + ServerAddress>(
        self,
        addrs: impl IntoIterator<Item = A>,
    ) -> Result<MetaClient> {
        let mut servers = Vec::new();
        let mut ids = Vec::new();
        for addr in addrs {
            ids.push(addr.identity());
            servers.push(Server::new(resolve(addr)?));
        }
        if servers.is_empty() {
            return Err(Error::Usage("at least one server address is required"));
        }
        Ok(MetaClient {
            servers: Arc::new(servers),
            ids: Arc::new(ids),
            router: self.router,
            engine: self.engine,
        })
    }
}

impl Default for MetaClientBuilder {
    fn default() -> MetaClientBuilder {
        MetaClientBuilder::new()
    }
}

/// A blocking meta protocol client.
///
/// The verbs return lazy [`Request`] builders; chain options and finish with
/// [`send`](Request::send). With multiple servers, keys are routed by a
/// [`Router`] (rendezvous hashing by default) and batches are split per
/// server.
///
/// The client is cheap to clone and shareable across threads; clones share
/// the connection pools and configuration. Hashing, pooling and timeouts
/// are set on [`MetaClientBuilder`] before connecting and stay fixed for
/// the client's lifetime, so clones cannot diverge. Each server keeps a
/// stack of idle connections (bounded by
/// [`max_idle`](MetaClientBuilder::max_idle) and
/// [`max_idle_age`](MetaClientBuilder::max_idle_age)); a pooled connection
/// that dies before the first byte of a request is written is replaced by
/// a fresh dial once, transparently; a connection that fails mid-exchange
/// is dropped instead of being reused, and the request's error says
/// whether it may have landed ([`Error::Ambiguous`]).
///
/// ```no_run
/// # use memcache::exp::{MetaClient, Ttl};
/// let client = MetaClient::connect("127.0.0.1:11211").unwrap();
/// client.set("foo", "bar").ttl(Ttl::secs(60)).send().unwrap();
/// let result = client.get("foo").send().unwrap();
/// ```
#[derive(Clone)]
pub struct MetaClient {
    servers: Arc<Vec<Server>>,
    /// One identifying address per server, handed to the router.
    ids: Arc<Vec<String>>,
    router: Arc<dyn Router>,
    engine: Engine,
}

impl MetaClient {
    /// Connect to one server with the default configuration; use
    /// [`builder`](Self::builder) to change it.
    pub fn connect<A: ToSocketAddrs + ServerAddress>(addr: A) -> Result<MetaClient> {
        MetaClient::connect_multiple([addr])
    }

    /// Connect to several servers with the default configuration; keys are
    /// distributed across them by rendezvous hashing (see
    /// [`Router`]). Addresses are resolved here, but connections are
    /// dialed lazily, so a down server surfaces at the first operation;
    /// [`noop`](Self::noop) verifies connectivity eagerly.
    pub fn connect_multiple<A: ToSocketAddrs + ServerAddress>(
        addrs: impl IntoIterator<Item = A>,
    ) -> Result<MetaClient> {
        MetaClient::builder().connect_multiple(addrs)
    }

    /// Start a [`MetaClientBuilder`] to configure hashing, pooling and
    /// timeouts before connecting.
    pub fn builder() -> MetaClientBuilder {
        MetaClientBuilder::new()
    }

    fn connection_index(&self, key: &[u8]) -> usize {
        route(self.router.as_ref(), key, &self.ids)
    }

    /// Check out a connection, run one transport exchange on it and return
    /// it to the pool. A failed exchange leaves the stream in an unknown
    /// state, so the connection is dropped instead of returned; a pooled
    /// connection that fails before anything was written is replaced by a
    /// fresh dial and the exchange retried once. The failure reports how
    /// much of the request was written, for attribution.
    fn with_connection<T>(
        &self,
        server: usize,
        mut exchange: impl FnMut(&mut MetaConnection) -> Result<T>,
    ) -> std::result::Result<T, Failure> {
        let server = &self.servers[server];
        let mut checkout = server.checkout(&self.engine).map_err(Failure::before_write)?;
        loop {
            match exchange(checkout.connection()) {
                Ok(value) => {
                    checkout.put_back(&self.engine);
                    return Ok(value);
                }
                Err(error) => {
                    let written = checkout.connection().written();
                    if Failure::redialable(checkout.pooled, written, &error) {
                        checkout.connection = Some(dial(&server.addrs, &self.engine).map_err(Failure::before_write)?);
                        checkout.pooled = false;
                        continue;
                    }
                    return Err(Failure { error, written });
                }
            }
        }
    }

    /// One command on its server, attributed: a request written in full
    /// but not answered is [`Error::Ambiguous`] when it has side effects;
    /// a partially written one was never parsed by the server.
    fn execute(&self, key: &[u8], command: &MetaCommand) -> Result<MetaResponse> {
        let index = self.connection_index(key);
        let payload = command.encode()?;
        self.with_connection(index, |connection| connection.execute_encoded(&payload))
            .map_err(|failure| failure.attribute(key, failure.written >= payload.len() && command.has_side_effect()))
    }

    /// One batch on one server: the payload is written whole, the
    /// responses read back and checked against the opaque tokens stamped
    /// on each command, so a reordered or mismatched response poisons the
    /// connection instead of being paired with the wrong command.
    fn execute_batch(&self, server: usize, batch: &Batch) -> (Vec<MetaResponse>, Option<Failure>) {
        let server = &self.servers[server];
        let mut checkout = match server.checkout(&self.engine) {
            Ok(checkout) => checkout,
            Err(error) => return (Vec::new(), Some(Failure::before_write(error))),
        };
        loop {
            let (responses, error) = checkout.connection().execute_payload(&batch.payload, batch.len());
            let (responses, error) = core::check_batch(responses, error);
            match error {
                None => {
                    checkout.put_back(&self.engine);
                    return (responses, None);
                }
                Some(error) => {
                    let written = checkout.connection().written();
                    if Failure::redialable(checkout.pooled, written, &error) {
                        match dial(&server.addrs, &self.engine) {
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

    /// Run one raw command for `key` on its server; the high-level layer's
    /// single-key exchange. Transport errors are attributed to `key`.
    pub(crate) fn exchange(&self, key: &[u8], command: &MetaCommand) -> Result<MetaCommandResult> {
        command.validate()?;
        parse_meta_result(self.execute(key, command)?)
    }

    /// Run raw commands grouped per server, one round trip each, and
    /// return one result per command in input order. Every group runs
    /// even when another fails; a failed group's commands each carry the
    /// group's error, attributed per command (see [`Batch`]).
    pub(crate) fn exchange_many(&self, commands: &[(Vec<u8>, MetaCommand)]) -> Vec<Result<MetaCommandResult>> {
        let mut groups: Vec<Vec<usize>> = vec![Vec::new(); self.servers.len()];
        let mut outputs: Vec<Option<Result<MetaCommandResult>>> = (0..commands.len()).map(|_| None).collect();
        for (index, (key, command)) in commands.iter().enumerate() {
            match command.validate() {
                Ok(()) => groups[self.connection_index(key)].push(index),
                Err(error) => outputs[index] = Some(Err(error)),
            }
        }
        for (server, indices) in groups.iter().enumerate() {
            if indices.is_empty() {
                continue;
            }
            let batch = match Batch::new(indices.iter().map(|&index| &commands[index].1)) {
                Ok(batch) => batch,
                Err(error) => {
                    for &index in indices {
                        outputs[index] = Some(Err(error.duplicate()));
                    }
                    continue;
                }
            };
            let (responses, failure) = self.execute_batch(server, &batch);
            let mut responses = responses.into_iter().map(Some);
            for (position, &index) in indices.iter().enumerate() {
                outputs[index] = Some(match (responses.next().flatten(), &failure) {
                    (Some(response), _) => parse_meta_result(response),
                    (None, Some(failure)) => Err(batch.attribute(position, &commands[index].0, failure)),
                    (None, None) => Err(Error::protocol("batch response missing")),
                });
            }
        }
        outputs.into_iter().map(|output| output.unwrap()).collect()
    }

    /// Read a key.
    pub fn get(&self, key: impl AsRef<[u8]>) -> Request<'_, MetaClient, Get> {
        Request::new(self, Get::new(key))
    }

    /// Store raw bytes under a key. The protocol layer does no
    /// serialization; set the stored client flags with
    /// [`client_flags`](Request::client_flags).
    pub fn set(&self, key: impl AsRef<[u8]>, value: impl AsRef<[u8]>) -> Request<'_, MetaClient, Set> {
        Request::new(self, Set::new(key, value))
    }

    /// Delete a key.
    pub fn delete(&self, key: impl AsRef<[u8]>) -> Request<'_, MetaClient, Delete> {
        Request::new(self, Delete::new(key))
    }

    /// Increment a counter (delta defaults to 1).
    pub fn increment(&self, key: impl AsRef<[u8]>) -> Request<'_, MetaClient, Arithmetic> {
        Request::new(self, Arithmetic::new(key))
    }

    /// Decrement a counter (delta defaults to 1); saturates at zero.
    pub fn decrement(&self, key: impl AsRef<[u8]>) -> Request<'_, MetaClient, Arithmetic> {
        let operation = Arithmetic {
            mode: ArithmeticMode::Decrement,
            ..Arithmetic::new(key)
        };
        Request::new(self, operation)
    }

    /// Run a standalone operation value; [`send`](Request::send) is sugar
    /// for this.
    pub fn run<O: Operation>(&self, operation: O) -> Result<O::Output> {
        let command = operation.prepare()?;
        command.validate()?;
        let response = self.execute(operation.key(), &command)?;
        operation.parse(parse_meta_result(response)?)
    }

    /// Run several operations, split per server and pipelined with one
    /// round trip per server.
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
    ///
    /// ```no_run
    /// # use memcache::exp::{Get, MetaClient, Set, Ttl};
    /// # let client = MetaClient::connect("127.0.0.1:11211").unwrap();
    /// let results = client.run_batch(vec![
    ///     Set::new("foo", "bar").ttl(Ttl::secs(60)).into(),
    ///     Get::new("baz").into(),
    /// ]).unwrap();
    /// let stored = results[0].as_ref().unwrap();
    /// ```
    pub fn run_batch(&self, operations: impl IntoIterator<Item = Op>) -> Result<Vec<Result<OpResult, Error>>> {
        let operations: Vec<Op> = operations.into_iter().collect();
        self.run_all(&operations)
    }

    /// Run several operations of one kind with typed results - a batch
    /// without the [`Op`]/[`OpResult`] wrapping. The multiget:
    /// `client.run_many(keys.iter().map(Get::new))`. Execution and failure
    /// semantics are those of [`run_batch`](Self::run_batch).
    pub fn run_many<O: Operation>(
        &self,
        operations: impl IntoIterator<Item = O>,
    ) -> Result<Vec<Result<O::Output, Error>>> {
        let operations: Vec<O> = operations.into_iter().collect();
        self.run_all(&operations)
    }

    fn run_all<O: Operation>(&self, operations: &[O]) -> Result<Vec<Result<O::Output, Error>>> {
        let plan = core::plan(operations, self.servers.len(), |key| self.connection_index(key))?;
        let mut outputs: Vec<Option<Result<O::Output>>> = (0..operations.len()).map(|_| None).collect();
        for (server, indices) in plan.groups.iter().enumerate() {
            if indices.is_empty() {
                continue;
            }
            let batch = Batch::new(indices.iter().map(|&index| &plan.commands[index]))?;
            let (responses, failure) = self.execute_batch(server, &batch);
            let mut responses = responses.into_iter().map(Some);
            for (position, &index) in indices.iter().enumerate() {
                outputs[index] = Some(match (responses.next().flatten(), &failure) {
                    (Some(response), _) => parse_meta_result(response).and_then(|wire| operations[index].parse(wire)),
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
    pub fn noop(&self) -> Result<()> {
        for server in 0..self.servers.len() {
            let response = self
                .with_connection(server, |connection| connection.execute(&build_noop()))
                .map_err(|failure| failure.attribute(b"", false))?;
            if response.rc != ReturnCode::Mn {
                return Err(Error::protocol("unexpected no-op response"));
            }
        }
        Ok(())
    }

    /// Fetch `me` debug fields for a key; `None` on a miss.
    pub fn debug(&self, key: impl AsRef<[u8]>) -> Result<Option<HashMap<String, String>>> {
        let key = key.as_ref().to_vec();
        let command = build_debug(&key)?;
        let response = self.execute(&key, &command)?;
        parse_debug_result(&response)
    }
}

impl<'a, O: Operation> Request<'a, MetaClient, O> {
    /// Execute the request and return its typed result.
    pub fn send(self) -> Result<O::Output> {
        let Request { client, operation } = self;
        client.run(operation)
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use std::io::{BufRead, BufReader, Read, Write};
    use std::net::TcpListener;
    use std::thread::JoinHandle;

    use super::super::result::{GetStatus, MutationStatus};
    use super::super::ttl::Ttl;
    use super::*;

    /// A single-connection server that answers each request with the next
    /// scripted response and records the request headers it saw.
    fn scripted_server(responses: Vec<&'static [u8]>) -> (SocketAddr, JoinHandle<Vec<Vec<u8>>>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let handle = std::thread::spawn(move || {
            let (stream, _) = listener.accept().unwrap();
            let mut reader = BufReader::new(stream);
            let mut requests = Vec::new();
            for response in responses {
                let mut header = Vec::new();
                reader.read_until(b'\n', &mut header).unwrap();
                // Consume the data block of an ms request.
                if header.starts_with(b"ms ") {
                    let line = String::from_utf8(header.clone()).unwrap();
                    let datalen: usize = line.split_whitespace().nth(2).unwrap().parse().unwrap();
                    let mut value = vec![0u8; datalen + 2];
                    reader.read_exact(&mut value).unwrap();
                }
                requests.push(header);
                reader.get_mut().write_all(response).unwrap();
            }
            requests
        });
        (addr, handle)
    }

    /// Route keys by their first byte, so tests can steer each key to a
    /// chosen server: `char_for(n)` yields a leading character for server
    /// `n` of two.
    pub(crate) struct FirstByte;

    impl Router for FirstByte {
        fn route(&self, key: &[u8], servers: &[String]) -> usize {
            key[0] as usize % servers.len()
        }
    }

    pub(crate) fn char_for(bucket: usize) -> char {
        (b'0'..=b'z').find(|&byte| byte as usize % 2 == bucket).unwrap() as char
    }

    #[test]
    fn misbehaving_router_is_clamped() {
        struct Beyond;
        impl Router for Beyond {
            fn route(&self, _: &[u8], _: &[String]) -> usize {
                usize::MAX
            }
        }
        let (addr, _server) = scripted_server(vec![]);
        let client = MetaClient::builder().router(Beyond).connect(addr).unwrap();
        assert_eq!(client.connection_index(b"k"), 0);
    }

    #[test]
    fn client_roundtrip() {
        // A single accepted connection serves every operation: the pool
        // reuses it across the whole test.
        let (addr, server) = scripted_server(vec![
            b"HD\r\n",
            b"VA 3 f0\r\nbar\r\n",
            b"NS\r\n",
            b"VA 2\r\n42\r\n",
            b"HD\r\n",
            b"MN\r\n",
        ]);
        let client = MetaClient::connect(addr).unwrap();

        let stored = client.set("foo", "bar").send().unwrap();
        assert_eq!(stored.status, MutationStatus::Applied);

        let fetched = client.get("foo").send().unwrap();
        assert_eq!(fetched.status, GetStatus::Hit);
        assert_eq!(fetched.value.as_deref(), Some(&b"bar"[..]));

        let added = client.set("foo", "baz").add().send().unwrap();
        assert_eq!(added.status, MutationStatus::AlreadyExists);

        let counter = client.increment("counter").delta(2).send().unwrap();
        assert_eq!(counter.value, Some(42));

        let deleted = client.delete("foo").send().unwrap();
        assert!(deleted.applied());

        client.noop().unwrap();

        let requests = server.join().unwrap();
        assert_eq!(requests[0], b"ms foo 3\r\n".to_vec());
        assert_eq!(requests[1], b"mg foo v f\r\n".to_vec());
        assert_eq!(requests[2], b"ms foo 3 ME\r\n".to_vec());
        assert_eq!(requests[3], b"ma counter v D2\r\n".to_vec());
        assert_eq!(requests[4], b"md foo\r\n".to_vec());
        assert_eq!(requests[5], b"mn\r\n".to_vec());
    }

    #[test]
    fn run_batch_mixed_operations() {
        let (addr, server) = scripted_server(vec![b"HD O0\r\n", b"VA 1 f0 O1\r\n1\r\n", b"NF O2\r\n"]);
        let client = MetaClient::connect(addr).unwrap();

        let results: Vec<_> = client
            .run_batch(vec![
                Set::new("a", "1").ttl(Ttl::secs(60)).into(),
                Get::new("a").into(),
                Delete::new("c").into(),
            ])
            .unwrap()
            .into_iter()
            .map(Result::unwrap)
            .collect();
        assert_eq!(results.len(), 3);
        assert!(results[0].as_mutation().unwrap().applied());
        assert_eq!(results[1].as_get().unwrap().value.as_deref(), Some(&b"1"[..]));
        assert_eq!(results[2].as_mutation().unwrap().status, MutationStatus::NotFound);

        // All three commands were written before the first response was read.
        let requests = server.join().unwrap();
        assert_eq!(requests[0], b"ms a 1 T60 O0\r\n".to_vec());
        assert_eq!(requests[1], b"mg a v f O1\r\n".to_vec());
        assert_eq!(requests[2], b"md c O2\r\n".to_vec());
    }

    #[test]
    fn run_batch_validates_before_writing() {
        let (addr, server) = scripted_server(vec![b"MN\r\n"]);
        let client = MetaClient::connect(addr).unwrap();

        // The second operation is invalid; nothing must reach the server.
        let error = client.run_batch(vec![
            Set::new("a", "1").into(),
            Delete::new("b").stale_for(Ttl::secs(30)).into(),
        ]);
        assert!(error.is_err());

        client.noop().unwrap();
        let requests = server.join().unwrap();
        assert_eq!(requests, vec![b"mn\r\n".to_vec()]);
    }

    #[test]
    fn run_executes_standalone_operations() {
        let (addr, server) = scripted_server(vec![b"HD\r\n", b"VA 1\r\n1\r\n"]);
        let client = MetaClient::connect(addr).unwrap();

        let operation = client.set("foo", "bar").ttl(Ttl::secs(60)).into_operation();
        assert!(client.run(operation).unwrap().applied());

        let decremented = client.decrement("counter").send().unwrap();
        assert_eq!(decremented.value, Some(1));

        let requests = server.join().unwrap();
        assert_eq!(requests[0], b"ms foo 3 T60\r\n".to_vec());
        assert_eq!(requests[1], b"ma counter MD v D1\r\n".to_vec());
    }

    #[test]
    fn multi_server_routes_by_key() {
        let (addr0, server0) = scripted_server(vec![b"HD\r\n", b"MN\r\n"]);
        let (addr1, server1) = scripted_server(vec![b"VA 1 f0\r\nx\r\n", b"MN\r\n"]);
        let client = MetaClient::builder()
            .router(FirstByte)
            .connect_multiple([addr0, addr1])
            .unwrap();
        let key0 = format!("{}a", char_for(0));
        let key1 = format!("{}b", char_for(1));

        assert!(client.set(&key0, "v").send().unwrap().applied());
        assert!(client.get(&key1).send().unwrap().hit());
        client.noop().unwrap();

        assert_eq!(
            server0.join().unwrap(),
            vec![format!("ms {} 1\r\n", key0).into_bytes(), b"mn\r\n".to_vec()]
        );
        assert_eq!(
            server1.join().unwrap(),
            vec![format!("mg {} v f\r\n", key1).into_bytes(), b"mn\r\n".to_vec()]
        );
    }

    #[test]
    fn multi_server_batch_splits_and_reorders() {
        let (addr0, server0) = scripted_server(vec![b"EN O0\r\n"]);
        let (addr1, server1) = scripted_server(vec![b"HD O0\r\n", b"NF O1\r\n"]);
        let client = MetaClient::builder()
            .router(FirstByte)
            .connect_multiple([addr0, addr1])
            .unwrap();
        let key_set = format!("{}a", char_for(1));
        let key_get = format!("{}b", char_for(0));
        let key_delete = format!("{}c", char_for(1));

        // Interleaved across servers; results must come back in input order.
        let results: Vec<_> = client
            .run_batch(vec![
                Set::new(&*key_set, "v").into(),
                Get::new(&*key_get).into(),
                Delete::new(&*key_delete).into(),
            ])
            .unwrap()
            .into_iter()
            .map(Result::unwrap)
            .collect();
        assert!(results[0].as_mutation().unwrap().applied());
        assert_eq!(results[1].as_get().unwrap().status, GetStatus::Miss);
        assert_eq!(results[2].as_mutation().unwrap().status, MutationStatus::NotFound);

        assert_eq!(
            server0.join().unwrap(),
            vec![format!("mg {} v f O0\r\n", key_get).into_bytes()]
        );
        assert_eq!(
            server1.join().unwrap(),
            vec![
                format!("ms {} 1 O0\r\n", key_set).into_bytes(),
                format!("md {} O1\r\n", key_delete).into_bytes(),
            ]
        );
    }

    #[test]
    fn run_batch_continues_after_group_failure() {
        let (addr0, server0) = scripted_server(vec![b"VA 1 f0 O0\r\nx\r\n"]);
        // A bound-then-dropped listener yields an address that refuses
        // connections, so the second server's group must fail in transport.
        let dead = TcpListener::bind("127.0.0.1:0").unwrap();
        let dead_addr = dead.local_addr().unwrap();
        drop(dead);
        let client = MetaClient::builder()
            .router(FirstByte)
            .connect_multiple([addr0, dead_addr])
            .unwrap();
        let key_live = format!("{}a", char_for(0));
        let key_dead = format!("{}b", char_for(1));

        let results = client
            .run_batch(vec![Get::new(&*key_live).into(), Set::new(&*key_dead, "v").into()])
            .unwrap();
        let live = results[0].as_ref().unwrap().as_get().unwrap();
        assert_eq!(live.value.as_deref(), Some(&b"x"[..]));
        assert!(results[1].is_err());
        server0.join().unwrap();
    }

    #[test]
    fn connect_multiple_rejects_empty() {
        assert!(MetaClient::connect_multiple(Vec::<SocketAddr>::new()).is_err());
    }

    #[test]
    fn io_timeout_poisons_connection() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let handle = std::thread::spawn(move || {
            // First connection: read the request but never respond, so the
            // read times out and the connection is poisoned.
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

        let client = MetaClient::builder()
            .io_timeout(Some(Duration::from_millis(100)))
            .connect(addr)
            .unwrap();
        let start = std::time::Instant::now();
        // The delete was written before the timeout struck: ambiguous.
        let error = client.delete("foo").send().unwrap_err();
        assert!(
            matches!(&error, Error::Ambiguous { key, source } if key == b"foo" && matches!(**source, Error::Timeout { .. })),
            "{error:?}"
        );
        assert!(error.is_ambiguous() && !error.is_retryable());
        assert!(start.elapsed() < Duration::from_secs(5));
        assert!(client.delete("foo").send().unwrap().applied());
        handle.join().unwrap();
    }

    #[test]
    fn io_timeout_bounds_whole_batch_exchange() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let handle = std::thread::spawn(move || {
            let (stream, _) = listener.accept().unwrap();
            let mut reader = BufReader::new(stream);
            let mut line = Vec::new();
            for _ in 0..3 {
                line.clear();
                reader.read_until(b'\n', &mut line).unwrap();
            }
            // Trickle one response every 60ms: each read on its own stays
            // under the limit, but the whole exchange exceeds it.
            for _ in 0..3 {
                std::thread::sleep(Duration::from_millis(60));
                let _ = reader.get_mut().write_all(b"NF\r\n");
            }
        });

        let client = MetaClient::builder()
            .io_timeout(Some(Duration::from_millis(100)))
            .connect(addr)
            .unwrap();
        let start = std::time::Instant::now();
        let results = client
            .run_batch(vec![
                Delete::new("a").into(),
                Delete::new("b").into(),
                Delete::new("c").into(),
            ])
            .unwrap();
        assert!(results.iter().any(|result| result.is_err()));
        assert!(start.elapsed() < Duration::from_secs(2));
        handle.join().unwrap();
    }

    #[test]
    fn connect_timeout_fails_fast() {
        // TEST-NET-1 (192.0.2.0/24) is reserved and unroutable, so the dial
        // either times out or is rejected outright; it must not hang.
        let client = MetaClient::builder()
            .connect_timeout(Some(Duration::from_millis(100)))
            .connect("192.0.2.1:11211")
            .unwrap();
        let start = std::time::Instant::now();
        assert!(client.delete("foo").send().is_err());
        assert!(start.elapsed() < Duration::from_secs(5));
    }

    #[test]
    fn poisoned_connection_is_not_reused() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let handle = std::thread::spawn(move || {
            // First connection (dialed by the first operation): serve one
            // bogus response, which must poison it.
            let (stream, _) = listener.accept().unwrap();
            let mut reader = BufReader::new(stream);
            let mut line = Vec::new();
            reader.read_until(b'\n', &mut line).unwrap();
            reader.get_mut().write_all(b"BOGUS\r\n").unwrap();
            // The next operation must arrive on a fresh connection.
            let (stream, _) = listener.accept().unwrap();
            let mut reader = BufReader::new(stream);
            let mut line = Vec::new();
            reader.read_until(b'\n', &mut line).unwrap();
            reader.get_mut().write_all(b"HD\r\n").unwrap();
        });

        let client = MetaClient::connect(addr).unwrap();
        assert!(client.get("foo").send().is_err());
        assert!(client.delete("foo").send().unwrap().applied());
        handle.join().unwrap();
    }

    #[test]
    fn framed_parse_error_keeps_connection() {
        // "cabc" is a complete response line with an unparsable CAS flag:
        // the decode fails but the stream stays synchronized, so the same
        // connection serves the next operation.
        let (addr, server) = scripted_server(vec![b"HD cabc\r\n", b"HD\r\n"]);
        let client = MetaClient::connect(addr).unwrap();

        assert!(client.delete("foo").send().is_err());
        assert!(client.delete("foo").send().unwrap().applied());
        server.join().unwrap();
    }

    #[test]
    fn stale_pooled_connection_is_discarded() {
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

        let client = MetaClient::connect(addr).unwrap();
        assert!(client.delete("foo").send().unwrap().applied());
        // Give the server's FIN time to arrive before the next checkout.
        std::thread::sleep(Duration::from_millis(50));
        assert!(client.delete("foo").send().unwrap().applied());
        handle.join().unwrap();
    }

    #[test]
    fn max_idle_zero_never_reuses() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let handle = std::thread::spawn(move || {
            // With max_idle 0 nothing is retained after use: every
            // operation must arrive on its own fresh connection.
            for _ in 0..2 {
                let (stream, _) = listener.accept().unwrap();
                let mut reader = BufReader::new(stream);
                let mut line = Vec::new();
                reader.read_until(b'\n', &mut line).unwrap();
                reader.get_mut().write_all(b"HD\r\n").unwrap();
            }
        });

        let client = MetaClient::builder().max_idle(0).connect(addr).unwrap();
        assert!(client.delete("foo").send().unwrap().applied());
        assert!(client.delete("foo").send().unwrap().applied());
        handle.join().unwrap();
    }

    #[test]
    fn plain_read_timeout_is_not_ambiguous() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let handle = std::thread::spawn(move || {
            let (stream, _) = listener.accept().unwrap();
            let mut reader = BufReader::new(stream);
            let mut line = Vec::new();
            reader.read_until(b'\n', &mut line).unwrap();
            std::thread::sleep(Duration::from_millis(300));
        });
        let client = MetaClient::builder()
            .io_timeout(Some(Duration::from_millis(100)))
            .connect(addr)
            .unwrap();
        let error = client.get("foo").send().unwrap_err();
        assert!(matches!(error, Error::Timeout { .. }), "{error:?}");
        assert!(error.is_retryable());
        handle.join().unwrap();
    }

    #[test]
    fn batch_failure_is_attributed_per_command() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let handle = std::thread::spawn(move || {
            // Answer the first command, then close: the rest were written
            // in full, so the side-effecting ones are ambiguous and the
            // plain read is not.
            let (stream, _) = listener.accept().unwrap();
            let mut reader = BufReader::new(stream);
            let mut line = Vec::new();
            for _ in 0..3 {
                line.clear();
                reader.read_until(b'\n', &mut line).unwrap();
            }
            reader.get_mut().write_all(b"HD O0\r\n").unwrap();
        });
        let client = MetaClient::connect(addr).unwrap();
        let results = client
            .run_batch(vec![
                Delete::new("a").into(),
                Delete::new("b").into(),
                Get::new("c").into(),
            ])
            .unwrap();
        assert!(results[0].as_ref().unwrap().as_mutation().unwrap().applied());
        let second = results[1].as_ref().unwrap_err();
        assert!(
            matches!(second, Error::Ambiguous { key, .. } if key == b"b"),
            "{second:?}"
        );
        let third = results[2].as_ref().unwrap_err();
        assert!(matches!(third, Error::Io(_)), "{third:?}");
        handle.join().unwrap();
    }

    #[test]
    fn misordered_batch_response_poisons_the_connection() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let handle = std::thread::spawn(move || {
            let (stream, _) = listener.accept().unwrap();
            let mut reader = BufReader::new(stream);
            let mut line = Vec::new();
            for _ in 0..2 {
                line.clear();
                reader.read_until(b'\n', &mut line).unwrap();
            }
            // Swapped opaque tokens.
            reader.get_mut().write_all(b"HD O1\r\nNF O0\r\n").unwrap();
            // The next operation must arrive on a fresh connection.
            let (stream, _) = listener.accept().unwrap();
            let mut reader = BufReader::new(stream);
            let mut line = Vec::new();
            reader.read_until(b'\n', &mut line).unwrap();
            reader.get_mut().write_all(b"HD\r\n").unwrap();
        });
        let client = MetaClient::connect(addr).unwrap();
        let results = client
            .run_batch(vec![Delete::new("a").into(), Delete::new("b").into()])
            .unwrap();
        for result in &results {
            let error = result.as_ref().unwrap_err();
            assert!(error.is_ambiguous(), "{error:?}");
            assert!(matches!(std::error::Error::source(error), Some(source) if source.to_string().contains("opaque")));
        }
        assert!(client.delete("c").send().unwrap().applied());
        handle.join().unwrap();
    }

    #[test]
    fn partial_batch_responses_are_order_checked() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let handle = std::thread::spawn(move || {
            let (stream, _) = listener.accept().unwrap();
            let mut reader = BufReader::new(stream);
            let mut line = Vec::new();
            for _ in 0..3 {
                line.clear();
                reader.read_until(b'\n', &mut line).unwrap();
            }
            // Two swapped responses, then the connection drops: the two
            // must not be paired with the wrong commands just because the
            // third never came.
            reader.get_mut().write_all(b"HD O1\r\nNF O0\r\n").unwrap();
        });
        let client = MetaClient::connect(addr).unwrap();
        let results = client
            .run_batch(vec![
                Delete::new("a").into(),
                Delete::new("b").into(),
                Delete::new("c").into(),
            ])
            .unwrap();
        for result in &results {
            let error = result.as_ref().unwrap_err();
            assert!(error.is_ambiguous(), "{error:?}");
            assert!(matches!(std::error::Error::source(error), Some(source) if source.to_string().contains("opaque")));
        }
        handle.join().unwrap();
    }

    #[test]
    fn old_idle_connections_are_not_reused() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let handle = std::thread::spawn(move || {
            for _ in 0..2 {
                let (stream, _) = listener.accept().unwrap();
                let mut reader = BufReader::new(stream);
                let mut line = Vec::new();
                reader.read_until(b'\n', &mut line).unwrap();
                reader.get_mut().write_all(b"HD\r\n").unwrap();
            }
        });
        let client = MetaClient::builder()
            .max_idle_age(Duration::from_millis(50))
            .connect(addr)
            .unwrap();
        assert!(client.delete("foo").send().unwrap().applied());
        std::thread::sleep(Duration::from_millis(100));
        assert!(client.delete("foo").send().unwrap().applied());
        handle.join().unwrap();
    }

    #[test]
    fn max_connections_bounds_the_pool() {
        use std::sync::mpsc;
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let (release, gate) = mpsc::channel::<()>();
        let handle = std::thread::spawn(move || {
            // The first connection answers only when released; a second
            // checkout must wait for it instead of dialing.
            let (stream, _) = listener.accept().unwrap();
            let mut reader = BufReader::new(stream);
            let mut line = Vec::new();
            reader.read_until(b'\n', &mut line).unwrap();
            gate.recv().unwrap();
            reader.get_mut().write_all(b"HD\r\n").unwrap();
            line.clear();
            reader.read_until(b'\n', &mut line).unwrap();
            reader.get_mut().write_all(b"HD\r\n").unwrap();
        });
        let client = MetaClient::builder()
            .max_connections(Some(1))
            .connect_timeout(Some(Duration::from_millis(200)))
            .connect(addr)
            .unwrap();
        let first = std::thread::spawn({
            let client = client.clone();
            move || client.delete("a").send().unwrap().applied()
        });
        std::thread::sleep(Duration::from_millis(50));
        // The pool is exhausted: the wait times out.
        let error = client.delete("b").send().unwrap_err();
        assert!(matches!(error, Error::Timeout { .. }), "{error:?}");
        release.send(()).unwrap();
        assert!(first.join().unwrap());
        // Slot freed: served on the same connection.
        assert!(client.delete("c").send().unwrap().applied());
        handle.join().unwrap();
    }
}
