use std::io::Write;
use std::collections::HashMap;
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
#[cfg(unix)]
use byteorder::{WriteBytesExt, BigEndian};
use connection::Connection;
use error::MemcacheError;
use value::{ToMemcacheValue, FromMemcacheValue};
use packet;
use packet::{Opcode, PacketHeader, Magic};

pub trait Connectable<'a> {
    fn get_urls(self) -> Vec<&'a str>;
}

impl<'a> Connectable<'a> for &'a str {
    fn get_urls(self) -> Vec<&'a str> {
        return vec![self];
    }
}

impl<'a> Connectable<'a> for Vec<&'a str> {
    fn get_urls(self) -> Vec<&'a str> {
        return self;
    }
}

pub struct Client {
    connections: Vec<Connection>,
    hash_function: fn(&str) -> u64,
}

fn default_hash_function(key: &str) -> u64 {
    let mut hasher = DefaultHasher::new();
    key.hash(&mut hasher);
    return hasher.finish();
}

impl<'a> Client {
    pub fn new<C: Connectable<'a>>(target: C) -> Result<Self, MemcacheError> {
        let urls = target.get_urls();
        let mut connections = vec![];
        for url in urls {
            connections.push(Connection::connect(url)?);
        }
        return Ok(Client {
            connections: connections,
            hash_function: default_hash_function,
        });
    }

    fn get_connection(&mut self, key: &str) -> &mut Connection {
        let connections_count = self.connections.len();
        return &mut self.connections[(self.hash_function)(key) as usize % connections_count];
    }

    /// Get the memcached server version.
    ///
    /// Example:
    ///
    /// ```rust
    /// let mut client = memcache::Client::new("memcache://localhost:12345").unwrap();
    /// client.version().unwrap();
    /// ```
    pub fn version(&mut self) -> Result<String, MemcacheError> {
        let request_header = PacketHeader {
            magic: Magic::Request as u8,
            opcode: Opcode::Version as u8,
            ..Default::default()
        };
        request_header.write(self.get_connection("TODO"))?;
        return packet::parse_version_response(self.get_connection("TODO"));
    }

    /// Flush all cache on memcached server immediately.
    ///
    /// Example:
    ///
    /// ```rust
    /// let mut client = memcache::Client::new("memcache://localhost:12345").unwrap();
    /// client.flush().unwrap();
    /// ```
    pub fn flush(&mut self) -> Result<(), MemcacheError> {
        let request_header = PacketHeader {
            magic: Magic::Request as u8,
            opcode: Opcode::Flush as u8,
            ..Default::default()
        };
        request_header.write(self.get_connection("TODO"))?;
        return packet::parse_header_only_response(self.get_connection("TODO"));
    }

    /// Flush all cache on memcached server with a delay seconds.
    ///
    /// Example:
    ///
    /// ```rust
    /// let mut client = memcache::Client::new("memcache://localhost:12345").unwrap();
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
        request_header.write(self.get_connection("TODO"))?;
        self.get_connection("TODO").write_u32::<BigEndian>(delay)?;
        return packet::parse_header_only_response(self.get_connection("TODO"));
    }

    /// Get a key from memcached server.
    ///
    /// Example:
    ///
    /// ```rust
    /// let mut client = memcache::Client::new("memcache://localhost:12345").unwrap();
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
        request_header.write(self.get_connection(key))?;
        self.get_connection(key).write(key.as_bytes())?;
        return packet::parse_get_response(self.get_connection(key));
    }

    /// Get multiple keys from memcached server. Using this function instead of calling `get` multiple times can reduce netwark workloads.
    ///
    /// Example:
    ///
    /// ```rust
    /// let mut client = memcache::Client::new("memcache://localhost:12345").unwrap();
    /// client.set("foo", "42");
    /// let result: std::collections::HashMap<String, String> = client.gets(vec!["foo", "bar", "baz"]).unwrap();
    /// assert_eq!(result.len(), 1);
    /// assert_eq!(result["foo"], "42");
    /// ```
    pub fn gets<V: FromMemcacheValue>(
        &mut self,
        keys: Vec<&str>,
    ) -> Result<HashMap<String, V>, MemcacheError> {
        for key in keys {
            let request_header = PacketHeader {
                magic: Magic::Request as u8,
                opcode: Opcode::GetKQ as u8,
                key_length: key.len() as u16, // TODO: check key length
                total_body_length: key.len() as u32,
                ..Default::default()
            };
            request_header.write(self.get_connection(key))?;
            self.get_connection(key).write(key.as_bytes())?;
        }
        let noop_request_header = PacketHeader {
            magic: Magic::Request as u8,
            opcode: Opcode::Noop as u8,
            ..Default::default()
        };
        noop_request_header.write(self.get_connection("TODO"))?;
        return packet::parse_gets_response(self.get_connection("TODO"));
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
        request_header.write(self.get_connection(key))?;
        self.get_connection(key).write_u32::<BigEndian>(
            extras.flags,
        )?;
        self.get_connection(key).write_u32::<BigEndian>(
            extras.expiration,
        )?;
        self.get_connection(key).write(key.as_bytes())?;
        value.write_to(self.get_connection(key))?;
        return packet::parse_header_only_response(self.get_connection(key));
    }

    /// Set a key with associate value into memcached server.
    ///
    /// Example:
    ///
    /// ```rust
    /// let mut client = memcache::Client::new("memcache://localhost:12345").unwrap();
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
    /// let mut client = memcache::Client::new("memcache://localhost:12345").unwrap();
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
    /// let mut client = memcache::Client::new("memcache://localhost:12345").unwrap();
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
    /// let mut client = memcache::Client::new("memcache://localhost:12345").unwrap();
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
    /// let mut client = memcache::Client::new("memcache://localhost:12345").unwrap();
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
    /// let mut client = memcache::Client::new("memcache://localhost:12345").unwrap();
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

    /// Append value to the key.
    ///
    /// Example:
    ///
    /// ```rust
    /// let mut client = memcache::Client::new("memcache://localhost:12345").unwrap();
    /// let key = "key_to_append";
    /// client.set(key, "hello").unwrap();
    /// client.append(key, ", world!").unwrap();
    /// let result: String = client.get(key).unwrap().unwrap();
    /// assert_eq!(result, "hello, world!");
    /// ```
    pub fn append<V: ToMemcacheValue<Connection>>(
        &mut self,
        key: &str,
        value: V,
    ) -> Result<(), MemcacheError> {
        let request_header = PacketHeader {
            magic: Magic::Request as u8,
            opcode: Opcode::Append as u8,
            key_length: key.len() as u16, // TODO: check key length
            total_body_length: (key.len() + value.get_length()) as u32,
            ..Default::default()
        };
        request_header.write(self.get_connection(key))?;
        self.get_connection(key).write(key.as_bytes())?;
        value.write_to(self.get_connection(key))?;
        return packet::parse_header_only_response(self.get_connection(key));
    }

    /// Prepend value to the key.
    ///
    /// Example:
    ///
    /// ```rust
    /// let mut client = memcache::Client::new("memcache://localhost:12345").unwrap();
    /// let key = "key_to_append";
    /// client.set(key, "world!").unwrap();
    /// client.prepend(key, "hello, ").unwrap();
    /// let result: String = client.get(key).unwrap().unwrap();
    /// assert_eq!(result, "hello, world!");
    /// ```
    pub fn prepend<V: ToMemcacheValue<Connection>>(
        &mut self,
        key: &str,
        value: V,
    ) -> Result<(), MemcacheError> {
        let request_header = PacketHeader {
            magic: Magic::Request as u8,
            opcode: Opcode::Prepend as u8,
            key_length: key.len() as u16, // TODO: check key length
            total_body_length: (key.len() + value.get_length()) as u32,
            ..Default::default()
        };
        request_header.write(self.get_connection(key))?;
        self.get_connection(key).write(key.as_bytes())?;
        value.write_to(&mut self.get_connection(key))?;
        return packet::parse_header_only_response(self.get_connection(key));
    }

    /// Delete a key from memcached server.
    ///
    /// Example:
    ///
    /// ```rust
    /// let mut client = memcache::Client::new("memcache://localhost:12345").unwrap();
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
        request_header.write(self.get_connection(key))?;
        self.get_connection(key).write(key.as_bytes())?;
        return packet::parse_delete_response(self.get_connection(key));
    }

    /// Increment the value with amount.
    ///
    /// Example:
    ///
    /// ```rust
    /// let mut client = memcache::Client::new("memcache://localhost:12345").unwrap();
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
        request_header.write(self.get_connection(key))?;
        self.get_connection(key).write_u64::<BigEndian>(
            extras.amount,
        )?;
        self.get_connection(key).write_u64::<BigEndian>(
            extras.initial_value,
        )?;
        self.get_connection(key).write_u32::<BigEndian>(
            extras.expiration,
        )?;
        self.get_connection(key).write(key.as_bytes())?;
        return packet::parse_counter_response(self.get_connection(key));
    }


    /// Decrement the value with amount.
    ///
    /// Example:
    ///
    /// ```rust
    /// let mut client = memcache::Client::new("memcache://localhost:12345").unwrap();
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
        request_header.write(self.get_connection(key))?;
        self.get_connection(key).write_u64::<BigEndian>(
            extras.amount,
        )?;
        self.get_connection(key).write_u64::<BigEndian>(
            extras.initial_value,
        )?;
        self.get_connection(key).write_u32::<BigEndian>(
            extras.expiration,
        )?;
        self.get_connection(key).write(key.as_bytes())?;
        return packet::parse_counter_response(self.get_connection(key));
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn unix() {
        let mut client = super::Client::new("memcache:///tmp/memcached.sock").unwrap();
        assert!(client.version().unwrap() != "");
    }

    #[test]
    fn delete() {
        let mut client = super::Client::new("memcache://localhost:12345").unwrap();
        client.set("an_exists_key", "value").unwrap();
        assert_eq!(client.delete("an_exists_key").unwrap(), true);
        assert_eq!(client.delete("a_not_exists_key").unwrap(), false);
    }

    #[test]
    fn increment() {
        let mut client = super::Client::new("memcache://localhost:12345").unwrap();
        client.delete("counter").unwrap();
        client.set("counter", 321).unwrap();
        assert_eq!(client.increment("counter", 123).unwrap(), 444);
    }
}
