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

impl Connectable for (&str, u16) {
    fn get_urls(self) -> Vec<String> {
        return vec![format!("{}:{}", self.0, self.1)];
    }
}

impl Connectable for &[(&str, u16)] {
    fn get_urls(self) -> Vec<String> {
        self.iter().map(|(host, port)| format!("{}:{}", host, port)).collect()
    }
}

impl Connectable for Url {
    fn get_urls(self) -> Vec<String> {
        return vec![self.to_string()];
    }
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

pub(crate) fn check_key_len(key: &str) -> Result<(), MemcacheError> {
    if key.len() > 250 {
        Err(ClientError::KeyTooLong)?
    }
    Ok(())
}

impl Client {
    #[deprecated(since = "0.10.0", note = "please use `connect` instead")]
    pub fn new<C: Connectable>(target: C) -> Result<Self, MemcacheError> {
        return Self::connect(target);
    }

    pub fn builder() -> ClientBuilder {
        ClientBuilder::new()
    }

    pub fn with_pool_size<C: Connectable>(target: C, size: u32) -> Result<Self, MemcacheError> {
        let urls = target.get_urls();
        let mut connections = vec![];
        for url in urls {
            let parsed = Url::parse(url.as_str())?;
            let timeout = parsed
                .query_pairs()
                .find(|&(ref k, ref _v)| k == "connect_timeout")
                .and_then(|(ref _k, ref v)| v.parse::<f64>().ok())
                .map(Duration::from_secs_f64);
            let builder = r2d2::Pool::builder().max_size(size);
            let builder = if let Some(timeout) = timeout {
                builder.connection_timeout(timeout)
            } else {
                builder
            };
            let pool = builder.build(ConnectionManager::new(parsed))?;
            connections.push(pool);
        }
        Ok(Client {
            connections,
            hash_function: default_hash_function,
        })
    }

    pub fn with_pool(pool: Pool<ConnectionManager>) -> Result<Self, MemcacheError> {
        Ok(Client {
            connections: vec![pool],
            hash_function: default_hash_function,
        })
    }

    pub fn connect<C: Connectable>(target: C) -> Result<Self, MemcacheError> {
        Self::builder().add_server(target)?.build()
    }

