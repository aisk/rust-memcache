//! Blocking client over the semantic layer.

use std::borrow::Cow;
use std::collections::HashMap;
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::net::{SocketAddr, TcpStream, ToSocketAddrs};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

use crate::error::{ClientError, MemcacheError, ServerError};

use super::connection::MetaConnection;
use super::core::{self, Operation};
use super::meta_api::{ArithmeticMode, build_debug, build_noop, parse_debug_result, parse_meta_result};
use super::meta_command::{MetaCommand, MetaResponse, ReturnCode};
use super::operation::{Arithmetic, Delete, Get, Op, Set};
use super::request::Request;
use super::result::OpResult;
use super::value::ToValue;

pub(crate) const DEFAULT_MAX_IDLE: usize = 8;

/// Generous enough that no reasonable workload hits it, while still keeping
/// the connection storm a server latency spike can cause well below
/// memcached's own connection limit (1024 by default).
pub(crate) const DEFAULT_MAX_CONNECTIONS: usize = 128;

/// Idle connections are silently killed by NAT gateways, load balancers and
/// server restarts, typically on timescales of a minute or more; anything
/// older is redialed rather than trusted.
pub(crate) const DEFAULT_IDLE_TIMEOUT: Duration = Duration::from_secs(60);

/// A reused connection is one the pool handed out again; a write failure on
/// it with one of these kinds means the peer had already torn the
/// connection down while it was idle.
pub(crate) fn is_disconnect(error: &MemcacheError) -> bool {
    use std::io::ErrorKind;
    matches!(
        error,
        MemcacheError::IOError(io) if matches!(
            io.kind(),
            ErrorKind::BrokenPipe | ErrorKind::ConnectionReset | ErrorKind::ConnectionAborted | ErrorKind::NotConnected
        )
    )
}

pub(crate) fn pool_exhausted() -> MemcacheError {
    ClientError::Error(Cow::Borrowed(
        "connection limit reached and no connection became free in time",
    ))
    .into()
}

pub(crate) fn default_hash_function(key: &[u8]) -> u64 {
    let mut hasher = DefaultHasher::new();
    key.hash(&mut hasher);
    hasher.finish()
}

/// Jump consistent hash (Lamping & Veach, 2014): maps a key hash to a
/// bucket in `[0, buckets)`. When the bucket count changes from n to n+1,
/// only 1/(n+1) of the keys move, so resizing the server list relocates as
/// few keys as possible.
pub(crate) fn jump_hash(mut key: u64, buckets: usize) -> usize {
    let mut b: i64 = -1;
    let mut j: i64 = 0;
    while j < buckets as i64 {
        b = j;
        key = key.wrapping_mul(2862933555777941757).wrapping_add(1);
        j = ((b + 1) as f64 * ((1u64 << 31) as f64 / ((key >> 33) + 1) as f64)) as i64;
    }
    b as usize
}

pub(crate) fn resolve<A: ToSocketAddrs>(addr: A) -> Result<Vec<SocketAddr>, MemcacheError> {
    let addrs: Vec<SocketAddr> = addr.to_socket_addrs()?.collect();
    if addrs.is_empty() {
        return Err(ClientError::Error(Cow::Borrowed("address resolved to no socket addresses")).into());
    }
    Ok(addrs)
}

/// The connection pool of one server: a stack of idle connections (each
/// with the time it was returned) and the count of checked-out connections.
struct Pool {
    idle: Vec<(MetaConnection, Instant)>,
    checked_out: usize,
}

/// One server: its resolved addresses and its connection pool.
///
/// Checkout takes an in-flight slot (waiting for one, bounded by the
/// connect timeout, when `max_connections` are already out), then pops a
/// fresh-enough idle connection or dials. Every checked-out connection is
/// eventually accounted for by `put_back` or `discard`.
struct Server {
    addrs: Vec<SocketAddr>,
    pool: Mutex<Pool>,
    /// Signaled whenever a checked-out connection is returned or dropped.
    available: Condvar,
}

