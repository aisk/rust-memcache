//! Blocking client over the semantic layer.

use std::borrow::Cow;
use std::collections::HashMap;
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::net::ToSocketAddrs;

use crate::error::{ClientError, MemcacheError, ServerError};

use super::connection::MetaConnection;
use super::core::Operation;
use super::meta_api::{ArithmeticMode, build_debug, build_noop, parse_debug_result, parse_meta_result};
use super::meta_command::{MetaCommand, ReturnCode};
use super::operation::{Arithmetic, Delete, Get, Op, Set};
use super::request::Request;
use super::result::OpResult;
use super::value::ToValue;

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

/// A blocking meta protocol client.
///
/// The verbs return lazy [`Request`] builders; chain options and finish with
/// [`send`](Request::send). With multiple servers, keys are routed by a
/// pluggable hash function and batches are split per server.
///
/// ```no_run
/// # use memcache::exp::MetaClient;
/// let mut client = MetaClient::connect("127.0.0.1:11211").unwrap();
/// client.set("foo", "bar").ttl(60).send().unwrap();
/// let result = client.get("foo").send().unwrap();
/// ```
pub struct MetaClient {
    connections: Vec<MetaConnection>,
    hash_function: fn(&[u8]) -> u64,
}

impl MetaClient {
    pub fn connect<A: ToSocketAddrs>(addr: A) -> Result<MetaClient, MemcacheError> {
        Ok(MetaClient::from_connection(MetaConnection::connect(addr)?))
    }

    /// Connect to several servers; keys are distributed across them by the
    /// hash function.
    pub fn connect_multiple<A: ToSocketAddrs>(addrs: impl IntoIterator<Item = A>) -> Result<MetaClient, MemcacheError> {
        let mut connections = Vec::new();
        for addr in addrs {
            connections.push(MetaConnection::connect(addr)?);
        }
        if connections.is_empty() {
            return Err(ClientError::Error(Cow::Borrowed("at least one server address is required")).into());
        }
        Ok(MetaClient {
            connections,
            hash_function: default_hash_function,
        })
    }

    pub fn from_connection(connection: MetaConnection) -> MetaClient {
        MetaClient {
            connections: vec![connection],
            hash_function: default_hash_function,
        }
    }

    /// Replace the function that hashes keys; the server is then picked by
    /// jump consistent hash over that value. The default hashes with
    /// [`DefaultHasher`].
    pub fn with_hash_function(mut self, hash_function: fn(&[u8]) -> u64) -> MetaClient {
        self.hash_function = hash_function;
        self
    }

    fn connection_index(&self, key: &[u8]) -> usize {
        jump_hash((self.hash_function)(key), self.connections.len())
    }

    /// Read a key.
    pub fn get(&mut self, key: impl Into<Vec<u8>>) -> Request<'_, MetaClient, Get> {
        Request::new(self, Get::new(key))
    }

    /// Store a value under a key; the value is encoded via
    /// [`ToValue`](super::ToValue).
    pub fn set(&mut self, key: impl Into<Vec<u8>>, value: impl ToValue) -> Request<'_, MetaClient, Set> {
        Request::new(self, Set::new(key, value))
    }

    /// Delete a key.
    pub fn delete(&mut self, key: impl Into<Vec<u8>>) -> Request<'_, MetaClient, Delete> {
        Request::new(self, Delete::new(key))
    }

    /// Increment a counter (delta defaults to 1).
    pub fn increment(&mut self, key: impl Into<Vec<u8>>) -> Request<'_, MetaClient, Arithmetic> {
        Request::new(self, Arithmetic::new(key))
    }

    /// Decrement a counter (delta defaults to 1); saturates at zero.
    pub fn decrement(&mut self, key: impl Into<Vec<u8>>) -> Request<'_, MetaClient, Arithmetic> {
        let operation = Arithmetic {
            mode: ArithmeticMode::Decrement,
            ..Arithmetic::new(key)
        };
        Request::new(self, operation)
    }

    /// Run a standalone operation value; [`send`](Request::send) is sugar
    /// for this.
    pub fn run<O: Operation>(&mut self, operation: O) -> Result<O::Output, MemcacheError> {
        let command = operation.prepare()?;
        let index = self.connection_index(operation.key());
        let wire = parse_meta_result(self.connections[index].execute(&command)?)?;
        operation.parse(wire)
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
    /// # let mut client = MetaClient::connect("127.0.0.1:11211").unwrap();
    /// let results = client.run_batch(vec![
    ///     Set::new("foo", "bar").ttl(60).into(),
    ///     Get::new("baz").into(),
    /// ]).unwrap();
    /// ```
    pub fn run_batch(&mut self, operations: impl IntoIterator<Item = Op>) -> Result<Vec<OpResult>, MemcacheError> {
        let operations: Vec<Op> = operations.into_iter().collect();
        self.run_all(&operations)
    }

    fn run_all<O: Operation>(&mut self, operations: &[O]) -> Result<Vec<O::Output>, MemcacheError> {
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
            let responses = self.connections[server].execute_batch(&commands)?;
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
    pub fn noop(&mut self) -> Result<(), MemcacheError> {
        for connection in &mut self.connections {
            let response = connection.execute(&build_noop())?;
            if response.rc != ReturnCode::Mn {
                return Err(ServerError::BadResponse("unexpected no-op response".into()).into());
            }
        }
        Ok(())
    }

    /// Fetch `me` debug fields for a key; `None` on a miss.
    pub fn debug(&mut self, key: impl Into<Vec<u8>>) -> Result<Option<HashMap<String, String>>, MemcacheError> {
        let key = key.into();
        let index = self.connection_index(&key);
        let response = self.connections[index].execute(&build_debug(key)?)?;
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
    use std::net::{SocketAddr, TcpListener};
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
        let (addr, server) = scripted_server(vec![
            b"HD\r\n",
            b"VA 3 f0\r\nbar\r\n",
            b"NS\r\n",
            b"VA 2\r\n42\r\n",
            b"HD\r\n",
            b"MN\r\n",
        ]);
        let mut client = MetaClient::connect(addr).unwrap();

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
        let mut client = MetaClient::connect(addr).unwrap();

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
        let mut client = MetaClient::connect(addr).unwrap();

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
        let mut client = MetaClient::connect(addr).unwrap();

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
        let mut client = MetaClient::connect_multiple([addr0, addr1])
            .unwrap()
            .with_hash_function(first_byte);
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
        let mut client = MetaClient::connect_multiple([addr0, addr1])
            .unwrap()
            .with_hash_function(first_byte);
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
}