    pub(super) fn get_connection(&self, key: &str) -> Pool<ConnectionManager> {
        let connections_count = self.connections.len();
        return self.connections[(self.hash_function)(key) as usize % connections_count].clone();
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
    ///
    /// #[cfg(not(feature = "async"))]
    /// client.version().unwrap();
    ///
    /// #[cfg(feature = "async")]
    /// async {
    ///     client.version().await.unwrap();
    /// };
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
    ///
    /// #[cfg(not(feature = "async"))]
    /// client.flush().unwrap();
    ///
    /// #[cfg(feature = "async")]
    /// async {
    ///     client.flush().await.unwrap();
    /// };
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
    ///
    /// #[cfg(not(feature = "async"))]
    /// client.flush_with_delay(10).unwrap();
    ///
    /// #[cfg(feature = "async")]
    /// async {
    ///    client.flush_with_delay(10).await.unwrap();
    /// };
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
    ///
    /// #[cfg(not(feature = "async"))]
    /// let _: Option<String> = client.get("foo").unwrap();
    ///
    /// #[cfg(feature = "async")]
    /// async {
    ///     let _: Option<String> = client.get("foo").await.unwrap();
    /// };
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
    ///
    /// #[cfg(not(feature = "async"))]
    /// {
    ///     client.set("foo", "42", 0).unwrap();
    ///     let result: std::collections::HashMap<String, String> = client.gets(&["foo", "bar", "baz"]).unwrap();
    ///     assert_eq!(result.len(), 1);
    ///     assert_eq!(result["foo"], "42");
    /// }
    ///
    /// #[cfg(feature = "async")]
    /// async {
    ///     client.set("foo", "42", 0).await.unwrap();
    ///     let result: std::collections::HashMap<String, String> = client.gets(&["foo", "bar", "baz"]).await.unwrap();
    ///     assert_eq!(result.len(), 1);
    ///     assert_eq!(result["foo"], "42");
    /// };
    /// ```
    pub fn gets<V: FromMemcacheValueExt>(&self, keys: &[&str]) -> Result<HashMap<String, V>, MemcacheError> {
        for key in keys {
            check_key_len(key)?;
        }
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
    ///
    /// #[cfg(not(feature = "async"))]
    /// {
    ///     client.set("foo", "bar", 10).unwrap();
    ///     client.flush().unwrap();
    /// }
    ///
    /// #[cfg(feature = "async")]
    /// async {
    ///     client.set("foo", "bar", 10).await.unwrap();
    ///     client.flush().await.unwrap();
    /// };
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
    ///
    /// #[cfg(not(feature = "async"))]
    /// {
    ///     client.set("foo", "bar", 10).unwrap();
    ///     let result: HashMap<String, (Vec<u8>, u32, Option<u64>)> = client.gets(&["foo"]).unwrap();
    ///     let (_, _, cas) = result.get("foo").unwrap();
    ///     let cas = cas.unwrap();
    ///     assert_eq!(true, client.cas("foo", "bar2", 10, cas).unwrap());
    ///     client.flush().unwrap();
    /// }
    ///
    /// #[cfg(feature = "async")]
    /// async {
    ///     client.set("foo", "bar", 10).await.unwrap();
    ///     let result: HashMap<String, (Vec<u8>, u32, Option<u64>)> = client.gets(&["foo"]).await.unwrap();
    ///     let (_, _, cas) = result.get("foo").unwrap();
    ///     let cas = cas.unwrap();
    ///     assert_eq!(true, client.cas("foo", "bar2", 10, cas).await.unwrap());
    ///     client.flush().await.unwrap();
    /// };
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
    ///
    /// #[cfg(not(feature = "async"))]
    /// {
    ///     client.delete(key).unwrap();
    ///     client.add(key, "bar", 100000000).unwrap();
    ///     client.flush().unwrap();
    /// }
    ///
    /// #[cfg(feature = "async")]
    /// async {
    ///     client.delete(key).await.unwrap();
    ///     client.add(key, "bar", 100000000).await.unwrap();
    ///     client.flush().await.unwrap();
    /// };
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
    ///
    /// #[cfg(not(feature = "async"))]
    /// {
    ///     client.set(key, "bar", 0).unwrap();
    ///     client.replace(key, "baz", 100000000).unwrap();
    ///     client.flush().unwrap();
    /// }
    ///
    /// #[cfg(feature = "async")]
    /// async {
    ///     client.set(key, "bar", 0).await.unwrap();
    ///     client.replace(key, "baz", 100000000).await.unwrap();
    ///     client.flush().await.unwrap();
    /// };
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
    ///
    /// #[cfg(not(feature = "async"))]
    /// {
    ///     client.set(key, "hello", 0).unwrap();
    ///     client.append(key, ", world!").unwrap();
    ///     let result: String = client.get(key).unwrap().unwrap();
    ///     assert_eq!(result, "hello, world!");
    ///     client.flush().unwrap();
    /// }
    ///
    /// #[cfg(feature = "async")]
    /// async {
    ///     client.set(key, "hello", 0).await.unwrap();
    ///     client.append(key, ", world!").await.unwrap();
    ///     let result: String = client.get(key).await.unwrap().unwrap();
    ///     assert_eq!(result, "hello, world!");
    ///     client.flush().await.unwrap();
    /// };
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
    ///
    /// #[cfg(not(feature = "async"))]
    /// {
    ///     client.set(key, "world!", 0).unwrap();
    ///     client.prepend(key, "hello, ").unwrap();
    ///     let result: String = client.get(key).unwrap().unwrap();
    ///     assert_eq!(result, "hello, world!");
    ///     client.flush().unwrap();
    /// }
    ///
    /// #[cfg(feature = "async")]
    /// async {
    ///     client.set(key, "world!", 0).await.unwrap();
    ///     client.prepend(key, "hello, ").await.unwrap();
    ///     let result: String = client.get(key).await.unwrap().unwrap();
    ///     assert_eq!(result, "hello, world!");
    ///     client.flush().await.unwrap();
    /// };
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
    ///
    /// #[cfg(not(feature = "async"))]
    /// {
    ///     client.delete("foo").unwrap();
    ///     client.flush().unwrap();
    /// }
    ///
    /// #[cfg(feature = "async")]
    /// async {
    ///     client.delete("foo").await.unwrap();
    ///     client.flush().await.unwrap();
    /// };
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
    ///
    /// #[cfg(not(feature = "async"))]
    /// {
    ///     client.increment("counter", 42).unwrap();
    ///     client.flush().unwrap();
    /// }
    ///
    /// #[cfg(feature = "async")]
    /// async {
    ///     client.increment("counter", 42).await.unwrap();
    ///     client.flush().await.unwrap();
    /// };
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
    ///
    /// #[cfg(not(feature = "async"))]
    /// {
    ///     client.decrement("counter", 42).unwrap();
    ///     client.flush().unwrap();
    /// }
    ///
    /// #[cfg(feature = "async")]
    /// async {
    ///     client.decrement("counter", 42).await.unwrap();
    ///     client.flush().await.unwrap();
    /// };
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
    ///
    /// #[cfg(not(feature = "async"))]
    /// {
    ///     assert_eq!(client.touch("not_exists_key", 12345).unwrap(), false);
    ///     client.set("foo", "bar", 123).unwrap();
    ///     assert_eq!(client.touch("foo", 12345).unwrap(), true);
    ///     client.flush().unwrap();
    /// }
    ///
    /// #[cfg(feature = "async")]
    /// async {
    ///     assert_eq!(client.touch("not_exists_key", 12345).await.unwrap(), false);
    ///     client.set("foo", "bar", 123).await.unwrap();
    ///     assert_eq!(client.touch("foo", 12345).await.unwrap(), true);
    ///     client.flush().await.unwrap();
    /// };
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
    ///
    /// #[cfg(not(feature = "async"))]
    /// let stats = client.stats().unwrap();
    ///
    /// #[cfg(feature = "async")]
    /// async {
    ///     let stats = client.stats().await.unwrap();
    /// };
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

pub struct ClientBuilder {
    targets: Vec<String>,
    max_size: u32,
    min_idle: Option<u32>,
    max_lifetime: Option<Duration>,
    read_timeout: Option<Duration>,
    write_timeout: Option<Duration>,
    connection_timeout: Option<Duration>,
    hash_function: fn(&str) -> u64,
}

impl ClientBuilder {
    /// Create an empty client builder.
    pub fn new() -> Self {
        ClientBuilder {
            targets: vec![],
            max_size: 1,
            min_idle: None,
            max_lifetime: None,
            read_timeout: None,
            write_timeout: None,
            connection_timeout: None,
            hash_function: default_hash_function,
        }
    }

    /// Add a memcached server to the pool.
    pub fn add_server<C: Connectable>(mut self, target: C) -> Result<Self, MemcacheError> {
        let targets = target.get_urls();

        if targets.len() == 0 {
            return Err(MemcacheError::BadURL("No servers specified".to_string()));
        }

        self.targets.extend(targets);
        Ok(self)
    }

    /// Set the maximum number of connections managed by the pool.
    pub fn with_max_pool_size(mut self, max_size: u32) -> Self {
        self.max_size = max_size;
        self
    }

    /// Set the minimum number of idle connections to maintain in the pool.
    pub fn with_min_idle_conns(mut self, min_idle: u32) -> Self {
        self.min_idle = Some(min_idle);
        self
    }

    /// Set the maximum lifetime of connections in the pool.
    pub fn with_max_conn_lifetime(mut self, max_lifetime: Duration) -> Self {
        self.max_lifetime = Some(max_lifetime);
        self
    }

    /// Set the socket read timeout for TCP connections.
    pub fn with_read_timeout(mut self, read_timeout: Duration) -> Self {
        self.read_timeout = Some(read_timeout);
        self
    }

    /// Set the socket write timeout for TCP connections.
    pub fn with_write_timeout(mut self, write_timeout: Duration) -> Self {
        self.write_timeout = Some(write_timeout);
        self
    }

    /// Set the connection timeout for TCP connections.
    pub fn with_connection_timeout(mut self, connection_timeout: Duration) -> Self {
        self.connection_timeout = Some(connection_timeout);
        self
    }

    /// Set the hash function for the client.
    pub fn with_hash_function(mut self, hash_function: fn(&str) -> u64) -> Self {
        self.hash_function = hash_function;
        self
    }

    /// Build the client. This will create a connection pool and return a client, or an error if the connection pool could not be created.
    pub fn build(self) -> Result<Client, MemcacheError> {
        let urls = self.targets;

        if urls.len() == 0 {
            return Err(MemcacheError::BadURL("No servers specified".to_string()));
        }

        let max_size = self.max_size;
        let min_idle = self.min_idle;
        let max_lifetime = self.max_lifetime;
        let timeout = self.connection_timeout;

        let mut connections = vec![];

        for url in urls.iter() {
            let url = Url::parse(url.as_str()).map_err(|e| MemcacheError::BadURL(e.to_string()))?;

            match url.scheme() {
                "memcache" | "memcache+tls" | "memcache+udp" => {}
                _ => {
                    return Err(MemcacheError::BadURL(format!("Unsupported protocol: {}", url.scheme())));
                }
            }

            let mut builder = r2d2::Pool::builder()
                .max_size(max_size)
                .min_idle(min_idle)
                .max_lifetime(max_lifetime);

            if let Some(timeout) = timeout {
                builder = builder.connection_timeout(timeout);
            }

            let connection = builder
                .build(ConnectionManager::new(url))
                .map_err(|e| MemcacheError::PoolError(e))?;

            connections.push(connection);
        }

        let client = Client {
            connections,
            hash_function: self.hash_function,
        };

        client.set_read_timeout(self.read_timeout)?;
        client.set_write_timeout(self.write_timeout)?;

        Ok(client)
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    #[test]
    fn build_client_happy_path() {
        let client = super::Client::builder()
            .add_server("memcache://localhost:12345")
            .unwrap()
            .build()
            .unwrap();
        assert!(client.version().unwrap()[0].1 != "");
    }

    #[test]
    fn build_client_bad_url() {
        let client = super::Client::builder()
            .add_server("memcache://localhost:12345:")
            .unwrap()
            .build();
        assert!(client.is_err());
    }

    #[test]
    fn build_client_no_url() {
        let client = super::Client::builder().build();
        assert!(client.is_err());

        let client = super::Client::builder().add_server(Vec::<String>::new());

        assert!(client.is_err());
    }

    #[test]
    fn build_client_with_large_pool_size() {
        let client = super::Client::builder()
            .add_server("memcache://localhost:12345")
            .unwrap()
            // This is a large pool size, but it should still be valid.
            // This does make the test run very slow however.
            .with_max_pool_size(100)
            .build();
        assert!(
            client.is_ok(),
            "Expected successful client creation with large pool size"
        );
    }

    #[test]
    fn build_client_with_custom_hash_function() {
        fn custom_hash_function(_key: &str) -> u64 {
            42 // A simple, predictable hash function for testing.
        }

        let client = super::Client::builder()
            .add_server("memcache://localhost:12345")
            .unwrap()
            .with_hash_function(custom_hash_function)
            .build()
            .unwrap();

        // This test assumes that the custom hash function will affect the selection of connections.
        // As the implementation details of connection selection are not exposed, this test might need to be adjusted.
        assert_eq!(
            (client.hash_function)("any_key"),
            42,
            "Expected custom hash function to be used"
        );
    }

    #[test]
    fn build_client_zero_min_idle_conns() {
        let client = super::Client::builder()
            .add_server("memcache://localhost:12345")
            .unwrap()
            .with_min_idle_conns(0)
            .build();
        assert!(client.is_ok(), "Should handle zero min idle conns");
    }

    #[test]
    fn build_client_invalid_hash_function() {
        let invalid_hash_function = |_: &str| -> u64 {
            panic!("This should not be called");
        };
        let client = super::Client::builder()
            .add_server("memcache://localhost:12345")
            .unwrap()
            .with_hash_function(invalid_hash_function)
            .build();
        assert!(client.is_ok(), "Should handle custom hash function gracefully");
    }

    #[test]
    fn build_client_with_unsupported_protocol() {
        let client = super::Client::builder()
            .add_server("unsupported://localhost:12345")
            .unwrap()
            .build();
        assert!(client.is_err(), "Expected error when using an unsupported protocol");
    }

    #[test]
    fn build_client_with_all_optional_parameters() {
        let client = super::Client::builder()
            .add_server("memcache://localhost:12345")
            .unwrap()
            .with_max_pool_size(10)
            .with_min_idle_conns(2)
            .with_max_conn_lifetime(Duration::from_secs(30))
            .with_read_timeout(Duration::from_secs(5))
            .with_write_timeout(Duration::from_secs(5))
            .with_connection_timeout(Duration::from_secs(2))
            .build();
        assert!(client.is_ok(), "Should successfully build with all optional parameters");
    }

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
