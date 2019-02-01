use std::net::TcpStream;
use std::time::Duration;
#[cfg(unix)]
use std::os::unix::net::UnixStream;
#[cfg(unix)]
use url::Host;
use url::Url;

use error::MemcacheError;
use stream::Stream;
use stream::UdpStream;
use protocol::{Protocol, BinaryProtocol};

/// a connection to the memcached server
pub struct Connection {
    pub protocol: Protocol,
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
            let udp_stream = Stream::Udp(UdpStream::new(url.clone())?);
            return Ok(Connection {
                url: url.to_string(),
                protocol: Protocol::Binary(BinaryProtocol{stream: udp_stream}),
            });
        }

        #[cfg(unix)]
        {
            if url.host() == Some(Host::Domain("")) && url.port() == None {
                let stream = UnixStream::connect(url.path())?;
                return Ok(Connection {
                    url: url.to_string(),
                    protocol: Protocol::Binary(BinaryProtocol{stream: Stream::Unix(stream)}),
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
            protocol: Protocol::Binary(BinaryProtocol{stream: Stream::Tcp(stream)}),
        });
    }

}
