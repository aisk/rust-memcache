use std::io::{Read, Write};
use std::net;
#[cfg(unix)]
use std::os;
#[cfg(unix)]
use std::convert::AsRef;
#[cfg(unix)]
use std::path::Path;
use error::MemcacheError;

/// The connection acts as a TCP connection to the memcached server
#[derive(Debug)]
pub struct Connection<C: Read + Write + Sized> {
    pub stream: C,
}
impl Connection<net::TcpStream> {
    /// connect to the memcached server with TCP connection.
    ///
    /// Example:
    ///
    /// ```rust
    /// let _ = memcache::Connection::connect("localhost:12345").unwrap();
    /// ```
    pub fn connect<A: net::ToSocketAddrs>(addr: A) -> Result<Self, MemcacheError> {
        let stream = net::TcpStream::connect(addr)?;
        return Ok(Connection { stream: stream });
    }
}
#[cfg(unix)]
impl Connection<os::unix::net::UnixStream> {
    /// connect to the memcached server with UNIX domain sockt connection.
    ///
    /// Example:
    ///
    /// ```rust
    /// let _ = memcache::Connection::open("/tmp/memcached.sock").unwrap();
    /// ```
    pub fn open<P: AsRef<Path>>(path: P) -> Result<Self, MemcacheError> {
        let stream = os::unix::net::UnixStream::connect(path)?;
        return Ok(Connection { stream: stream });
    }
}

impl<C: Read + Write + Sized> Connection<C> {
    pub fn from_io(io: C) -> Result<Self, MemcacheError> {
        return Ok(Connection { stream: io });
    }
}
