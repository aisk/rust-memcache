use std::io::BufReader;
use std::net::TcpStream;
#[cfg(unix)]
use std::os::unix::net::UnixStream;
use std::time::Duration;
#[cfg(unix)]
use url::Host;
use url::Url;

use error::MemcacheError;
use protocol::{AsciiProtocol, BinaryProtocol, Protocol};
use stream::Stream;
use stream::UdpStream;

/// a connection to the memcached server
pub struct Connection {
    pub protocol: Protocol,
    pub url: String,
}

enum Transport {
    TCP(TCPOptions),
    UDP,
    #[cfg(unix)]
    Unix,
}

struct TCPOptions {
    timeout: Option<Duration>,
    nodelay: bool,
}

impl TCPOptions {
    fn from_url(url: &Url) -> Self {
        let nodelay = !url
            .query_pairs()
            .any(|(ref k, ref v)| k == "tcp_nodelay" && v == "false");
        let timeout = url
            .query_pairs()
            .find(|&(ref k, ref _v)| k == "timeout")
            .and_then(|(ref _k, ref v)| v.parse::<u64>().ok())
            .map(Duration::from_secs);
        TCPOptions {
            nodelay: nodelay,
            timeout: timeout,
        }
    }
}

impl Transport {
    fn from_url(url: &Url) -> Result<Self, MemcacheError> {
        let parts: Vec<&str> = url.scheme().split("+").collect();
        if parts.len() != 1 && parts.len() != 2 || parts[0] != "memcache" {
            return Err(MemcacheError::ClientError(
                "memcache URL's scheme should start with 'memcache'".into(),
            ));
        }

        // scheme has highest priority
        if parts.len() == 2 {
            return match parts[1] {
                "tcp" => Ok(Transport::TCP(TCPOptions::from_url(url))),
                "udp" => Ok(Transport::UDP),
                #[cfg(unix)]
                "unix" => Ok(Transport::Unix),
                // "tls" => Ok(Transport::TLS),
                _ => Err(MemcacheError::ClientError(
                    "memcache URL's scheme should be 'memcache+tcp' or 'memcache+udp' or 'memcache+unix' or 'memcache+tls'".into(),
                )),
            };
        }

        let is_udp = url.query_pairs().any(|(ref k, ref v)| k == "udp" && v == "true");
        if is_udp {
            return Ok(Transport::UDP)
        }

        #[cfg(unix)]
        {
            if url.host() == Some(Host::Domain("")) && url.port() == None {
                return Ok(Transport::Unix)
            }
        }

        Ok(Transport::TCP(TCPOptions::from_url(url)))
    }
}

impl Connection {
    pub(crate) fn connect(url: &Url) -> Result<Self, MemcacheError> {
        let transport = Transport::from_url(url)?;
        let is_ascii =  url.query_pairs().any(|(ref k, ref v)| k == "protocol" && v == "ascii");
        let stream: Stream = match transport {
            Transport::TCP(options) => {
                let tcp_stream = TcpStream::connect(url.clone())?;
                if options.timeout.is_some() {
                    tcp_stream.set_read_timeout(options.timeout)?;
                    tcp_stream.set_write_timeout(options.timeout)?;
                }
                tcp_stream.set_nodelay(options.nodelay)?;
                Stream::Tcp(tcp_stream)
            },
            Transport::UDP => {
                Stream::Udp(UdpStream::new(url.clone())?)
            },
            #[cfg(unix)]
            Transport::Unix => {
                Stream::Unix(UnixStream::connect(url.path())?)
            },
        };

        let protocol = if is_ascii {
            Protocol::Ascii(AsciiProtocol {
                reader: BufReader::new(stream),
            })
        } else {
            Protocol::Binary(BinaryProtocol {
                stream: stream,
            })
        };

        Ok(Connection {
            url: url.to_string(),
            protocol: protocol,
        })
    }
}
