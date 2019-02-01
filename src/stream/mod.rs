mod udp_stream;

use std::io::{self, Read, Write};
use std::net::TcpStream;
use std::time::Duration;
#[cfg(unix)]
use std::os::unix::net::UnixStream;

use error::MemcacheError;

pub(crate) use self::udp_stream::UdpStream;

pub enum Stream {
    Tcp(TcpStream),
    Udp(UdpStream),
    #[cfg(unix)]
    Unix(UnixStream),
}

impl Stream {
    pub(crate) fn set_read_timeout(&mut self, timeout: Option<Duration>) -> Result<(), MemcacheError> {
        if  let Stream::Tcp(ref mut conn) =  self {
            conn.set_read_timeout(timeout)?;
        }
        Ok(())
    }

    pub(crate) fn set_write_timeout(&mut self, timeout: Option<Duration>) -> Result<(), MemcacheError> {
        if  let Stream::Tcp(ref mut conn) =  self {
            conn.set_write_timeout(timeout)?;
        }
        Ok(())
    }
}

impl Read for Stream {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        match self {
            Stream::Tcp(ref mut stream) => stream.read(buf),
            Stream::Udp(ref mut stream) => stream.read(buf),
            #[cfg(unix)]
            Stream::Unix(ref mut stream) => stream.read(buf),
        }
    }
}

impl Write for Stream {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        match self {
            Stream::Tcp(ref mut stream) => stream.write(buf),
            Stream::Udp(ref mut stream) => stream.write(buf),
            #[cfg(unix)]
            Stream::Unix(ref mut stream) => stream.write(buf),
        }
    }

    fn flush(&mut self) -> io::Result<()> {
        match self {
            Stream::Tcp(ref mut stream) => stream.flush(),
            Stream::Udp(ref mut stream) => stream.flush(),
            #[cfg(unix)]
            Stream::Unix(ref mut stream) => stream.flush(),
        }
    }
}
