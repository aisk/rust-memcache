use std::net::{TcpStream, ToSocketAddrs};
use error::MemcacheError;

/// The connection acts as a TCP connection to the memcached server
pub struct Connection {
    pub stream: TcpStream,
}
impl Connection {
    /// connect to the memcached server with TCP connection.
    ///
    /// Example:
    ///
    /// ```rust
    /// let _ = memcache::Connection::connect("localhost:12345").unwrap();
    /// ```
    pub fn connect<A: ToSocketAddrs>(addr: A) -> Result<Self, MemcacheError> {
        let stream = TcpStream::connect(addr)?;
        return Ok(Connection { stream: stream });
    }
}
