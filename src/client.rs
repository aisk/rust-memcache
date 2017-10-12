use std::io::{Read, Write};
use std::net::{TcpStream, ToSocketAddrs};
use byteorder::{WriteBytesExt, ReadBytesExt, BigEndian};
use connection::Connection;
use error::MemcacheError;
use value::{ToMemcacheValue, FromMemcacheValue};
use packet::{Opcode, PacketHeader, Magic, ResponseStatus, StoreExtras};

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
            ..Default::default()
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
            ..Default::default()
        };
        request_header.write(self.connections[0].reader.get_mut());
        let response_header = PacketHeader::read(self.connections[0].reader.get_mut())?;
        if response_header.vbucket_id_or_status != ResponseStatus::NoError as u16 {
            // TODO: throw error
        }
        return Ok(());
    }

    /// Get a key from memcached server.
    ///
    /// Example:
    ///
    /// ```rust
    /// let client = memcache::Client::new("localhost:12345").unwrap();
    /// client.get("foo").unwrap();
    /// ```
    pub fn get(mut self, key: &str) -> Result<(), MemcacheError> {
        let request_header = PacketHeader {
            magic: Magic::Request as u8,
            opcode: Opcode::Get as u8,
            key_length: key.len() as u16, // TODO: check key length
            total_body_length: key.len() as u32,
            ..Default::default()
        };
        request_header.write(self.connections[0].reader.get_mut());
        self.connections[0].reader.get_mut().write(key.as_bytes())?;
        let response_header = PacketHeader::read(self.connections[0].reader.get_mut())?;
        let mut result = String::new();
        self.connections[0]
            .reader
            .get_mut()
            .take(response_header.total_body_length.into())
            .read_to_string(&mut result)?;
        // TODO: handle error and return result
        return Ok(());
    }

    /// Set a key with associate value into memcached server.
    ///
    /// Example:
    ///
    /// ```rust
    /// let client = memcache::Client::new("localhost:12345").unwrap();
    /// client.set("foofoo", "barbarbarian").unwrap();
    /// ```
    pub fn set<V: ToMemcacheValue<TcpStream>>(
        mut self,
        key: &str,
        value: V,
    ) -> Result<(), MemcacheError> {
        let request_header = PacketHeader {
            magic: Magic::Request as u8,
            opcode: Opcode::Set as u8,
            key_length: key.len() as u16,  // TODO: check key length
            extras_length: 8,
            total_body_length: (8 + key.len() + value.get_length()) as u32,
            ..Default::default()
        };
        let extras = StoreExtras{ flags:0, expiration: 0 };
        request_header.write(self.connections[0].reader.get_mut());
        self.connections[0].reader.get_mut().write_u32::<BigEndian>(extras.flags)?;
        self.connections[0].reader.get_mut().write_u32::<BigEndian>(extras.expiration)?;
        self.connections[0].reader.get_mut().write(key.as_bytes())?;
        value.write_to(self.connections[0].reader.get_mut())?;
        let response_header = PacketHeader::read(self.connections[0].reader.get_mut())?;
        return Ok(());
    }
}
