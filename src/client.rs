use std::io::{Read, Write};
use std::net::{TcpStream, ToSocketAddrs};
use byteorder::{WriteBytesExt, ReadBytesExt, BigEndian};
use connection::Connection;
use error::MemcacheError;
use value::{ToMemcacheValue, FromMemcacheValue};
use packet::{Opcode, PacketHeader, Magic, ResponseStatus, StoreExtras};

#[derive(Debug)]
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
    /// let mut client = memcache::Client::new("localhost:12345").unwrap();
    /// client.version().unwrap();
    /// ```
    pub fn version(&mut self) -> Result<String, MemcacheError> {
        let request_header = PacketHeader {
            magic: Magic::Request as u8,
            opcode: Opcode::Version as u8,
            ..Default::default()
        };
        request_header.write(&mut self.connections[0].stream)?;
        let response_header = PacketHeader::read(&mut self.connections[0].stream)?;
        if response_header.vbucket_id_or_status != ResponseStatus::NoError as u16 {
            return Err(MemcacheError::from(response_header.vbucket_id_or_status));
        }
        let mut buffer = vec![0; response_header.total_body_length as usize];
        self.connections[0].stream.read_exact(buffer.as_mut_slice())?;
        return Ok(String::from_utf8(buffer)?);
    }

    /// Flush all cache on memcached server.
    ///
    /// Example:
    ///
    /// ```rust
    /// let mut client = memcache::Client::new("localhost:12345").unwrap();
    /// client.flush().unwrap();
    /// ```
    pub fn flush(&mut self) -> Result<(), MemcacheError> {
        let request_header = PacketHeader {
            magic: Magic::Request as u8,
            opcode: Opcode::Flush as u8,
            ..Default::default()
        };
        request_header.write(&mut self.connections[0].stream)?;
        let response_header = PacketHeader::read(&mut self.connections[0].stream)?;
        if response_header.vbucket_id_or_status != ResponseStatus::NoError as u16 {
            return Err(MemcacheError::from(response_header.vbucket_id_or_status));
        }
        return Ok(());
    }

    /// Get a key from memcached server.
    ///
    /// Example:
    ///
    /// ```rust
    /// let mut client = memcache::Client::new("localhost:12345").unwrap();
    /// let result: String = client.get("foo").unwrap();
    /// println!(">>> {}", result);
    /// ```
    pub fn get<V: FromMemcacheValue>(&mut self, key: &str) -> Result<V, MemcacheError> {
        let request_header = PacketHeader {
            magic: Magic::Request as u8,
            opcode: Opcode::Get as u8,
            key_length: key.len() as u16, // TODO: check key length
            total_body_length: key.len() as u32,
            ..Default::default()
        };
        request_header.write(&mut self.connections[0].stream)?;
        self.connections[0].stream.write(key.as_bytes())?;
        let response_header = PacketHeader::read(&mut self.connections[0].stream)?;
        let flags = self.connections[0].stream.read_u32::<BigEndian>()?;
        let value_length = response_header.total_body_length - 4; // 32bit for extras
        let mut buffer = vec![0; value_length as usize];
        self.connections[0].stream.read_exact(buffer.as_mut_slice())?;
        // TODO: change flags from u16 to u32
        return Ok(FromMemcacheValue::from_memcache_value(
            buffer,
            flags as u16,
        )?);
    }

    /// Set a key with associate value into memcached server.
    ///
    /// Example:
    ///
    /// ```rust
    /// let mut client = memcache::Client::new("localhost:12345").unwrap();
    /// client.set("foofoo", "barbarbarian").unwrap();
    /// ```
    pub fn set<V: ToMemcacheValue<TcpStream>>(
        &mut self,
        key: &str,
        value: V,
    ) -> Result<(), MemcacheError> {
        let request_header = PacketHeader {
            magic: Magic::Request as u8,
            opcode: Opcode::Set as u8,
            key_length: key.len() as u16, // TODO: check key length
            extras_length: 8,
            total_body_length: (8 + key.len() + value.get_length()) as u32,
            ..Default::default()
        };
        let extras = StoreExtras {
            flags: 0,
            expiration: 0,
        };
        request_header.write(&mut self.connections[0].stream)?;
        self.connections[0].stream.write_u32::<BigEndian>(
            extras.flags,
        )?;
        self.connections[0].stream.write_u32::<BigEndian>(
            extras.expiration,
        )?;
        self.connections[0].stream.write(key.as_bytes())?;
        value.write_to(&mut self.connections[0].stream)?;
        let response_header = PacketHeader::read(&mut self.connections[0].stream)?;
        if response_header.vbucket_id_or_status != ResponseStatus::NoError as u16 {
            return Err(MemcacheError::from(response_header.vbucket_id_or_status));
        }
        return Ok(());
    }
}
