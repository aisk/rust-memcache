use std::collections::HashMap;
use std::time::Duration;

use crate::client::Stats;
use crate::error::MemcacheError;
use crate::stream::Stream;
use crate::value::{FromMemcacheValueExt, ToMemcacheValue};
use crate::Connectable;

use super::client as blocking;

pub struct Client {
    inner: blocking::Client,
}

impl From<blocking::Client> for Client {
    fn from(client: blocking::Client) -> Self {
        Self { inner: client }
    }
}

impl Client {
    pub fn builder() -> ClientBuilder {
        ClientBuilder::new()
    }

    pub fn connect<C: Connectable>(target: C) -> Result<Self, MemcacheError> {
        Ok(blocking::Client::connect(target)?.into())
    }

    /// Get a reference to the inner `Client` object.
    /// This will allow you to call methods on the `Client` object synchronously.
    ///
    /// Example:
    ///
    /// ```rust
    /// let client = memcache::Client::connect("memcache://localhost:12345").unwrap();
    /// let blocking_client = client.blocking();
    /// let _: Option<String> = blocking_client.get("foo").unwrap();
    /// ```
    pub fn blocking(&self) -> &blocking::Client {
        &self.inner
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
        self.inner.set_read_timeout(timeout)
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
        self.inner.set_write_timeout(timeout)
    }

    /// Get the memcached server version.
    ///
    /// Example:
    ///
    /// ```rust
    /// let client = memcache::Client::connect("memcache://localhost:12345").unwrap();
    /// async {
    ///     client.version().await.unwrap();
    /// };
    /// ```
    pub async fn version(&self) -> Result<Vec<(String, String)>, MemcacheError> {
        self.inner.version()
    }

    /// Flush all cache on memcached server immediately.
    ///
    /// Example:
    ///
    /// ```rust
    /// let client = memcache::Client::connect("memcache://localhost:12345").unwrap();
    /// async {
    ///     client.flush().await.unwrap();
    /// };
    /// ```
    pub async fn flush(&self) -> Result<(), MemcacheError> {
        self.inner.flush()
    }

    /// Flush all cache on memcached server with a delay seconds.
    ///
    /// Example:
    ///
    /// ```rust
    /// let client = memcache::Client::connect("memcache://localhost:12345").unwrap();
    /// async {
    ///     client.flush_with_delay(10).await.unwrap();
    /// };
    /// ```
    pub async fn flush_with_delay(&self, delay: u32) -> Result<(), MemcacheError> {
        self.inner.flush_with_delay(delay)
    }

    /// Get a key from memcached server.
    ///
    /// Example:
    ///
    /// ```rust
    /// let client = memcache::Client::connect("memcache://localhost:12345").unwrap();
    /// async {
    ///     let _: Option<String> = client.get("foo").await.unwrap();
    /// };
    /// ```
    pub async fn get<V: FromMemcacheValueExt>(&self, key: &str) -> Result<Option<V>, MemcacheError> {
        self.inner.get(key)
    }

    /// Get multiple keys from memcached server. Using this function instead of calling `get` multiple times can reduce network workloads.
    ///
    /// Example:
    ///
    /// ```rust
    /// let client = memcache::Client::connect("memcache://localhost:12345").unwrap();
    /// async {
    ///     client.set("foo", "42", 0).await.unwrap();
    ///     let result: std::collections::HashMap<String, String> = client.gets(&["foo", "bar", "baz"]).await.unwrap();
    ///     assert_eq!(result.len(), 1);
    ///     assert_eq!(result["foo"], "42");
    /// };
    /// ```
    pub async fn gets<V: FromMemcacheValueExt>(&self, keys: &[&str]) -> Result<HashMap<String, V>, MemcacheError> {
        self.inner.gets(keys)
    }

    /// Set a key with associate value into memcached server with expiration seconds.
    ///
    /// Example:
    ///
    /// ```rust
    /// let client = memcache::Client::connect("memcache://localhost:12345").unwrap();
    /// async {
    ///     client.set("foo", "bar", 10).await.unwrap();
    ///     client.flush().await.unwrap();
    /// };
    /// ```
    pub async fn set<V: ToMemcacheValue<Stream>>(
        &self,
        key: &str,
        value: V,
        expiration: u32,
    ) -> Result<(), MemcacheError> {
        self.inner.set(key, value, expiration)
    }

    /// Compare and swap a key with the associate value into memcached server with expiration seconds.
    /// `cas_id` should be obtained from a previous `gets` call.
    ///
    /// Example:
    ///
    /// ```rust
    /// use std::collections::HashMap;
    /// let client = memcache::Client::connect("memcache://localhost:12345").unwrap();
    /// async {
    ///     client.set("foo", "bar", 10).await.unwrap();
    ///     let result: HashMap<String, (Vec<u8>, u32, Option<u64>)> = client.gets(&["foo"]).await.unwrap();
    ///     let (_, _, cas) = result.get("foo").unwrap();
    ///     let cas = cas.unwrap();
    ///     assert_eq!(true, client.cas("foo", "bar2", 10, cas).await.unwrap());
    ///     client.flush().await.unwrap();
    /// };
    /// ```
    pub async fn cas<V: ToMemcacheValue<Stream>>(
        &self,
        key: &str,
        value: V,
        expiration: u32,
        cas_id: u64,
    ) -> Result<bool, MemcacheError> {
        self.inner.cas(key, value, expiration, cas_id)
    }

