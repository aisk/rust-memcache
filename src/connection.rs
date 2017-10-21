use std::io;
use std::io::{Read, Write};
use std::net::{TcpStream, ToSocketAddrs};
use error::MemcacheError;

/// The connection acts as a TCP connection to the memcached server
pub enum Connection {
    TcpStream(TcpStream),
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
        return Ok(Connection::TcpStream(stream));
    }
}

impl Read for Connection {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        match *self {
            Connection::TcpStream(ref mut tcp_stream) => tcp_stream.read(buf),
        }
    }
}

impl Write for Connection {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        match *self {
            Connection::TcpStream(ref mut tcp_stream) => tcp_stream.write(buf),
        }
    }

    fn flush(&mut self) -> io::Result<()> {
        match *self {
            Connection::TcpStream(ref mut tcp_stream) => tcp_stream.flush(),
        }
    }
}
