use std::fmt;
use std::io::BufRead;
use std::io::Write;
use std::io::Read;
use std::io;
use std::net;
#[cfg(unix)]
use std::os;
#[cfg(unix)]
use std::convert::AsRef;
#[cfg(unix)]
use std::path::Path;
use options::Options;
use value::{ToMemcacheValue, FromMemcacheValue};
use error::{MemcacheError, is_memcache_error};

/// The connection acts as a TCP connection to the memcached server
#[derive(Debug)]
pub struct Connection<C: Read + Write + Sized> {
    pub reader: io::BufReader<C>,
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
        return Ok(Connection { reader: io::BufReader::new(stream) });
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
        return Ok(Connection { reader: io::BufReader::new(stream) });
    }
}

enum StoreCommand {
    Set,
    Add,
    Replace,
    Append,
    Prepend,
}

impl fmt::Display for StoreCommand {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match *self {
            StoreCommand::Set => write!(f, "set"),
            StoreCommand::Add => write!(f, "add"),
            StoreCommand::Replace => write!(f, "replace"),
            StoreCommand::Append => write!(f, "append"),
            StoreCommand::Prepend => write!(f, "prepend"),
        }
    }
}

impl<C: Read + Write + Sized> Connection<C> {
    pub fn from_io(io: C) -> Result<Self, MemcacheError> {
        return Ok(Connection { reader: io::BufReader::new(io) });
    }
}