    /// Add a key with associate value into memcached server with expiration seconds.
    ///
    /// Example:
    ///
    /// ```rust
    /// let client = memcache::Client::connect("memcache://localhost:12345").unwrap();
    /// let key = "add_test";
    /// async {
    ///     client.delete(key).await.unwrap();
    ///     client.add(key, "bar", 100000000).await.unwrap();
    ///     client.flush().await.unwrap();
    /// };
    /// ```
    pub async fn add<V: ToMemcacheValue<Stream>>(
        &self,
        key: &str,
        value: V,
        expiration: u32,
    ) -> Result<(), MemcacheError> {
        self.inner.add(key, value, expiration)
    }

    /// Replace a key with associate value into memcached server with expiration seconds.
    ///
    /// Example:
    ///
    /// ```rust
    /// let client = memcache::Client::connect("memcache://localhost:12345").unwrap();
    /// let key = "replace_test";
    /// async {
    ///     client.set(key, "bar", 0).await.unwrap();
    ///     client.replace(key, "baz", 100000000).await.unwrap();
    ///     client.flush().await.unwrap();
    /// };
    /// ```
    pub async fn replace<V: ToMemcacheValue<Stream>>(
        &self,
        key: &str,
        value: V,
        expiration: u32,
    ) -> Result<(), MemcacheError> {
        self.inner.replace(key, value, expiration)
    }

    /// Append value to the key.
    ///
    /// Example:
    ///
    /// ```rust
    /// let client = memcache::Client::connect("memcache://localhost:12345").unwrap();
    /// let key = "key_to_append";
    /// async {
    ///     client.set(key, "hello", 0).await.unwrap();
    ///     client.append(key, ", world!").await.unwrap();
    ///     let result: String = client.get(key).await.unwrap().unwrap();
    ///     assert_eq!(result, "hello, world!");
    ///     client.flush().await.unwrap();
    /// };
    /// ```
    pub async fn append<V: ToMemcacheValue<Stream>>(&self, key: &str, value: V) -> Result<(), MemcacheError> {
        self.inner.append(key, value)
    }

    /// Prepend value to the key.
    ///
    /// Example:
    ///
    /// ```rust
    /// let client = memcache::Client::connect("memcache://localhost:12345").unwrap();
    /// let key = "key_to_append";
    /// async {
    ///     client.set(key, "world!", 0).await.unwrap();
    ///     client.prepend(key, "hello, ").await.unwrap();
    ///     let result: String = client.get(key).await.unwrap().unwrap();
    ///     assert_eq!(result, "hello, world!");
    ///     client.flush().await.unwrap();
    /// };
    /// ```
    pub async fn prepend<V: ToMemcacheValue<Stream>>(&self, key: &str, value: V) -> Result<(), MemcacheError> {
        self.inner.prepend(key, value)
    }

    /// Delete a key from memcached server.
    ///
    /// Example:
    ///
    /// ```rust
    /// let client = memcache::Client::connect("memcache://localhost:12345").unwrap();
    /// async {
    ///     client.delete("foo").await.unwrap();
    ///     client.flush().await.unwrap();
    /// };
    /// ```
    pub async fn delete(&self, key: &str) -> Result<bool, MemcacheError> {
        self.inner.delete(key)
    }

    /// Increment the value with amount.
    ///
    /// Example:
    ///
    /// ```rust
    /// let client = memcache::Client::connect("memcache://localhost:12345").unwrap();
    /// async {
    ///     client.increment("counter", 42).await.unwrap();
    ///     client.flush().await.unwrap();
    /// };
    /// ```
    pub async fn increment(&self, key: &str, amount: u64) -> Result<u64, MemcacheError> {
        self.inner.increment(key, amount)
    }

    /// Decrement the value with amount.
    ///
    /// Example:
    ///
    /// ```rust
    /// let client = memcache::Client::connect("memcache://localhost:12345").unwrap();
    /// async {
    ///     client.decrement("counter", 42).await.unwrap();
    ///     client.flush().await.unwrap();
    /// };
    /// ```
    pub async fn decrement(&self, key: &str, amount: u64) -> Result<u64, MemcacheError> {
        self.inner.decrement(key, amount)
    }

    /// Set a new expiration time for a exist key.
    ///
    /// Example:
    ///
    /// ```rust
    /// let client = memcache::Client::connect("memcache://localhost:12345").unwrap();
    /// async {
    ///     assert_eq!(client.touch("not_exists_key", 12345).await.unwrap(), false);
    ///     client.set("foo", "bar", 123).await.unwrap();
    ///     assert_eq!(client.touch("foo", 12345).await.unwrap(), true);
    ///     client.flush().await.unwrap();
    /// };
    /// ```
    pub async fn touch(&self, key: &str, expiration: u32) -> Result<bool, MemcacheError> {
        self.inner.touch(key, expiration)
    }