impl Server {
    fn new(addrs: Vec<SocketAddr>) -> Server {
        Server {
            addrs,
            pool: Mutex::new(Pool {
                idle: Vec::new(),
                checked_out: 0,
            }),
            available: Condvar::new(),
        }
    }

    /// Check out a connection; the flag is true when it came from the pool
    /// rather than a fresh dial.
    fn checkout(&self, config: &Config) -> Result<(MetaConnection, bool), MemcacheError> {
        let cap = config.max_connections.unwrap_or(usize::MAX);
        let deadline = config.timeouts.connect.map(|timeout| Instant::now() + timeout);
        let mut pool = self.pool.lock().unwrap();
        while pool.checked_out >= cap {
            pool = match deadline {
                Some(deadline) => {
                    let Some(remaining) = deadline.checked_duration_since(Instant::now()) else {
                        return Err(pool_exhausted());
                    };
                    self.available.wait_timeout(pool, remaining).unwrap().0
                }
                None => self.available.wait(pool).unwrap(),
            };
        }
        pool.checked_out += 1;
        while let Some((connection, since)) = pool.idle.pop() {
            if config.idle_timeout.is_some_and(|limit| since.elapsed() >= limit) {
                // Too old to trust: an idle connection may have been torn
                // down by a middlebox or server restart.
                continue;
            }
            return Ok((connection, true));
        }
        drop(pool);
        match self.dial(&config.timeouts) {
            Ok(connection) => Ok((connection, false)),
            Err(error) => {
                self.discard();
                Err(error)
            }
        }
    }

    fn dial(&self, timeouts: &Timeouts) -> Result<MetaConnection, MemcacheError> {
        let stream = match timeouts.connect {
            Some(duration) => {
                let mut last_error = None;
                let mut connected = None;
                for addr in &self.addrs {
                    match TcpStream::connect_timeout(addr, duration) {
                        Ok(stream) => {
                            connected = Some(stream);
                            break;
                        }
                        Err(error) => last_error = Some(error),
                    }
                }
                // `addrs` is never empty, so a miss always has an error.
                connected.ok_or_else(|| last_error.unwrap())?
            }
            None => TcpStream::connect(self.addrs.as_slice())?,
        };
        stream.set_nodelay(true)?;
        let connection = MetaConnection::from_stream(stream);
        // The configuration is immutable, so the socket timeouts are set
        // once per connection.
        connection.set_io_timeout(timeouts.io)?;
        Ok(connection)
    }

    fn put_back(&self, connection: MetaConnection, config: &Config) {
        let mut pool = self.pool.lock().unwrap();
        if pool.idle.len() < config.max_idle {
            pool.idle.push((connection, Instant::now()));
        }
        pool.checked_out -= 1;
        drop(pool);
        self.available.notify_one();
    }

    /// Account for a checked-out connection that was dropped instead of
    /// returned.
    fn discard(&self) {
        self.pool.lock().unwrap().checked_out -= 1;
        self.available.notify_one();
    }
}

/// Cache operations normally complete in milliseconds, so one second is
/// already a generous bound; a hung cache server should fail fast rather
/// than stall its callers.
pub(crate) const DEFAULT_TIMEOUT: Duration = Duration::from_secs(1);

#[derive(Clone, Copy)]
pub(crate) struct Timeouts {
    pub(crate) connect: Option<Duration>,
    pub(crate) io: Option<Duration>,
}

impl Default for Timeouts {
    fn default() -> Timeouts {
        Timeouts {
            connect: Some(DEFAULT_TIMEOUT),
            io: Some(DEFAULT_TIMEOUT),
        }
    }
}

