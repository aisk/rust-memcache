use std::io::Write;
use std::net::{TcpStream, ToSocketAddrs};
use byteorder::{WriteBytesExt, BigEndian};
use connection::Connection;
use error::MemcacheError;
use value::{ToMemcacheValue, FromMemcacheValue};
use packet;
use packet::{Opcode, PacketHeader, Magic};

pub struct Client {
    connection: Connection,
}

impl Client {
    pub fn new<A: ToSocketAddrs>(addr: A) -> Result<Self, MemcacheError> {
        let connection = Connection::connect(addr)?;
        return Ok(Client { connection: connection });
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
        request_header.write(&mut self.connection.stream)?;
        return packet::parse_version_response(&mut self.connection.stream);
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
        request_header.write(&mut self.connection.stream)?;
        return packet::parse_header_only_response(&mut self.connection.stream);
    }

    /// Get a key from memcached server.
    ///
    /// Example:
    ///
    /// ```rust
    /// let mut client = memcache::Client::new("localhost:12345").unwrap();
    /// let _: Option<String> = client.get("foo").unwrap();
    /// ```
    pub fn get<V: FromMemcacheValue>(&mut self, key: &str) -> Result<Option<V>, MemcacheError> {
        let request_header = PacketHeader {
            magic: Magic::Request as u8,
            opcode: Opcode::Get as u8,
            key_length: key.len() as u16, // TODO: check key length
            total_body_length: key.len() as u32,
            ..Default::default()
        };
        request_header.write(&mut self.connection.stream)?;
        self.connection.stream.write(key.as_bytes())?;
        return packet::parse_get_response(&mut self.connection.stream);
    }

    /// Set a key with associate value into memcached server.
    ///
    /// Example:
    ///
    /// ```rust
    /// let mut client = memcache::Client::new("localhost:12345").unwrap();
    /// client.set("foo", "bar").unwrap();
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
        let extras = packet::StoreExtras {
            flags: value.get_flags(),
            expiration: 0,
        };
        request_header.write(&mut self.connection.stream)?;
        self.connection.stream.write_u32::<BigEndian>(extras.flags)?;
        self.connection.stream.write_u32::<BigEndian>(
            extras.expiration,
        )?;
        self.connection.stream.write(key.as_bytes())?;
        value.write_to(&mut self.connection.stream)?;
        return packet::parse_header_only_response(&mut self.connection.stream);
    }

    /// Delete a key from memcached server.
    ///
    /// Example:
    ///
    /// ```rust
    /// let mut client = memcache::Client::new("localhost:12345").unwrap();
    /// client.delete("foo").unwrap();
    /// ```
    pub fn delete(&mut self, key: &str) -> Result<bool, MemcacheError> {
        let request_header = PacketHeader {
            magic: Magic::Request as u8,
            opcode: Opcode::Delete as u8,
            key_length: key.len() as u16, // TODO: check key length
            total_body_length: key.len() as u32,
            ..Default::default()
        };
        request_header.write(&mut self.connection.stream)?;
        self.connection.stream.write(key.as_bytes())?;
        return packet::parse_delete_response(&mut self.connection.stream);
    }

    /// Increment the value with amount.
    ///
    /// Example:
    ///
    /// ```rust
    /// let mut client = memcache::Client::new("localhost:12345").unwrap();
    /// client.increment("counter", 42).unwrap();
    /// ```
    pub fn increment(&mut self, key: &str, amount: u64) -> Result<u64, MemcacheError> {
        let request_header = PacketHeader {
            magic: Magic::Request as u8,
            opcode: Opcode::Increment as u8,
            key_length: key.len() as u16, // TODO: check key length
            extras_length: 20,
            total_body_length: (20 + key.len()) as u32,
            ..Default::default()
        };
        let extras = packet::CounterExtras {
            amount: amount,
            initial_value: 0,
            expiration: 0,
        };
        request_header.write(&mut self.connection.stream)?;
        self.connection.stream.write_u64::<BigEndian>(extras.amount)?;
        self.connection.stream.write_u64::<BigEndian>(
            extras.initial_value,
        )?;
        self.connection.stream.write_u32::<BigEndian>(
            extras.expiration,
        )?;
        self.connection.stream.write(key.as_bytes())?;
        return packet::parse_counter_response(&mut self.connection.stream);
    }


    /// Decrement the value with amount.
    ///
    /// Example:
    ///
    /// ```rust
    /// let mut client = memcache::Client::new("localhost:12345").unwrap();
    /// client.decrement("counter", 42).unwrap();
    /// ```
    pub fn decrement(&mut self, key: &str, amount: u64) -> Result<u64, MemcacheError> {
        let request_header = PacketHeader {
            magic: Magic::Request as u8,
            opcode: Opcode::Decrement as u8,
            key_length: key.len() as u16, // TODO: check key length
            extras_length: 20,
            total_body_length: (20 + key.len()) as u32,
            ..Default::default()
        };
        let extras = packet::CounterExtras {
            amount: amount,
            initial_value: 0,
            expiration: 0,
        };
        request_header.write(&mut self.connection.stream)?;
        self.connection.stream.write_u64::<BigEndian>(extras.amount)?;
        self.connection.stream.write_u64::<BigEndian>(
            extras.initial_value,
        )?;
        self.connection.stream.write_u32::<BigEndian>(
            extras.expiration,
        )?;
        self.connection.stream.write(key.as_bytes())?;
        return packet::parse_counter_response(&mut self.connection.stream);
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn delete() {
        let mut client = super::Client::new("localhost:12345").unwrap();
        client.set("an_exists_key", "value").unwrap();
        assert_eq!(client.delete("an_exists_key").unwrap(), true);
        assert_eq!(client.delete("a_not_exists_key").unwrap(), false);
    }

    #[test]
    fn increment() {
        let mut client = super::Client::new("localhost:12345").unwrap();
        client.delete("counter").unwrap();
        client.set("counter", 321).unwrap();
        assert_eq!(client.increment("counter", 123).unwrap(), 444);
    }
}
