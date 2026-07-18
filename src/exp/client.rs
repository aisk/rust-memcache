//! Blocking single-server client over the semantic layer.

use std::collections::HashMap;
use std::net::ToSocketAddrs;

use crate::error::{MemcacheError, ServerError};

use super::connection::MetaConnection;
use super::core;
use super::meta_api::{MetaCommandResult, build_debug, build_noop, parse_debug_result, parse_meta_result};
use super::meta_command::{MetaCommand, ReturnCode};
use super::operation::{Arithmetic, Delete, Get, Set};
use super::result::{ArithmeticResult, GetResult, MutationResult};

/// A blocking meta protocol client for a single server.
///
/// Operations go in, typed results come out; values are raw bytes.
/// Serialization, multi-server routing and pipelining are not implemented
/// yet.
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

    fn run(&mut self, command: &MetaCommand) -> Result<MetaCommandResult, MemcacheError> {
        parse_meta_result(self.connection.execute(command)?)
    }

    pub fn get(&mut self, operation: &Get) -> Result<GetResult, MemcacheError> {
        let command = core::prepare_get(operation)?;
        core::parse_get(operation, self.run(&command)?)
    }

    pub fn set(&mut self, operation: &Set) -> Result<MutationResult, MemcacheError> {
        let command = core::prepare_set(operation)?;
        core::parse_set(operation, self.run(&command)?)
    }

    pub fn delete(&mut self, operation: &Delete) -> Result<MutationResult, MemcacheError> {
        let command = core::prepare_delete(operation)?;
        core::parse_delete(operation, self.run(&command)?)
    }

    pub fn arithmetic(&mut self, operation: &Arithmetic) -> Result<ArithmeticResult, MemcacheError> {
        let command = core::prepare_arithmetic(operation)?;
        core::parse_arithmetic(operation, self.run(&command)?)
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

#[cfg(test)]
mod tests {
    use std::io::{BufRead, BufReader, Read, Write};
    use std::net::{SocketAddr, TcpListener};
    use std::thread::JoinHandle;

    use super::super::meta_api::SetMode;
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

        let stored = client.set(&Set::new("foo", "bar")).unwrap();
        assert_eq!(stored.status, MutationStatus::Stored);

        let fetched = client.get(&Get::new("foo")).unwrap();
        assert_eq!(fetched.status, GetStatus::Hit);
        assert_eq!(fetched.value.as_deref(), Some(&b"bar"[..]));

        let added = client
            .set(&Set {
                mode: SetMode::Add,
                ..Set::new("foo", "baz")
            })
            .unwrap();
        assert_eq!(added.status, MutationStatus::AlreadyExists);

        let counter = client.arithmetic(&Arithmetic::new("counter")).unwrap();
        assert_eq!(counter.value, Some(42));

        let deleted = client.delete(&Delete::new("foo")).unwrap();
        assert!(deleted.stored());

        client.noop().unwrap();

        let requests = server.join().unwrap();
        assert_eq!(requests[0], b"ms foo 3\r\n".to_vec());
        assert_eq!(requests[1], b"mg foo v f\r\n".to_vec());
        assert_eq!(requests[2], b"ms foo 3 ME\r\n".to_vec());
        assert_eq!(requests[3], b"ma counter v D1\r\n".to_vec());
        assert_eq!(requests[4], b"md foo\r\n".to_vec());
        assert_eq!(requests[5], b"mn\r\n".to_vec());
    }
}
