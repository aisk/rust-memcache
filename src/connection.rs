use error::MemcacheError;
use std::io;
use std::io::{Read, Write};
use std::net::TcpStream;
#[cfg(unix)]
use std::os::unix::net::UnixStream;
use std::time::Duration;
#[cfg(unix)]
use url::Host;
use url::Url;

use stream::Stream;
use stream::UdpStream;

/// a connection to the memcached server
pub struct Connection {
    stream: Stream,
    pub url: String,
}

impl Connection {
    pub(crate) fn connect(url: &Url) -> Result<Self, MemcacheError> {
        if url.scheme() != "memcache" {
            return Err(MemcacheError::ClientError(
                "memcache URL should start with 'memcache://'".into(),
            ));
        }

        let is_udp = url
            .query_pairs()
            .any(|(ref k, ref v)| k == "udp" && v == "true");

        if is_udp {
            let udp_stream = Stream::UdpStream(UdpStream::new(url.clone())?);
            return Ok(Connection {
                url: url.to_string(),
                stream: udp_stream,
            });
        }

        #[cfg(unix)]
        {
            if url.host() == Some(Host::Domain("")) && url.port() == None {
                let stream = UnixStream::connect(url.path())?;
                return Ok(Connection {
                    url: url.to_string(),
                    stream: Stream::UnixStream(stream),
                });
            }
        }

        let stream = TcpStream::connect(url.clone())?;

        let disable_tcp_nodelay = url
            .query_pairs()
            .any(|(ref k, ref v)| k == "tcp_nodelay" && v == "false");
        if !disable_tcp_nodelay {
            stream.set_nodelay(true)?;
        }
        let timeout = url.query_pairs()
            .find(|&(ref k, ref _v)| k == "timeout")
            .and_then(|(ref _k, ref v)| v.parse::<u64>().ok())
            .map(Duration::from_secs);
        if timeout.is_some() {
            stream.set_read_timeout(timeout)?;
            stream.set_write_timeout(timeout)?;
        }
        return Ok(Connection {
            url: url.to_string(),
            stream: Stream::TcpStream(stream),
        });
    }

    pub(crate) fn set_read_timeout(&mut self, timeout: Option<Duration>) -> Result<(), MemcacheError> {
        if  let Stream::TcpStream(ref mut conn) =  self.stream {
            conn.set_read_timeout(timeout)?;
        }
        Ok(())
    }

    pub(crate) fn set_write_timeout(&mut self, timeout: Option<Duration>) -> Result<(), MemcacheError> {
        if  let Stream::TcpStream(ref mut conn) =  self.stream {
            conn.set_write_timeout(timeout)?;
        }
        Ok(())
    }
}

impl Read for Connection {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        match self.stream {
            Stream::TcpStream(ref mut stream) => stream.read(buf),
            Stream::UdpStream(ref mut stream) => stream.read(buf),
            #[cfg(unix)]
            Stream::UnixStream(ref mut stream) => stream.read(buf),
        }
    }
}

impl Write for Connection {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        match self.stream {
            Stream::TcpStream(ref mut stream) => stream.write(buf),
            Stream::UdpStream(ref mut stream) => stream.write(buf),
            #[cfg(unix)]
            Stream::UnixStream(ref mut stream) => stream.write(buf),
        }
    }

    fn flush(&mut self) -> io::Result<()> {
        match self.stream {
            Stream::TcpStream(ref mut stream) => stream.flush(),
            Stream::UdpStream(ref mut stream) => stream.flush(),
            #[cfg(unix)]
            Stream::UnixStream(ref mut stream) => stream.flush(),
        }
    }
}