/// The full client configuration, frozen at connect time.
#[derive(Clone, Copy)]
pub(crate) struct Config {
    pub(crate) hash_function: fn(&[u8]) -> u64,
    pub(crate) max_idle: usize,
    pub(crate) max_connections: Option<usize>,
    pub(crate) idle_timeout: Option<Duration>,
    pub(crate) timeouts: Timeouts,
}

impl Default for Config {
    fn default() -> Config {
        Config {
            hash_function: default_hash_function,
            max_idle: DEFAULT_MAX_IDLE,
            max_connections: Some(DEFAULT_MAX_CONNECTIONS),
            idle_timeout: Some(DEFAULT_IDLE_TIMEOUT),
            timeouts: Timeouts::default(),
        }
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
    pub(crate) config: Config,
}

impl MetaClientBuilder {
    pub fn new() -> MetaClientBuilder {
        MetaClientBuilder {
            config: Config::default(),
        }
    }

    /// Replace the function that hashes keys; the server is then picked by
    /// jump consistent hash over that value. The default hashes with
    /// [`DefaultHasher`].
    pub fn hash_function(mut self, hash_function: fn(&[u8]) -> u64) -> MetaClientBuilder {
        self.config.hash_function = hash_function;
        self
    }

    /// Cap how many idle connections each server retains (default 8).
    /// Concurrency above the cap dials extra connections, which are dropped
    /// when returned.
    pub fn max_idle(mut self, max_idle: usize) -> MetaClientBuilder {
        self.config.max_idle = max_idle;
        self
    }

    /// Cap how many connections per server may be in flight at once
    /// (default 128; `None` removes the limit). At the cap an operation
    /// waits, within the connect timeout, for a connection to come free
    /// instead of dialing yet another one; this bounds the connection
    /// storm a server latency spike can otherwise cause. Idle connections
    /// are capped separately by [`max_idle`](Self::max_idle).
    pub fn max_connections(mut self, max_connections: Option<usize>) -> MetaClientBuilder {
        self.config.max_connections = max_connections;
        self
    }

    /// Drop pooled connections that have been idle for longer than this at
    /// checkout (default 60 seconds; `None` keeps them indefinitely).
    /// Middleboxes (NAT gateways, load balancers) and server restarts kill
    /// idle connections silently; an age cap discards them before they
    /// surface as errors.
    pub fn idle_timeout(mut self, idle_timeout: Option<Duration>) -> MetaClientBuilder {
        self.config.idle_timeout = idle_timeout;
        self
    }

    /// Limit how long dialing a server may take (default 1 second; `None`
    /// removes the limit).
    pub fn connect_timeout(mut self, timeout: Option<Duration>) -> MetaClientBuilder {
        self.config.timeouts.connect = timeout;
        self
    }

    /// Limit I/O waiting (default 1 second; `None` removes the limit). The
    /// blocking client applies it to each socket read or write; the tokio
    /// client applies it to a whole command or batch exchange. A timeout
    /// poisons the connection like any other transport error.
    pub fn io_timeout(mut self, timeout: Option<Duration>) -> MetaClientBuilder {
        self.config.timeouts.io = timeout;
        self
    }

    /// Connect to one server with this configuration.
    pub fn connect<A: ToSocketAddrs>(self, addr: A) -> Result<MetaClient, MemcacheError> {
        self.connect_multiple([addr])
    }

