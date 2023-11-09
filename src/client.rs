use std::collections::hash_map::DefaultHasher;
use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::time::Duration;

use url::Url;

use crate::connection::ConnectionManager;
use crate::error::{ClientError, MemcacheError};
use crate::protocol::{Protocol, ProtocolTrait};
use crate::stream::Stream;
use crate::value::{FromMemcacheValueExt, ToMemcacheValue};
use r2d2::Pool;

pub type Stats = HashMap<String, String>;

pub trait Connectable {
    fn get_urls(self) -> Vec<String>;
}

impl Connectable for String {
    fn get_urls(self) -> Vec<String> {
        return vec![self];
    }
}

impl Connectable for Vec<String> {
    fn get_urls(self) -> Vec<String> {
        return self;
    }
}

impl Connectable for &str {
    fn get_urls(self) -> Vec<String> {
        return vec![self.to_string()];
    }
}

impl Connectable for Vec<&str> {
    fn get_urls(self) -> Vec<String> {
        let mut urls = vec![];
        for url in self {
            urls.push(url.to_string());
        }
        return urls;
    }
}

#[derive(Clone)]
pub struct Client {
    connections: Vec<Pool<ConnectionManager>>,
    pub hash_function: fn(&str) -> u64,
    continuum: Continuum
}

#[derive(Clone)]
struct VNode {
    position: u64,
    connection_index: usize
}

#[derive(Clone)]
pub struct Continuum {
    vnodes : Vec<VNode>
}

unsafe impl Send for Client {}

fn default_hash_function(key: &str) -> u64 {
    let mut hasher = DefaultHasher::new();
    key.hash(&mut hasher);
    return hasher.finish();
}

fn empty_hash_function(_key: &str) -> u64 {
    return 0;
}

pub(crate) fn check_key_len(key: &str) -> Result<(), MemcacheError> {
    if key.len() > 250 {
        Err(ClientError::KeyTooLong)?
    }
    Ok(())
}

impl Continuum {
    fn bsearch_continuum_index(&self, hash: u64) -> usize {
        let mut left=0;
        let mut right=self.vnodes.len();
        let mut middle;
        while left < right {
            middle = (left+right)/2;
            if self.vnodes[middle].position < hash {
                left = middle+1;
            }else{
                right = middle;
            }
        }
        left // This can be continuum.vnodes.len()
    }

    fn add_vnode(&mut self, vnodename: String, idx: usize) {
        let hash = default_hash_function(&vnodename);
        if self.vnodes.len() == 0 {
            self.vnodes.push( VNode{ position:hash, connection_index:idx });
        }else{
            let cont_idx = self.bsearch_continuum_index(hash);
            if cont_idx == self.vnodes.len() {
                self.vnodes.push(VNode{ position:hash, connection_index:idx });
            } else {
                self.vnodes.insert(cont_idx, VNode{ position:hash, connection_index:idx });
            }
        }
    }

    fn add_vnodes(&mut self, hostname: String, idx: usize) {
        for n in 0..160 {
            let vnodename = format!("{}-{}#{}",hostname,idx,n);
            self.add_vnode(vnodename,idx);
        }
    }
}

impl Client {
    #[deprecated(since = "0.10.0", note = "please use `connect` instead")]
    pub fn new<C: Connectable>(target: C) -> Result<Self, MemcacheError> {
        return Self::connect(target);
    }

    fn get_connections_index(&self, hash: u64) -> usize {
        let mut idx = self.continuum.bsearch_continuum_index(hash);
        if idx == self.continuum.vnodes.len() { idx = 0; }
        self.continuum.vnodes[idx].connection_index
    }

    pub fn with_pool_size<C: Connectable>(target: C, size: u32) -> Result<Self, MemcacheError> {
        let urls = target.get_urls();
        let mut connections = vec![];
        let mut continuum = Continuum{ vnodes: Vec::new() };
        for url in urls {
            let parsed = Url::parse(url.as_str())?;
            let pool = r2d2::Pool::builder()
                .max_size(size)
                .build(ConnectionManager::new(parsed))?;
            connections.push(pool);
            continuum.add_vnodes(url.to_string(),connections.len()-1);
        }
        Ok(Client {
            connections,
            hash_function: empty_hash_function,
            continuum: continuum,
        })
    }

    pub fn with_pool(pool: Pool<ConnectionManager>) -> Result<Self, MemcacheError> {
        Ok(Client {
            connections: vec![pool],
            hash_function: empty_hash_function,
            continuum: Continuum{ vnodes: Vec::new() }
        })
    }

