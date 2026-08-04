//! Blocking client over the semantic layer.

use std::borrow::Cow;
use std::collections::HashMap;
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::net::{SocketAddr, TcpStream, ToSocketAddrs};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crate::error::{ClientError, MemcacheError, ServerError};

use super::connection::MetaConnection;
use super::core::{self, Operation};
use super::meta_api::{ArithmeticMode, build_debug, build_noop, parse_debug_result, parse_meta_result};
use super::meta_command::{MetaCommand, ReturnCode};
use super::operation::{Arithmetic, Delete, Get, Op, Set};
use super::request::Request;
use super::result::OpResult;
use super::value::ToValue;

pub(crate) const DEFAULT_MAX_IDLE: usize = 8;

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

/// One server: its resolved addresses and a stack of idle connections.
///
/// Checkout pops an idle connection or dials a new one; there is no cap on
/// concurrent connections, only on how many idle ones are retained.
struct Server {
    addrs: Vec<SocketAddr>,
    idle: Mutex<Vec<MetaConnection>>,
}

impl Server {
    fn checkout(&self, timeouts: &Timeouts) -> Result<MetaConnection, MemcacheError> {
        // Idle connections may have been closed by the server or a
        // middlebox while pooled; probe and discard instead of handing a
        // dead connection to the caller.
        loop {
            let Some(mut connection) = self.idle.lock().unwrap().pop() else {
                break;
            };
            if connection.is_reusable() {
                return Ok(connection);
            }
        }
        self.dial(timeouts)
    }