    /// Connect to several servers with this configuration; keys are
    /// distributed across them by the hash function. Addresses are resolved
    /// here, but connections are dialed lazily, so a down server surfaces
    /// at the first operation.
    pub fn connect_multiple<A: ToSocketAddrs>(
        self,
        addrs: impl IntoIterator<Item = A>,
    ) -> Result<MetaClient, MemcacheError> {
        let mut servers = Vec::new();
        for addr in addrs {
            servers.push(Server::new(resolve(addr)?));
        }
        if servers.is_empty() {
            return Err(ClientError::Error(Cow::Borrowed("at least one server address is required")).into());
        }
        Ok(MetaClient {
            servers: Arc::new(servers),
            config: self.config,
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
/// [`send`](Request::send). With multiple servers, keys are routed by jump
/// consistent hash over a pluggable key hash and batches are split per
/// server.
///
/// The client is cheap to clone and shareable across threads; clones share
/// the connection pools and configuration. Hashing, pooling and timeouts
/// are set on [`MetaClientBuilder`] before connecting and stay fixed for
/// the client's lifetime, so clones cannot diverge.
///
/// Each server keeps a stack of idle connections (bounded by
/// [`max_idle`](MetaClientBuilder::max_idle)) and dials extra ones under
/// concurrency, up to [`max_connections`](MetaClientBuilder::max_connections);
/// at that cap an operation waits for a connection to come free. Idle
/// connections older than [`idle_timeout`](MetaClientBuilder::idle_timeout)
/// are discarded at checkout, and a pooled connection that turns out dead
/// at the first write is replaced by a fresh dial once, transparently. A
/// connection that fails mid-exchange is dropped instead of being reused,
/// and the next operation dials a fresh one.
///
/// ```no_run
/// # use memcache::exp::MetaClient;
/// let client = MetaClient::connect("127.0.0.1:11211").unwrap();
/// client.set("foo", "bar").ttl(60).send().unwrap();
/// let result = client.get("foo").send().unwrap();
/// ```
#[derive(Clone)]
pub struct MetaClient {
    servers: Arc<Vec<Server>>,
    config: Config,
}

impl MetaClient {
    /// Connect to one server with the default configuration; use
    /// [`builder`](Self::builder) to change it.
    pub fn connect<A: ToSocketAddrs>(addr: A) -> Result<MetaClient, MemcacheError> {
        MetaClient::connect_multiple([addr])
    }

    /// Connect to several servers with the default configuration; keys are
    /// distributed across them by the hash function. Addresses are resolved
    /// here, but connections are dialed lazily, so a down server surfaces
    /// at the first operation.
    pub fn connect_multiple<A: ToSocketAddrs>(addrs: impl IntoIterator<Item = A>) -> Result<MetaClient, MemcacheError> {
        MetaClient::builder().connect_multiple(addrs)
    }

    /// Start a [`MetaClientBuilder`] to configure hashing, pooling and
    /// timeouts before connecting.
    pub fn builder() -> MetaClientBuilder {
        MetaClientBuilder::new()
    }

    fn connection_index(&self, key: &[u8]) -> usize {
        jump_hash((self.config.hash_function)(key), self.servers.len())
    }

    /// Check out a connection, write the payload, read `responses` framed
    /// responses and return the connection to the pool.
    ///
    /// A pooled connection that fails the write with a disconnect error
    /// was torn down while idle; the server cannot have executed anything,
    /// so the exchange retries once on a freshly dialed connection. Any
    /// other failure leaves the stream in an unknown state and drops the
    /// connection instead of returning it.
    fn exchange(&self, server: usize, payload: &[u8], responses: usize) -> Result<Vec<MetaResponse>, MemcacheError> {
        let server = &self.servers[server];
        let (mut connection, reused) = server.checkout(&self.config)?;
        if let Err(error) = connection.write_payload(payload) {
            if !(reused && is_disconnect(&error)) {
                server.discard();
                return Err(error);
            }
            // The dead connection's live slot carries over to the redial.
            drop(connection);
            connection = match server.dial(&self.config.timeouts) {
                Ok(connection) => connection,
                Err(error) => {
                    server.discard();
                    return Err(error);
                }
            };
            if let Err(error) = connection.write_payload(payload) {
                server.discard();
                return Err(error);
            }
        }
        let mut collected = Vec::with_capacity(responses);
        for _ in 0..responses {
            match connection.receive() {
                Ok(response) => collected.push(response),
                Err(error) => {
                    server.discard();
                    return Err(error);
                }
            }
        }
        server.put_back(connection, &self.config);
        Ok(collected)
    }

    /// Read a key.
    pub fn get(&self, key: impl Into<Vec<u8>>) -> Request<'_, MetaClient, Get> {
        Request::new(self, Get::new(key))
    }

    /// Store a value under a key; the value is encoded via
    /// [`ToValue`](super::ToValue).
    pub fn set(&self, key: impl Into<Vec<u8>>, value: impl ToValue) -> Request<'_, MetaClient, Set> {
        Request::new(self, Set::new(key, value))
    }

    /// Delete a key.
    pub fn delete(&self, key: impl Into<Vec<u8>>) -> Request<'_, MetaClient, Delete> {
        Request::new(self, Delete::new(key))
    }

    /// Increment a counter (delta defaults to 1).
    pub fn increment(&self, key: impl Into<Vec<u8>>) -> Request<'_, MetaClient, Arithmetic> {
        Request::new(self, Arithmetic::new(key))
    }