    pub fn connect<C: Connectable>(target: C) -> Result<Self, MemcacheError> {
        Self::with_pool_size(target, 1)
    }

    fn get_connection(&self, key: &str) -> Pool<ConnectionManager> {
        let connections_count = self.connections.len();
        // XXX: This may not be deterministic so ideally need to hash the whole function itself
        if self.hash_function as isize == empty_hash_function as isize {
            let connection_index = self.get_connections_index(default_hash_function(key));
            self.connections[connection_index as usize].clone()
        } else {
            self.connections[(self.hash_function)(key) as usize % connections_count].clone()
        }
    }

    /// Set the socket read timeout for TCP connections.
    ///
    /// Example:
    ///
    /// ```rust
    /// let client = memcache::Client::connect("memcache://localhost:12345").unwrap();
    /// client.set_read_timeout(Some(::std::time::Duration::from_secs(3))).unwrap();
    /// ```
    pub fn set_read_timeout(&self, timeout: Option<Duration>) -> Result<(), MemcacheError> {
        for conn in self.connections.iter() {
            let mut conn = conn.get()?;
            match **conn {
                Protocol::Ascii(ref mut protocol) => protocol.stream().set_read_timeout(timeout)?,
                Protocol::Binary(ref mut protocol) => protocol.stream.set_read_timeout(timeout)?,
            }
        }
        Ok(())
    }

    /// Set the socket write timeout for TCP connections.
    ///
    /// Example:
    ///
    /// ```rust
    /// let client = memcache::Client::connect("memcache://localhost:12345?protocol=ascii").unwrap();
    /// client.set_write_timeout(Some(::std::time::Duration::from_secs(3))).unwrap();
    /// ```
    pub fn set_write_timeout(&self, timeout: Option<Duration>) -> Result<(), MemcacheError> {
        for conn in self.connections.iter() {
            let mut conn = conn.get()?;
            match **conn {
                Protocol::Ascii(ref mut protocol) => protocol.stream().set_write_timeout(timeout)?,
                Protocol::Binary(ref mut protocol) => protocol.stream.set_write_timeout(timeout)?,
            }
        }
        Ok(())
    }

    /// Get the memcached server version.
    ///
    /// Example:
    ///
    /// ```rust
    /// let client = memcache::Client::connect("memcache://localhost:12345").unwrap();
    /// client.version().unwrap();
    /// ```
    pub fn version(&self) -> Result<Vec<(String, String)>, MemcacheError> {
        let mut result = Vec::with_capacity(self.connections.len());
        for connection in self.connections.iter() {
            let mut connection = connection.get()?;
            let url = connection.get_url();
            result.push((url, connection.version()?));
        }
        Ok(result)
    }

    /// Flush all cache on memcached server immediately.
    ///
    /// Example:
    ///
    /// ```rust
    /// let client = memcache::Client::connect("memcache://localhost:12345").unwrap();
    /// client.flush().unwrap();
    /// ```
    pub fn flush(&self) -> Result<(), MemcacheError> {
        for connection in self.connections.iter() {
            connection.get()?.flush()?;
        }
        return Ok(());
    }

    /// Flush all cache on memcached server with a delay seconds.
    ///
    /// Example:
    ///
    /// ```rust
    /// let client = memcache::Client::connect("memcache://localhost:12345").unwrap();
    /// client.flush_with_delay(10).unwrap();
    /// ```
    pub fn flush_with_delay(&self, delay: u32) -> Result<(), MemcacheError> {
        for connection in self.connections.iter() {
            connection.get()?.flush_with_delay(delay)?;
        }
        return Ok(());
    }

    /// Get a key from memcached server.
    ///
    /// Example:
    ///
    /// ```rust
    /// let client = memcache::Client::connect("memcache://localhost:12345").unwrap();
    /// let _: Option<String> = client.get("foo").unwrap();
    /// ```
    pub fn get<V: FromMemcacheValueExt>(&self, key: &str) -> Result<Option<V>, MemcacheError> {
        check_key_len(key)?;
        return self.get_connection(key).get()?.get(key);
    }