    /// Get all servers' statistics.
    ///
    /// Example:
    /// ```rust
    /// let client = memcache::Client::connect("memcache://localhost:12345").unwrap();
    /// async {
    ///     let stats = client.stats().await.unwrap();
    /// };
    /// ```
    pub async fn stats(&self) -> Result<Vec<(String, Stats)>, MemcacheError> {
        self.inner.stats()
    }
}

pub struct ClientBuilder {
    inner: blocking::ClientBuilder,
}

impl ClientBuilder {
    /// Create an empty client builder.
    pub fn new() -> Self {
        ClientBuilder {
            inner: blocking::ClientBuilder::new(),
        }
    }

    /// Add a memcached server to the pool.
    pub fn add_server<C: Connectable>(mut self, target: C) -> Result<Self, MemcacheError> {
        self.inner = self.inner.add_server(target)?;
        Ok(self)
    }

    /// Set the maximum number of connections managed by the pool.
    pub fn with_max_pool_size(mut self, max_size: u32) -> Self {
        self.inner = self.inner.with_max_pool_size(max_size);
        self
    }

    /// Set the minimum number of idle connections to maintain in the pool.
    pub fn with_min_idle_conns(mut self, min_idle: u32) -> Self {
        self.inner = self.inner.with_min_idle_conns(min_idle);
        self
    }

    /// Set the maximum lifetime of connections in the pool.
    pub fn with_max_conn_lifetime(mut self, max_lifetime: Duration) -> Self {
        self.inner = self.inner.with_max_conn_lifetime(max_lifetime);
        self
    }

    /// Set the socket read timeout for TCP connections.
    pub fn with_read_timeout(mut self, read_timeout: Duration) -> Self {
        self.inner = self.inner.with_read_timeout(read_timeout);
        self
    }

    /// Set the socket write timeout for TCP connections.
    pub fn with_write_timeout(mut self, write_timeout: Duration) -> Self {
        self.inner = self.inner.with_write_timeout(write_timeout);
        self
    }

    /// Set the connection timeout for TCP connections.
    pub fn with_connection_timeout(mut self, connection_timeout: Duration) -> Self {
        self.inner = self.inner.with_connection_timeout(connection_timeout);
        self
    }

    /// Set the hash function for the client.
    pub fn with_hash_function(mut self, hash_function: fn(&str) -> u64) -> Self {
        self.inner = self.inner.with_hash_function(hash_function);
        self
    }

    /// Build the client. This will create a connection pool and return a client, or an error if the connection pool could not be created.
    pub fn build(self) -> Result<Client, MemcacheError> {
        Ok(Client {
            inner: self.inner.build()?,
        })
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    #[tokio::test]
    async fn build_client_happy_path() {
        let client = super::Client::builder()
            .add_server("memcache://localhost:12345")
            .unwrap()
            .build()
            .unwrap();
        assert!(client.version().await.unwrap()[0].1 != "");
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
            (client.inner.hash_function)("any_key"),
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
    #[tokio::test]
    async fn unix() {
        let client = super::Client::connect("memcache:///tmp/memcached.sock").unwrap();
        assert!(client.version().await.unwrap()[0].1 != "");
    }

    #[cfg(feature = "tls")]
    #[tokio::test]
    async fn ssl_noverify() {
        let client = super::Client::connect("memcache+tls://localhost:12350?verify_mode=none").unwrap();
        assert!(client.version().await.unwrap()[0].1 != "");
    }

    #[cfg(feature = "tls")]
    #[tokio::test]
    async fn ssl_verify() {
        let client = super::Client::connect(
            "memcache+tls://localhost:12350?ca_path=tests/assets/RUST_MEMCACHE_TEST_CERT.crt",
        )
        .unwrap();
        assert!(client.version().await.unwrap()[0].1 != "");
    }

    #[cfg(feature = "tls")]
    #[tokio::test]
    async fn ssl_client_certs() {
        let client = super::Client::connect("memcache+tls://localhost:12351?key_path=tests/assets/client.key&cert_path=tests/assets/client.crt&ca_path=tests/assets/RUST_MEMCACHE_TEST_CERT.crt").unwrap();
        assert!(client.version().await.unwrap()[0].1 != "");
    }

    #[tokio::test]
    async fn delete() {
        let client = super::Client::connect("memcache://localhost:12345").unwrap();
        client.set("an_exists_key", "value", 0).await.unwrap();
        assert_eq!(client.delete("an_exists_key").await.unwrap(), true);
        assert_eq!(client.delete("a_not_exists_key").await.unwrap(), false);
    }

    #[tokio::test]
    async fn increment() {
        let client = super::Client::connect("memcache://localhost:12345").unwrap();
        client.delete("counter").await.unwrap();
        client.set("counter", 321, 0).await.unwrap();
        assert_eq!(client.increment("counter", 123).await.unwrap(), 444);
    }
}
