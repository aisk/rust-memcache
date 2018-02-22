use std::borrow::Cow;
use std::io;
use std::io::{Read, Write};
use std::net::TcpStream;
#[cfg(unix)]
use std::os::unix::net::UnixStream;
use url::Url;
#[cfg(unix)]
use url::Host;
use error::MemcacheError;

enum Stream {
    TcpStream(TcpStream),
    #[cfg(unix)] UnixStream(UnixStream),
}

/// The connection acts as a TCP connection to the memcached server
pub struct Connection {
    stream: Stream,
    pub url: String,
}

impl Connection {
    pub fn connect(addr: &str) -> Result<Self, MemcacheError> {
        let addr = match Url::parse(addr) {
            Ok(v) => v,
            Err(_) => return Err(MemcacheError::ClientError("Invalid memcache URL".into())),
        };
        if addr.scheme() != "memcache" {
            return Err(MemcacheError::ClientError(
                "memcache URL should start with 'memcache://'".into(),
            ));
        }
        #[cfg(unix)]
        {
            if addr.host() == Some(Host::Domain("")) && addr.port() == None {
                let stream = UnixStream::connect(addr.path())?;
                return Ok(Connection {
                    url: addr.into_string(),
                    stream: Stream::UnixStream(stream),
                });
            }
        }
        let stream = TcpStream::connect(addr.clone())?;
        let tcp_nodelay = addr.query_pairs()
            .find(|&(ref k, ref v)| {
                k == &Cow::Borrowed("tcp_nodelay") && v == &Cow::Borrowed("true")
            })
            .is_some();
        stream.set_nodelay(tcp_nodelay)?;
        return Ok(Connection {
            url: addr.into_string(),
            stream: Stream::TcpStream(stream),
        });
    }
}

impl Read for Connection {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        match self.stream {
            Stream::TcpStream(ref mut stream) => stream.read(buf),
            #[cfg(unix)]
            Stream::UnixStream(ref mut stream) => stream.read(buf),
        }
    }
}

impl Write for Connection {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        match self.stream {
            Stream::TcpStream(ref mut stream) => stream.write(buf),
            #[cfg(unix)]
            Stream::UnixStream(ref mut stream) => stream.write(buf),
        }
    }

    fn flush(&mut self) -> io::Result<()> {
        match self.stream {
            Stream::TcpStream(ref mut stream) => stream.flush(),
            #[cfg(unix)]
            Stream::UnixStream(ref mut stream) => stream.flush(),
        }
    }
}