    /// Get multiple keys from memcached server. Using this function instead of calling `get` multiple times can reduce network workloads.
    ///
    /// Example:
    ///
    /// ```rust
    /// let client = memcache::Client::connect("memcache://localhost:12345").unwrap();
    /// client.set("foo", "42", 0).unwrap();
    /// let result: std::collections::HashMap<String, String> = client.gets(&["foo", "bar", "baz"]).unwrap();
    /// assert_eq!(result.len(), 1);
    /// assert_eq!(result["foo"], "42");
    /// ```
    pub fn gets<V: FromMemcacheValueExt>(&self, keys: &[&str]) -> Result<HashMap<String, V>, MemcacheError> {
        for key in keys {
            check_key_len(key)?;
        }
        let mut con_keys: HashMap<usize, Vec<&str>> = HashMap::new();
        let mut result: HashMap<String, V> = HashMap::new();
        let connections_count = self.connections.len();

        for key in keys {
            let connection_index;
            // XXX: This may not be deterministic so ideally need to hash the whole fuction itself
            if self.hash_function as isize == empty_hash_function as isize {
                connection_index = self.get_connections_index(default_hash_function(key));
            } else {
                connection_index = (self.hash_function)(key) as usize % connections_count;
            }
            let array = con_keys.entry(connection_index).or_insert_with(Vec::new);
            array.push(key);
        }

        for (&connection_index, keys) in con_keys.iter() {
            let connection = self.connections[connection_index].clone();
            result.extend(connection.get()?.gets(keys)?);
        }
        return Ok(result);
    }

    /// Set a key with associate value into memcached server with expiration seconds.
    ///
    /// Example:
    ///
    /// ```rust
    /// let client = memcache::Client::connect("memcache://localhost:12345").unwrap();
    /// client.set("foo", "bar", 10).unwrap();
    /// # client.flush().unwrap();
    /// ```
    pub fn set<V: ToMemcacheValue<Stream>>(&self, key: &str, value: V, expiration: u32) -> Result<(), MemcacheError> {
        check_key_len(key)?;
        return self.get_connection(key).get()?.set(key, value, expiration);
    }

    /// Compare and swap a key with the associate value into memcached server with expiration seconds.
    /// `cas_id` should be obtained from a previous `gets` call.
    ///
    /// Example:
    ///
    /// ```rust
    /// use std::collections::HashMap;
    /// let client = memcache::Client::connect("memcache://localhost:12345").unwrap();
    /// client.set("foo", "bar", 10).unwrap();
    /// let result: HashMap<String, (Vec<u8>, u32, Option<u64>)> = client.gets(&["foo"]).unwrap();
    /// let (_, _, cas) = result.get("foo").unwrap();
    /// let cas = cas.unwrap();
    /// assert_eq!(true, client.cas("foo", "bar2", 10, cas).unwrap());
    /// # client.flush().unwrap();
    /// ```
    pub fn cas<V: ToMemcacheValue<Stream>>(
        &self,
        key: &str,
        value: V,
        expiration: u32,
        cas_id: u64,
    ) -> Result<bool, MemcacheError> {
        check_key_len(key)?;
        self.get_connection(key).get()?.cas(key, value, expiration, cas_id)
    }

    /// Add a key with associate value into memcached server with expiration seconds.
    ///
    /// Example:
    ///
    /// ```rust
    /// let client = memcache::Client::connect("memcache://localhost:12345").unwrap();
    /// let key = "add_test";
    /// client.delete(key).unwrap();
    /// client.add(key, "bar", 100000000).unwrap();
    /// # client.flush().unwrap();
    /// ```
    pub fn add<V: ToMemcacheValue<Stream>>(&self, key: &str, value: V, expiration: u32) -> Result<(), MemcacheError> {
        check_key_len(key)?;
        return self.get_connection(key).get()?.add(key, value, expiration);
    }

    /// Replace a key with associate value into memcached server with expiration seconds.
    ///
    /// Example:
    ///
    /// ```rust
    /// let client = memcache::Client::connect("memcache://localhost:12345").unwrap();
    /// let key = "replace_test";
    /// client.set(key, "bar", 0).unwrap();
    /// client.replace(key, "baz", 100000000).unwrap();
    /// # client.flush().unwrap();
    /// ```
    pub fn replace<V: ToMemcacheValue<Stream>>(
        &self,
        key: &str,
        value: V,
        expiration: u32,
    ) -> Result<(), MemcacheError> {
        check_key_len(key)?;
        return self.get_connection(key).get()?.replace(key, value, expiration);
    }