    fn dial(&self, timeouts: &Timeouts) -> Result<MetaConnection, MemcacheError> {
        let stream = match timeouts.connect {
            Some(duration) => {
                // One deadline shared by all resolved addresses, matching
                // the tokio client's behavior.
                let deadline = Instant::now() + duration;
                let mut last_error = None;
                let mut connected = None;
                for addr in &self.addrs {
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
                connected.ok_or_else(|| last_error.unwrap())?
            }
            None => TcpStream::connect(self.addrs.as_slice())?,
        };
        stream.set_nodelay(true)?;
        let mut connection = MetaConnection::from_stream(stream);
        connection.set_io_timeout(timeouts.io);
        Ok(connection)
    }

    fn put_back(&self, connection: MetaConnection, max_idle: usize) {
        let mut idle = self.idle.lock().unwrap();
        if idle.len() < max_idle {
            idle.push(connection);
        }
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
    pub(crate) hash_function: fn(&[u8]) -> u64,
    pub(crate) max_idle: usize,
    pub(crate) timeouts: Timeouts,
}

impl MetaClientBuilder {
    pub fn new() -> MetaClientBuilder {
        MetaClientBuilder {
            hash_function: default_hash_function,
            max_idle: DEFAULT_MAX_IDLE,
            timeouts: Timeouts::default(),
        }
    }

    /// Replace the function that hashes keys; the server is then picked by
    /// jump consistent hash over that value. The default hashes with
    /// [`DefaultHasher`].
    pub fn hash_function(mut self, hash_function: fn(&[u8]) -> u64) -> MetaClientBuilder {
        self.hash_function = hash_function;
        self
    }

    /// Cap how many idle connections each server retains (default 8).
    /// Concurrency above the cap dials extra connections, which are dropped
    /// when returned.
    pub fn max_idle(mut self, max_idle: usize) -> MetaClientBuilder {
        self.max_idle = max_idle;
        self
    }

    /// Limit how long dialing a server may take (default 1 second; `None`
    /// removes the limit).
    pub fn connect_timeout(mut self, timeout: Option<Duration>) -> MetaClientBuilder {
        self.timeouts.connect = timeout;
        self
    }

    /// Limit how long an exchange may take (default 1 second; `None`
    /// removes the limit). Both clients apply it to a whole command or
    /// batch exchange - request write plus response reads - not to
    /// individual socket operations. A timeout poisons the connection like
    /// any other transport error.
    pub fn io_timeout(mut self, timeout: Option<Duration>) -> MetaClientBuilder {
        self.timeouts.io = timeout;
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
            servers.push(Server {
                addrs: resolve(addr)?,
                idle: Mutex::new(Vec::new()),
            });
        }
        if servers.is_empty() {
            return Err(ClientError::Error(Cow::Borrowed("at least one server address is required")).into());
        }
        Ok(MetaClient {
            servers: Arc::new(servers),
            hash_function: self.hash_function,
            max_idle: self.max_idle,
            timeouts: self.timeouts,
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
/// the client's lifetime, so clones cannot diverge. Each server keeps a
/// stack of idle connections (bounded by
/// [`max_idle`](MetaClientBuilder::max_idle)); a connection that fails
/// mid-exchange is dropped instead of being reused, and the next operation
/// dials a fresh one.
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
    hash_function: fn(&[u8]) -> u64,
    max_idle: usize,
    timeouts: Timeouts,
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
        jump_hash((self.hash_function)(key), self.servers.len())
    }

    /// Check out a connection, run one transport exchange on it and return
    /// it to the pool. A failed exchange leaves the stream in an unknown
    /// state, so the connection is dropped instead of returned.
    fn with_connection<T>(
        &self,
        server: usize,
        exchange: impl FnOnce(&mut MetaConnection) -> Result<T, MemcacheError>,
    ) -> Result<T, MemcacheError> {
        let server = &self.servers[server];
        let mut connection = server.checkout(&self.timeouts)?;
        let result = exchange(&mut connection);
        if result.is_ok() {
            server.put_back(connection, self.max_idle);
        }
        result
    }

    /// Read a key.
    pub fn get(&self, key: impl AsRef<[u8]>) -> Request<'_, MetaClient, Get> {
        Request::new(self, Get::new(key))
    }

    /// Store a value under a key. The value is encoded via
    /// [`ToValue`](super::ToValue), which also picks the stored client
    /// flags: [`FLAG_STR`](super::FLAG_STR) for strings,
    /// [`FLAG_INT`](super::FLAG_INT) for integers and
    /// [`FLAG_BYTES`](super::FLAG_BYTES) (zero) otherwise. Other clients
    /// may not share these conventions; override with
    /// [`client_flags`](Request::client_flags).
    pub fn set(&self, key: impl AsRef<[u8]>, value: impl ToValue) -> Request<'_, MetaClient, Set> {
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
    pub fn run<O: Operation>(&self, operation: O) -> Result<O::Output, MemcacheError> {
        let command = operation.prepare()?;
        let index = self.connection_index(operation.key());
        let response = self.with_connection(index, |connection| connection.execute(&command))?;
        operation.parse(parse_meta_result(response)?)
    }

    /// Run several operations, split per server and pipelined with one
    /// round trip per server.
    ///
    /// All operations are validated before anything is written; a validation
    /// failure is the outer error and guarantees nothing executed. After
    /// that, every operation gets its own entry in input order: a transport
    /// failure fails the operations of that server's group (their entries
    /// are `Err`, and whether they took effect on the server is unknown)
    /// while the remaining groups still execute. Semantic outcomes (miss,
    /// CAS mismatch, ...) are not errors; they show up inside [`OpResult`].
    /// A batch is not a transaction.
    ///
    /// ```no_run
    /// # use memcache::exp::{Get, MetaClient, Set};
    /// # let client = MetaClient::connect("127.0.0.1:11211").unwrap();
    /// let results = client.run_batch(vec![
    ///     Set::new("foo", "bar").ttl(60).into(),
    ///     Get::new("baz").into(),
    /// ]).unwrap();
    /// let stored = results[0].as_ref().unwrap();
    /// ```
    pub fn run_batch(
        &self,
        operations: impl IntoIterator<Item = Op>,
    ) -> Result<Vec<Result<OpResult, MemcacheError>>, MemcacheError> {
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
    ) -> Result<Vec<Result<O::Output, MemcacheError>>, MemcacheError> {
        let operations: Vec<O> = operations.into_iter().collect();
        self.run_all(&operations)
    }

    fn run_all<O: Operation>(&self, operations: &[O]) -> Result<Vec<Result<O::Output, MemcacheError>>, MemcacheError> {
        let mut plan = core::plan(operations, self.servers.len(), |key| self.connection_index(key))?;
        let mut outputs: Vec<Option<Result<O::Output, MemcacheError>>> = (0..operations.len()).map(|_| None).collect();
        for (server, indices) in plan.groups.iter().enumerate() {
            if indices.is_empty() {
                continue;
            }
            let commands: Vec<MetaCommand> = indices
                .iter()
                .map(|&index| plan.commands[index].take().unwrap())
                .collect();
            match self.with_connection(server, |connection| connection.execute_batch(&commands)) {
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
    pub fn noop(&self) -> Result<(), MemcacheError> {
        for server in 0..self.servers.len() {
            let response = self.with_connection(server, |connection| connection.execute(&build_noop()))?;
            if response.rc != ReturnCode::Mn {
                return Err(ServerError::BadResponse("unexpected no-op response".into()).into());
            }
        }
        Ok(())
    }

    /// Fetch `me` debug fields for a key; `None` on a miss.
    pub fn debug(&self, key: impl AsRef<[u8]>) -> Result<Option<HashMap<String, String>>, MemcacheError> {
        let key = key.as_ref().to_vec();
        let index = self.connection_index(&key);
        let command = build_debug(key)?;
        let response = self.with_connection(index, |connection| connection.execute(&command))?;
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

        let results: Vec<_> = client
            .run_batch(vec![
                Set::new("a", "1").ttl(60).into(),
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
        assert!(client.run(operation).unwrap().applied());

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

        assert!(client.set(&key0, "v").send().unwrap().applied());
        assert!(client.get(&key1).send().unwrap().hit());
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
    fn run_batch_continues_after_group_failure() {
        let (addr0, server0) = scripted_server(vec![b"VA 1 f0\r\nx\r\n"]);
        // A bound-then-dropped listener yields an address that refuses
        // connections, so the second server's group must fail in transport.
        let dead = TcpListener::bind("127.0.0.1:0").unwrap();
        let dead_addr = dead.local_addr().unwrap();
        drop(dead);
        let client = MetaClient::builder()
            .hash_function(first_byte)
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
        assert!(client.delete("foo").send().is_err());
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
}
