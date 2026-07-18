//! Blocking single-server client over the semantic layer.

use std::collections::HashMap;
use std::net::ToSocketAddrs;

use crate::error::{MemcacheError, ServerError};

use super::connection::MetaConnection;
use super::core::Operation;
use super::meta_api::{ArithmeticMode, build_debug, build_noop, parse_debug_result, parse_meta_result};
use super::meta_command::ReturnCode;
use super::operation::{Arithmetic, Delete, Get, Set};
use super::request::Request;

/// A blocking meta protocol client for a single server.
///
/// The verbs return lazy [`Request`] builders; chain options and finish with
/// [`send`](Request::send). Values are raw bytes. Serialization,
/// multi-server routing and batching are not implemented yet.
///
/// ```no_run
/// # use memcache::exp::MetaClient;
/// let mut client = MetaClient::connect("127.0.0.1:11211").unwrap();
/// client.set("foo", "bar").ttl(60).send().unwrap();
/// let result = client.get("foo").send().unwrap();
/// ```
pub struct MetaClient {
    connection: MetaConnection,
}

impl MetaClient {
    pub fn connect<A: ToSocketAddrs>(addr: A) -> Result<MetaClient, MemcacheError> {
        Ok(MetaClient::from_connection(MetaConnection::connect(addr)?))
    }

    pub fn from_connection(connection: MetaConnection) -> MetaClient {
        MetaClient { connection }
    }

    /// Read a key.
    pub fn get(&mut self, key: impl Into<Vec<u8>>) -> Request<'_, MetaClient, Get> {
        Request::new(self, Get::new(key))
    }

    /// Store a value under a key.
    pub fn set(&mut self, key: impl Into<Vec<u8>>, value: impl Into<Vec<u8>>) -> Request<'_, MetaClient, Set> {
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
        let wire = parse_meta_result(self.connection.execute(&command)?)?;
        operation.parse(wire)
    }

    /// Round-trip an `mn` no-op; useful as a connection health check.
    pub fn noop(&mut self) -> Result<(), MemcacheError> {
        let response = self.connection.execute(&build_noop())?;
        if response.rc != ReturnCode::Mn {
            return Err(ServerError::BadResponse("unexpected no-op response".into()).into());
        }
        Ok(())
    }

    /// Fetch `me` debug fields for a key; `None` on a miss.
    pub fn debug(&mut self, key: impl Into<Vec<u8>>) -> Result<Option<HashMap<String, String>>, MemcacheError> {
        let response = self.connection.execute(&build_debug(key)?)?;
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
        assert_eq!(requests[0], b"ms foo 3\r\n".to_vec());
        assert_eq!(requests[1], b"mg foo v f\r\n".to_vec());
        assert_eq!(requests[2], b"ms foo 3 ME\r\n".to_vec());
        assert_eq!(requests[3], b"ma counter v D2\r\n".to_vec());
        assert_eq!(requests[4], b"md foo\r\n".to_vec());
        assert_eq!(requests[5], b"mn\r\n".to_vec());
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
        assert_eq!(requests[0], b"ms foo 3 T60\r\n".to_vec());
        assert_eq!(requests[1], b"ma counter MD v D1\r\n".to_vec());
    }
}