    /// Decrement a counter (delta defaults to 1); saturates at zero.
    pub fn decrement(&self, key: impl Into<Vec<u8>>) -> Request<'_, MetaClient, Arithmetic> {
        let operation = Arithmetic {
            mode: ArithmeticMode::Decrement,
            ..Arithmetic::new(key)
        };
        Request::new(self, operation)
    }

    /// Run a standalone operation value; [`send`](Request::send) is sugar
    /// for this.
    pub fn run<O: Operation>(&self, operation: O) -> Result<O::Output, MemcacheError> {
        let command = operation.prepare()?;
        let payload = command.encode()?;
        let index = self.connection_index(operation.key());
        let response = self
            .exchange(index, &payload, 1)?
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
    ///
    /// ```no_run
    /// # use memcache::exp::{Get, MetaClient, Set};
    /// # let client = MetaClient::connect("127.0.0.1:11211").unwrap();
    /// let results = client.run_batch(vec![
    ///     Set::new("foo", "bar").ttl(60).into(),
    ///     Get::new("baz").into(),
    /// ]).unwrap();
    /// ```
    pub fn run_batch(&self, operations: impl IntoIterator<Item = Op>) -> Result<Vec<OpResult>, MemcacheError> {
        let operations: Vec<Op> = operations.into_iter().collect();
        self.run_all(&operations)
    }

    fn run_all<O: Operation>(&self, operations: &[O]) -> Result<Vec<O::Output>, MemcacheError> {
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
            let responses = self.exchange(server, &payload, commands.len())?;
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
    pub fn noop(&self) -> Result<(), MemcacheError> {
        let payload = build_noop().encode()?;
        for server in 0..self.servers.len() {
            let response = self
                .exchange(server, &payload, 1)?
                .pop()
                .expect("exchange returned no response");
            if response.rc != ReturnCode::Mn {
                return Err(ServerError::BadResponse("unexpected no-op response".into()).into());
            }
        }
        Ok(())
    }

    /// Fetch `me` debug fields for a key; `None` on a miss.
    pub fn debug(&self, key: impl Into<Vec<u8>>) -> Result<Option<HashMap<String, String>>, MemcacheError> {
        let key = key.into();
        let index = self.connection_index(&key);
        let payload = build_debug(key)?.encode()?;
        let response = self
            .exchange(index, &payload, 1)?
            .pop()
            .expect("exchange returned no response");
        parse_debug_result(&response)
    }
}

impl<'a, O: Operation> Request<'a, MetaClient, O> {
    /// Execute the request and return its typed result.
    pub fn send(self) -> Result<O::Output, MemcacheError> {
        let Request { client, operation } = self;
        client.run(operation)
    }
}

#[cfg(test)]
mod tests {
    use std::io::{BufRead, BufReader, Read, Write};
    use std::net::TcpListener;
    use std::thread::JoinHandle;

    use super::super::result::{GetStatus, MutationStatus};
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
    /// chosen server; `char_for` finds a leading character that jump-hashes
    /// to the wanted bucket under two servers.
    fn first_byte(key: &[u8]) -> u64 {
        key[0] as u64
    }

    fn char_for(bucket: usize) -> char {
        (b'0'..=b'z').find(|&byte| jump_hash(byte as u64, 2) == bucket).unwrap() as char
    }

    #[test]
    fn jump_hash_properties() {
        for key in 0..1000u64 {
            assert_eq!(jump_hash(key, 1), 0);
            // Growing n -> n+1 either keeps a key in place or moves it to
            // the new bucket; nothing else may change.
            for buckets in 1..10 {
                let before = jump_hash(key, buckets);
                let after = jump_hash(key, buckets + 1);
                assert!(after == before || after == buckets);
            }
        }

        let mut counts = [0usize; 4];
        for key in 0..4000u64 {
            counts[jump_hash(default_hash_function(&key.to_le_bytes()), 4)] += 1;
        }
        for &count in &counts {
            assert!(count > 700, "unbalanced buckets: {:?}", counts);
        }
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
        assert_eq!(stored.status, MutationStatus::Stored);

        let fetched = client.get("foo").send().unwrap();
        assert_eq!(fetched.status, GetStatus::Hit);
        assert_eq!(fetched.value.as_deref(), Some(&b"bar"[..]));

        let added = client.set("foo", "baz").add().send().unwrap();
        assert_eq!(added.status, MutationStatus::AlreadyExists);

        let counter = client.increment("counter").delta(2).send().unwrap();
        assert_eq!(counter.value, Some(42));

        let deleted = client.delete("foo").send().unwrap();
        assert!(deleted.stored());

        client.noop().unwrap();

        let requests = server.join().unwrap();
        assert_eq!(requests[0], b"ms foo 3 F16\r\n".to_vec());
        assert_eq!(requests[1], b"mg foo v f\r\n".to_vec());
        assert_eq!(requests[2], b"ms foo 3 ME F16\r\n".to_vec());
        assert_eq!(requests[3], b"ma counter v D2\r\n".to_vec());
        assert_eq!(requests[4], b"md foo\r\n".to_vec());
        assert_eq!(requests[5], b"mn\r\n".to_vec());
    }

    #[test]
    fn run_batch_mixed_operations() {
        let (addr, server) = scripted_server(vec![b"HD\r\n", b"VA 1 f0\r\n1\r\n", b"NF\r\n"]);
        let client = MetaClient::connect(addr).unwrap();

        let results = client
            .run_batch(vec![
                Set::new("a", "1").ttl(60).into(),
                Get::new("a").into(),
                Delete::new("c").into(),
            ])
            .unwrap();
        assert_eq!(results.len(), 3);
        assert!(results[0].as_mutation().unwrap().stored());
        assert_eq!(results[1].as_get().unwrap().value.as_deref(), Some(&b"1"[..]));
        assert_eq!(results[2].as_mutation().unwrap().status, MutationStatus::NotFound);

        // All three commands were written before the first response was read.
        let requests = server.join().unwrap();
        assert_eq!(requests[0], b"ms a 1 F16 T60\r\n".to_vec());
        assert_eq!(requests[1], b"mg a v f\r\n".to_vec());
        assert_eq!(requests[2], b"md c\r\n".to_vec());
    }

    #[test]
    fn run_batch_validates_before_writing() {
        let (addr, server) = scripted_server(vec![b"MN\r\n"]);
        let client = MetaClient::connect(addr).unwrap();

        // The second operation is invalid; nothing must reach the server.
        let error = client.run_batch(vec![Set::new("a", "1").into(), Delete::new("b").stale_for(30).into()]);
        assert!(error.is_err());

        client.noop().unwrap();
        let requests = server.join().unwrap();
        assert_eq!(requests, vec![b"mn\r\n".to_vec()]);
    }

    #[test]
    fn run_executes_standalone_operations() {
        let (addr, server) = scripted_server(vec![b"HD\r\n", b"VA 1\r\n1\r\n"]);
        let client = MetaClient::connect(addr).unwrap();

        let operation = client.set("foo", "bar").ttl(60).into_operation();
        assert!(client.run(operation).unwrap().stored());

        let decremented = client.decrement("counter").send().unwrap();
        assert_eq!(decremented.value, Some(1));

        let requests = server.join().unwrap();
        assert_eq!(requests[0], b"ms foo 3 F16 T60\r\n".to_vec());
        assert_eq!(requests[1], b"ma counter MD v D1\r\n".to_vec());
    }

    #[test]
    fn multi_server_routes_by_key() {
        let (addr0, server0) = scripted_server(vec![b"HD\r\n", b"MN\r\n"]);
        let (addr1, server1) = scripted_server(vec![b"VA 1 f0\r\nx\r\n", b"MN\r\n"]);
        let client = MetaClient::builder()
            .hash_function(first_byte)
            .connect_multiple([addr0, addr1])
            .unwrap();
        let key0 = format!("{}a", char_for(0));
        let key1 = format!("{}b", char_for(1));

        assert!(client.set(&*key0, "v").send().unwrap().stored());
        assert!(client.get(&*key1).send().unwrap().hit());
        client.noop().unwrap();

        assert_eq!(
            server0.join().unwrap(),
            vec![format!("ms {} 1 F16\r\n", key0).into_bytes(), b"mn\r\n".to_vec()]
        );
        assert_eq!(
            server1.join().unwrap(),
            vec![format!("mg {} v f\r\n", key1).into_bytes(), b"mn\r\n".to_vec()]
        );
    }

    #[test]
    fn multi_server_batch_splits_and_reorders() {
        let (addr0, server0) = scripted_server(vec![b"EN\r\n"]);
        let (addr1, server1) = scripted_server(vec![b"HD\r\n", b"NF\r\n"]);
        let client = MetaClient::builder()
            .hash_function(first_byte)
            .connect_multiple([addr0, addr1])
            .unwrap();
        let key_set = format!("{}a", char_for(1));
        let key_get = format!("{}b", char_for(0));
        let key_delete = format!("{}c", char_for(1));

        // Interleaved across servers; results must come back in input order.
        let results = client
            .run_batch(vec![
                Set::new(&*key_set, "v").into(),
                Get::new(&*key_get).into(),
                Delete::new(&*key_delete).into(),
            ])
            .unwrap();
        assert!(results[0].as_mutation().unwrap().stored());
        assert_eq!(results[1].as_get().unwrap().status, GetStatus::Miss);
        assert_eq!(results[2].as_mutation().unwrap().status, MutationStatus::NotFound);

        assert_eq!(
            server0.join().unwrap(),
            vec![format!("mg {} v f\r\n", key_get).into_bytes()]
        );
        assert_eq!(
            server1.join().unwrap(),
            vec![
                format!("ms {} 1 F16\r\n", key_set).into_bytes(),
                format!("md {}\r\n", key_delete).into_bytes(),
            ]
        );
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
        assert!(client.delete("foo").send().is_err());
        assert!(start.elapsed() < Duration::from_secs(5));
        assert!(client.delete("foo").send().unwrap().stored());
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
        assert!(client.delete("foo").send().unwrap().stored());
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
        assert!(client.delete("foo").send().unwrap().stored());
        server.join().unwrap();
    }

    /// A listener that accepts any number of connections, counts them and
    /// answers every request line with `HD`.
    fn counting_server() -> (SocketAddr, Arc<std::sync::atomic::AtomicUsize>) {
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
        (addr, accepted)
    }

    #[test]
    fn max_connections_bounds_concurrent_dials() {
        let (addr, accepted) = counting_server();
        let client = MetaClient::builder().max_connections(Some(2)).connect(addr).unwrap();
        std::thread::scope(|scope| {
            for _ in 0..4 {
                let client = client.clone();
                scope.spawn(move || {
                    for _ in 0..5 {
                        assert!(client.delete("foo").send().unwrap().stored());
                    }
                });
            }
        });
        assert!(accepted.load(std::sync::atomic::Ordering::SeqCst) <= 2);
    }

    #[test]
    fn pool_exhaustion_fails_fast() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let handle = std::thread::spawn(move || {
            // Accept one connection and go silent, so its holder blocks on
            // the read until the io timeout fires.
            let (stream, _) = listener.accept().unwrap();
            let mut reader = BufReader::new(stream);
            let mut line = Vec::new();
            let _ = reader.read_until(b'\n', &mut line);
            // Keep the connection (and the listener) open until the
            // holder gives up and drops its end.
            line.clear();
            let _ = reader.read_until(b'\n', &mut line);
        });

        let client = MetaClient::builder()
            .max_connections(Some(1))
            .connect_timeout(Some(Duration::from_millis(100)))
            .io_timeout(Some(Duration::from_millis(500)))
            .connect(addr)
            .unwrap();
        std::thread::scope(|scope| {
            let holder = scope.spawn(|| {
                // Occupies the only slot until its io timeout.
                assert!(client.delete("a").send().is_err());
            });
            std::thread::sleep(Duration::from_millis(100));
            let start = std::time::Instant::now();
            let error = client.delete("b").send().unwrap_err();
            // Waited out the connect timeout, not the holder's io timeout.
            assert!(start.elapsed() < Duration::from_millis(400));
            assert!(matches!(error, MemcacheError::ClientError(_)), "{:?}", error);
            holder.join().unwrap();
        });
        handle.join().unwrap();
    }

    #[test]
    fn idle_timeout_zero_never_reuses() {
        let (addr, accepted) = counting_server();
        let client = MetaClient::builder()
            .idle_timeout(Some(Duration::ZERO))
            .connect(addr)
            .unwrap();
        assert!(client.delete("foo").send().unwrap().stored());
        assert!(client.delete("foo").send().unwrap().stored());
        assert_eq!(accepted.load(std::sync::atomic::Ordering::SeqCst), 2);
    }

    #[test]
    fn dead_pooled_connection_is_redialed() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let handle = std::thread::spawn(move || {
            // First connection: answer one request, then close, leaving a
            // dead connection in the pool.
            {
                let (stream, _) = listener.accept().unwrap();
                let mut reader = BufReader::new(stream);
                let mut line = Vec::new();
                reader.read_until(b'\n', &mut line).unwrap();
                reader.get_mut().write_all(b"HD\r\n").unwrap();
            }
            // The big write must fail on the dead connection and be
            // retried here, on a fresh one.
            let (stream, _) = listener.accept().unwrap();
            let mut reader = BufReader::new(stream);
            let mut header = Vec::new();
            reader.read_until(b'\n', &mut header).unwrap();
            let line = String::from_utf8(header).unwrap();
            let datalen: usize = line.split_whitespace().nth(2).unwrap().parse().unwrap();
            let mut value = vec![0u8; datalen + 2];
            reader.read_exact(&mut value).unwrap();
            reader.get_mut().write_all(b"HD\r\n").unwrap();
        });

        let client = MetaClient::connect(addr).unwrap();
        assert!(client.delete("foo").send().unwrap().stored());
        // Give the server's FIN time to arrive.
        std::thread::sleep(Duration::from_millis(50));
        // A value larger than the socket buffers guarantees the write
        // itself fails (EPIPE/ECONNRESET) rather than the read after it.
        let big = vec![b'x'; 8 * 1024 * 1024];
        assert!(client.set("foo", &big[..]).send().unwrap().stored());
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
        assert!(client.delete("foo").send().unwrap().stored());
        assert!(client.delete("foo").send().unwrap().stored());
        handle.join().unwrap();
    }
}
