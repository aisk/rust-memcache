use std::collections::hash_map::DefaultHasher;
use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::time::Duration;

use url::Url;

use connection::{ConnCustomizer, ConnectionManager};
use error::MemcacheError;
use protocol::ProtocolTrait;
use r2d2::Pool;
use stream::Stream;
use value::{FromMemcacheValueExt, ToMemcacheValue};

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
}

unsafe impl Send for Client {}

fn default_hash_function(key: &str) -> u64 {
    let mut hasher = DefaultHasher::new();
    key.hash(&mut hasher);
    return hasher.finish();
}

pub struct ClientBuilder<C> {
    target: C,
    max_size: u32,
    min_idle: Option<u32>,
    max_lifetime: Option<Duration>,
    read_timeout: Option<Duration>,
    write_timeout: Option<Duration>,
    pool_wait_timeout: Duration,
}

impl<C: Connectable> ClientBuilder<C> {
    pub fn new(target: C) -> Self {
        Self {
            target,
            max_size: 1,
            min_idle: None,
            max_lifetime: None,
            read_timeout: None,
            write_timeout: None,
            pool_wait_timeout: Duration::new(30, 0),
        }
    }

    pub fn with_min_idle(mut self, min_idle: Option<u32>) -> Self {
        self.min_idle = min_idle;
        self
    }

    pub fn with_max_lifetime(mut self, max_lifetime: Option<Duration>) -> Self {
        self.max_lifetime = max_lifetime;
        self
    }

    pub fn with_max_size(mut self, size: u32) -> Self {
        self.max_size = size;
        self
    }

    pub fn with_read_timeout(mut self, read_timeout: Option<Duration>) -> Self {
        self.read_timeout = read_timeout;
        self
    }

    pub fn with_pool_wait_timeout(mut self, pool_wait_timeout: Duration) -> Self {
        self.pool_wait_timeout = pool_wait_timeout;
        self
    }

    pub fn with_write_timeout(mut self, write_timeout: Option<Duration>) -> Self {
        self.write_timeout = write_timeout;
        self
    }

    pub fn build(self) -> Result<Client, MemcacheError> {
        let urls = self.target.get_urls();
        let mut connections = vec![];
        for url in urls {
            let parsed = Url::parse(url.as_str())?;
            let pool = Pool::builder()
                .max_size(self.max_size)
                .min_idle(self.min_idle)
                .max_lifetime(self.max_lifetime)
                .connection_timeout(self.pool_wait_timeout)
                .connection_customizer(Box::new(ConnCustomizer::new(self.read_timeout, self.write_timeout)))
                .build(ConnectionManager::new(parsed))?;
            connections.push(pool);
        }
        Ok(Client {
            connections,
            hash_function: default_hash_function,
        })
    }
}

impl Client {
    #[deprecated(since = "0.10.0", note = "please use `connect` instead")]
    pub fn new<C: Connectable>(target: C) -> Result<Self, MemcacheError> {
        return Self::connect(target);
    }

    pub fn connect<C: Connectable>(target: C) -> Result<Self, MemcacheError> {
        ClientBuilder::new(target).with_max_size(1).build()
    }

    fn get_connection(&self, key: &str) -> Pool<ConnectionManager> {
        let connections_count = self.connections.len();
        return self.connections[(self.hash_function)(key) as usize % connections_count].clone();
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
        return self.get_connection(key).get()?.get(key);
    }

    /// Get multiple keys from memcached server. Using this function instead of calling `get` multiple times can reduce netwark workloads.
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
        let mut con_keys: HashMap<usize, Vec<&str>> = HashMap::new();
        let mut result: HashMap<String, V> = HashMap::new();
        let connections_count = self.connections.len();

        for key in keys {
            let connection_index = (self.hash_function)(key) as usize % connections_count;
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
}
