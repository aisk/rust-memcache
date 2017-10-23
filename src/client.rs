use std::io::Write;
use std::net::{ToSocketAddrs, TcpStream};
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
    pub fn connect<A: ToSocketAddrs>(addr: A) -> Result<Self, MemcacheError> {
        let stream = TcpStream::connect(addr)?;
        return Ok(Client { connection: Connection::TcpStream(stream) });
    }

    /// Get the memcached server version.
    ///
    /// Example:
    ///
    /// ```rust
    /// let mut client = memcache::Client::connect("localhost:12345").unwrap();
    /// client.version().unwrap();
    /// ```
    pub fn version(&mut self) -> Result<String, MemcacheError> {
        let request_header = PacketHeader {
            magic: Magic::Request as u8,
            opcode: Opcode::Version as u8,
            ..Default::default()
        };
        request_header.write(&mut self.connection)?;
        return packet::parse_version_response(&mut self.connection);
    }

    /// Flush all cache on memcached server immediately.
    ///
    /// Example:
    ///
    /// ```rust
    /// let mut client = memcache::Client::connect("localhost:12345").unwrap();
    /// client.flush().unwrap();
    /// ```
    pub fn flush(&mut self) -> Result<(), MemcacheError> {
        let request_header = PacketHeader {
            magic: Magic::Request as u8,
            opcode: Opcode::Flush as u8,
            ..Default::default()
        };
        request_header.write(&mut self.connection)?;
        return packet::parse_header_only_response(&mut self.connection);
    }

    /// Flush all cache on memcached server with a delay seconds.
    ///
    /// Example:
    ///
    /// ```rust
    /// let mut client = memcache::Client::connect("localhost:12345").unwrap();
    /// client.flush_with_delay(10).unwrap();
    /// ```
    pub fn flush_with_delay(&mut self, delay: u32) -> Result<(), MemcacheError> {
        let request_header = PacketHeader {
            magic: Magic::Request as u8,
            opcode: Opcode::Flush as u8,
            extras_length: 4,
            total_body_length: 4,
            ..Default::default()
        };
        request_header.write(&mut self.connection)?;
        self.connection.write_u32::<BigEndian>(delay)?;
        return packet::parse_header_only_response(&mut self.connection);
    }

    /// Get a key from memcached server.
    ///
    /// Example:
    ///
    /// ```rust
    /// let mut client = memcache::Client::connect("localhost:12345").unwrap();
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
        request_header.write(&mut self.connection)?;
        self.connection.write(key.as_bytes())?;
        return packet::parse_get_response(&mut self.connection);
    }

    fn store<V: ToMemcacheValue<Connection>>(
        &mut self,
        opcode: Opcode,
        key: &str,
        value: V,
        expiration: u32,
    ) -> Result<(), MemcacheError> {
        let request_header = PacketHeader {
            magic: Magic::Request as u8,
            opcode: opcode as u8,
            key_length: key.len() as u16, // TODO: check key length
            extras_length: 8,
            total_body_length: (8 + key.len() + value.get_length()) as u32,
            ..Default::default()
        };
        let extras = packet::StoreExtras {
            flags: value.get_flags(),
            expiration: expiration,
        };
        request_header.write(&mut self.connection)?;
        self.connection.write_u32::<BigEndian>(extras.flags)?;
        self.connection.write_u32::<BigEndian>(extras.expiration)?;
        self.connection.write(key.as_bytes())?;
        value.write_to(&mut self.connection)?;
        return packet::parse_header_only_response(&mut self.connection);
    }

    /// Set a key with associate value into memcached server.
    ///
    /// Example:
    ///
    /// ```rust
    /// let mut client = memcache::Client::connect("localhost:12345").unwrap();
    /// client.set("foo", "bar").unwrap();
    /// ```
    pub fn set<V: ToMemcacheValue<Connection>>(
        &mut self,
        key: &str,
        value: V,
    ) -> Result<(), MemcacheError> {
        return self.store(Opcode::Set, key, value, 0);
    }

    /// Set a key with associate value into memcached server with expiration seconds.
    ///
    /// Example:
    ///
    /// ```rust
    /// let mut client = memcache::Client::connect("localhost:12345").unwrap();
    /// client.set_with_expiration("foo", "bar", 10).unwrap();
    /// ```
    pub fn set_with_expiration<V: ToMemcacheValue<Connection>>(
        &mut self,
        key: &str,
        value: V,
        expiration: u32,
    ) -> Result<(), MemcacheError> {
        return self.store(Opcode::Set, key, value, expiration);
    }

    /// Add a key with associate value into memcached server.
    ///
    /// Example:
    ///
    /// ```rust
    /// let mut client = memcache::Client::connect("localhost:12345").unwrap();
    /// let key = "add_test";
    /// client.delete(key).unwrap();
    /// client.add(key, "bar").unwrap();
    /// ```
    pub fn add<V: ToMemcacheValue<Connection>>(
        &mut self,
        key: &str,
        value: V,
    ) -> Result<(), MemcacheError> {
        return self.store(Opcode::Add, key, value, 0);
    }

    /// Add a key with associate value into memcached server with expiration seconds.
    ///
    /// Example:
    ///
    /// ```rust
    /// let mut client = memcache::Client::connect("localhost:12345").unwrap();
    /// let key = "add_with_expiration_test";
    /// client.delete(key).unwrap();
    /// client.add_with_expiration(key, "bar", 100000000).unwrap();
    /// ```
    pub fn add_with_expiration<V: ToMemcacheValue<Connection>>(
        &mut self,
        key: &str,
        value: V,
        expiration: u32,
    ) -> Result<(), MemcacheError> {
        return self.store(Opcode::Add, key, value, expiration);
    }

    /// Replace a key with associate value into memcached server.
    ///
    /// Example:
    ///
    /// ```rust
    /// let mut client = memcache::Client::connect("localhost:12345").unwrap();
    /// let key = "replace_test";
    /// client.set(key, "bar").unwrap();
    /// client.replace(key, "baz").unwrap();
    /// ```
    pub fn replace<V: ToMemcacheValue<Connection>>(
        &mut self,
        key: &str,
        value: V,
    ) -> Result<(), MemcacheError> {
        return self.store(Opcode::Replace, key, value, 0);
    }

    /// Replace a key with associate value into memcached server with expiration seconds.
    ///
    /// Example:
    ///
    /// ```rust
    /// let mut client = memcache::Client::connect("localhost:12345").unwrap();
    /// let key = "replace_with_expiration_test";
    /// client.set(key, "bar").unwrap();
    /// client.replace_with_expiration(key, "baz", 100000000).unwrap();
    /// ```
    pub fn replace_with_expiration<V: ToMemcacheValue<Connection>>(
        &mut self,
        key: &str,
        value: V,
        expiration: u32,
    ) -> Result<(), MemcacheError> {
        return self.store(Opcode::Replace, key, value, expiration);
    }

    /// Delete a key from memcached server.
    ///
    /// Example:
    ///
    /// ```rust
    /// let mut client = memcache::Client::connect("localhost:12345").unwrap();
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
        request_header.write(&mut self.connection)?;
        self.connection.write(key.as_bytes())?;
        return packet::parse_delete_response(&mut self.connection);
    }

    /// Increment the value with amount.
    ///
    /// Example:
    ///
    /// ```rust
    /// let mut client = memcache::Client::connect("localhost:12345").unwrap();
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
        request_header.write(&mut self.connection)?;
        self.connection.write_u64::<BigEndian>(extras.amount)?;
        self.connection.write_u64::<BigEndian>(extras.initial_value)?;
        self.connection.write_u32::<BigEndian>(extras.expiration)?;
        self.connection.write(key.as_bytes())?;
        return packet::parse_counter_response(&mut self.connection);
    }


    /// Decrement the value with amount.
    ///
    /// Example:
    ///
    /// ```rust
    /// let mut client = memcache::Client::connect("localhost:12345").unwrap();
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
        request_header.write(&mut self.connection)?;
        self.connection.write_u64::<BigEndian>(extras.amount)?;
        self.connection.write_u64::<BigEndian>(extras.initial_value)?;
        self.connection.write_u32::<BigEndian>(extras.expiration)?;
        self.connection.write(key.as_bytes())?;
        return packet::parse_counter_response(&mut self.connection);
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn delete() {
        let mut client = super::Client::connect("localhost:12345").unwrap();
        client.set("an_exists_key", "value").unwrap();
        assert_eq!(client.delete("an_exists_key").unwrap(), true);
        assert_eq!(client.delete("a_not_exists_key").unwrap(), false);
    }

    #[test]
    fn increment() {
        let mut client = super::Client::connect("localhost:12345").unwrap();
        client.delete("counter").unwrap();
        client.set("counter", 321).unwrap();
        assert_eq!(client.increment("counter", 123).unwrap(), 444);
    }
}
