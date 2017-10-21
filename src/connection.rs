use std::io;
use std::io::{Read, Write};
use std::net::TcpStream;
#[cfg(unix)]
use std::os::unix::net::UnixStream;

/// The connection acts as a TCP connection to the memcached server
pub enum Connection {
    TcpStream(TcpStream),
    #[cfg(unix)]
    UnixStream(UnixStream),
}

impl Read for Connection {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        match *self {
            Connection::TcpStream(ref mut stream) => stream.read(buf),
            #[cfg(unix)]
            Connection::UnixStream(ref mut stream) => stream.read(buf),
        }
    }
}

impl Write for Connection {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        match *self {
            Connection::TcpStream(ref mut stream) => stream.write(buf),
            #[cfg(unix)]
            Connection::UnixStream(ref mut stream) => stream.write(buf),
        }
    }

    fn flush(&mut self) -> io::Result<()> {
        match *self {
            Connection::TcpStream(ref mut stream) => stream.flush(),
            #[cfg(unix)]
            Connection::UnixStream(ref mut stream) => stream.flush(),
        }
    }
}
