use std::io::Read;
use std::net::{TcpStream, ToSocketAddrs};
use connection::Connection;
use error::MemcacheError;
use packet::{Opcode, PacketHeader, Magic, ResponseStatus};

pub struct Client {
    connections: Vec<Connection<TcpStream>>,
}

impl Client {
    pub fn new<A: ToSocketAddrs>(addr: A) -> Result<Self, MemcacheError> {
        let connection = Connection::connect(addr)?;
        return Ok(Client { connections: vec![connection] });
    }

    /// Get the memcached server version.
    ///
    /// Example:
    ///
    /// ```rust
    /// let client = memcache::Client::new("localhost:12345").unwrap();
    /// client.version().unwrap();
    /// ```
    pub fn version(mut self) -> Result<String, MemcacheError> {
        let request_header = PacketHeader {
            magic: Magic::Request as u8,
            opcode: Opcode::Version as u8,
            key_length: 0,
            extras_length: 0,
            data_type: 0,
            vbucket_id_or_status: 0,
            total_body_length: 0,
            opaque: 0,
            cas: 0,
        };
        request_header.write(self.connections[0].reader.get_mut());
        let response_header = PacketHeader::read(self.connections[0].reader.get_mut())?;
        if response_header.vbucket_id_or_status != ResponseStatus::NoError as u16 {
            // TODO: throw error
        }
        let mut version = String::new();
        self.connections[0]
            .reader
            .get_mut()
            .take(response_header.total_body_length.into())
            .read_to_string(&mut version)?;
        return Ok(version);
    }

    /// Flush all cache on memcached server.
    ///
    /// Example:
    ///
    /// ```rust
    /// let client = memcache::Client::new("localhost:12345").unwrap();
    /// client.flush().unwrap();
    /// ```
    pub fn flush(mut self) -> Result<(), MemcacheError> {
        let request_header = PacketHeader {
            magic: Magic::Request as u8,
            opcode: Opcode::Flush as u8,
            key_length: 0,
            extras_length: 0,
            data_type: 0,
            vbucket_id_or_status: 0,
            total_body_length: 0,
            opaque: 0,
            cas: 0,
        };
        request_header.write(self.connections[0].reader.get_mut());
        let response_header = PacketHeader::read(self.connections[0].reader.get_mut())?;
        if response_header.vbucket_id_or_status != ResponseStatus::NoError as u16 {
            // TODO: throw error
        }
        return Ok(());
    }
}