    /// Append value to the key.
    ///
    /// Example:
    ///
    /// ```rust
    /// let client = memcache::Client::connect("memcache://localhost:12345").unwrap();
    /// let key = "key_to_append";
    /// client.set(key, "hello", 0).unwrap();
    /// client.append(key, ", world!").unwrap();
    /// let result: String = client.get(key).unwrap().unwrap();
    /// assert_eq!(result, "hello, world!");
    /// # client.flush().unwrap();
    /// ```
    pub fn append<V: ToMemcacheValue<Stream>>(&self, key: &str, value: V) -> Result<(), MemcacheError> {
        check_key_len(key)?;
        return self.get_connection(key).get()?.append(key, value);
    }

    /// Prepend value to the key.
    ///
    /// Example:
    ///
    /// ```rust
    /// let client = memcache::Client::connect("memcache://localhost:12345").unwrap();
    /// let key = "key_to_append";
    /// client.set(key, "world!", 0).unwrap();
    /// client.prepend(key, "hello, ").unwrap();
    /// let result: String = client.get(key).unwrap().unwrap();
    /// assert_eq!(result, "hello, world!");
    /// # client.flush().unwrap();
    /// ```
    pub fn prepend<V: ToMemcacheValue<Stream>>(&self, key: &str, value: V) -> Result<(), MemcacheError> {
        check_key_len(key)?;
        return self.get_connection(key).get()?.prepend(key, value);
    }

    /// Delete a key from memcached server.
    ///
    /// Example:
    ///
    /// ```rust
    /// let client = memcache::Client::connect("memcache://localhost:12345").unwrap();
    /// client.delete("foo").unwrap();
    /// # client.flush().unwrap();
    /// ```
    pub fn delete(&self, key: &str) -> Result<bool, MemcacheError> {
        check_key_len(key)?;
        return self.get_connection(key).get()?.delete(key);
    }

    /// Increment the value with amount.
    ///
    /// Example:
    ///
    /// ```rust
    /// let client = memcache::Client::connect("memcache://localhost:12345").unwrap();
    /// client.increment("counter", 42).unwrap();
    /// # client.flush().unwrap();
    /// ```
    pub fn increment(&self, key: &str, amount: u64) -> Result<u64, MemcacheError> {
        check_key_len(key)?;
        return self.get_connection(key).get()?.increment(key, amount);
    }

    /// Decrement the value with amount.
    ///
    /// Example:
    ///
    /// ```rust
    /// let client = memcache::Client::connect("memcache://localhost:12345").unwrap();
    /// client.decrement("counter", 42).unwrap();
    /// # client.flush().unwrap();
    /// ```
    pub fn decrement(&self, key: &str, amount: u64) -> Result<u64, MemcacheError> {
        check_key_len(key)?;
        return self.get_connection(key).get()?.decrement(key, amount);
    }

    /// Set a new expiration time for a exist key.
    ///
    /// Example:
    ///
    /// ```rust
    /// let client = memcache::Client::connect("memcache://localhost:12345").unwrap();
    /// assert_eq!(client.touch("not_exists_key", 12345).unwrap(), false);
    /// client.set("foo", "bar", 123).unwrap();
    /// assert_eq!(client.touch("foo", 12345).unwrap(), true);
    /// # client.flush().unwrap();
    /// ```
    pub fn touch(&self, key: &str, expiration: u32) -> Result<bool, MemcacheError> {
        check_key_len(key)?;
        return self.get_connection(key).get()?.touch(key, expiration);
    }

    /// Get all servers' statistics.
    ///
    /// Example:
    /// ```rust
    /// let client = memcache::Client::connect("memcache://localhost:12345").unwrap();
    /// let stats = client.stats().unwrap();
    /// ```
    pub fn stats(&self) -> Result<Vec<(String, Stats)>, MemcacheError> {
        let mut result: Vec<(String, HashMap<String, String>)> = vec![];
        for connection in self.connections.iter() {
            let mut connection = connection.get()?;
            let stats_info = connection.stats()?;
            let url = connection.get_url();
            result.push((url, stats_info));
        }
        return Ok(result);
    }
}

#[cfg(test)]
mod tests {
    #[cfg(unix)]
    #[test]
    fn unix() {
        let client = super::Client::connect("memcache:///tmp/memcached.sock").unwrap();
        assert!(client.version().unwrap()[0].1 != "");
    }

    #[cfg(feature = "tls")]
    #[test]
    fn ssl_noverify() {
        let client = super::Client::connect("memcache+tls://localhost:12350?verify_mode=none").unwrap();
        assert!(client.version().unwrap()[0].1 != "");
    }

    #[cfg(feature = "tls")]
    #[test]
    fn ssl_verify() {
        let client =
            super::Client::connect("memcache+tls://localhost:12350?ca_path=tests/assets/RUST_MEMCACHE_TEST_CERT.crt")
                .unwrap();
        assert!(client.version().unwrap()[0].1 != "");
    }

    #[cfg(feature = "tls")]
    #[test]
    fn ssl_client_certs() {
        let client = super::Client::connect("memcache+tls://localhost:12351?key_path=tests/assets/client.key&cert_path=tests/assets/client.crt&ca_path=tests/assets/RUST_MEMCACHE_TEST_CERT.crt").unwrap();
        assert!(client.version().unwrap()[0].1 != "");
    }

    #[test]
    fn delete() {
        let client = super::Client::connect("memcache://localhost:12345").unwrap();
        client.set("an_exists_key", "value", 0).unwrap();
        assert_eq!(client.delete("an_exists_key").unwrap(), true);
        assert_eq!(client.delete("a_not_exists_key").unwrap(), false);
    }

    #[test]
    fn increment() {
        let client = super::Client::connect("memcache://localhost:12345").unwrap();
        client.delete("counter").unwrap();
        client.set("counter", 321, 0).unwrap();
        assert_eq!(client.increment("counter", 123).unwrap(), 444);
    }

    #[test]
    fn test_bsearch() {
        let mut continuum = super::Continuum{vnodes: Vec::new()};

        continuum.vnodes.push(super::VNode{position:10,connection_index:1});
        continuum.vnodes.push(super::VNode{position:20,connection_index:2});
        continuum.vnodes.push(super::VNode{position:30,connection_index:3});
        continuum.vnodes.push(super::VNode{position:40,connection_index:2});

        assert_eq!(0, continuum.bsearch_continuum_index(5));
        assert_eq!(1, continuum.vnodes[continuum.bsearch_continuum_index(5)].connection_index);
        assert_eq!(1, continuum.bsearch_continuum_index(15));
        assert_eq!(2, continuum.vnodes[continuum.bsearch_continuum_index(15)].connection_index);
        assert_eq!(2, continuum.bsearch_continuum_index(30));
        assert_eq!(3, continuum.vnodes[continuum.bsearch_continuum_index(30)].connection_index);
        assert_eq!(3, continuum.bsearch_continuum_index(31));
        assert_eq!(2, continuum.vnodes[continuum.bsearch_continuum_index(31)].connection_index);

        assert_eq!(4, continuum.bsearch_continuum_index(41));
        assert_eq!(4, continuum.bsearch_continuum_index(65535));

        continuum.vnodes.insert(0,super::VNode{position:1, connection_index:3});
        continuum.vnodes.insert(2,super::VNode{position:19, connection_index:1});
        continuum.vnodes.insert(6,super::VNode{position:50, connection_index:4});

        assert_eq!(0, continuum.bsearch_continuum_index(0));
        assert_eq!(3, continuum.vnodes[continuum.bsearch_continuum_index(0)].connection_index);
        assert_eq!(2, continuum.bsearch_continuum_index(15));
        assert_eq!(1, continuum.vnodes[continuum.bsearch_continuum_index(15)].connection_index);
        assert_eq!(5, continuum.bsearch_continuum_index(31));
        assert_eq!(2, continuum.vnodes[continuum.bsearch_continuum_index(31)].connection_index);
        assert_eq!(6, continuum.bsearch_continuum_index(41));
        assert_eq!(4, continuum.vnodes[continuum.bsearch_continuum_index(41)].connection_index);

        assert_eq!(7, continuum.bsearch_continuum_index(51));
    }

    #[test]
    fn test_key_distribution() {
        let mut servers = Vec::new();
        for i in 0..5 {
            servers.push(format!("memcache://localhost:{}",12345+i));
        }
        let client = super::Client::connect(servers).unwrap();
        let mut map = super::HashMap::<usize, u64>::new();
        for i in 1..10000 {
            let key = super::default_hash_function(&format!("key{}",i));
            let idx = client.get_connections_index(key);
            *map.entry(idx).or_insert(1) += 1;
        }
        for (_k,v) in map {
            // Each server should contain at least 75% of expected # of keys
            assert!( ((10000/5) as f64) * 0.75 < v as f64 );
        }
    }
}
