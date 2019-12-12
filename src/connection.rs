use std::io::BufReader;
use std::net::TcpStream;
#[cfg(unix)]
use std::os::unix::net::UnixStream;
use std::time::Duration;
#[cfg(unix)]
use url::Host;
use url::Url;

use error::MemcacheError;

#[cfg(feature = "tls")]
use openssl::ssl::{SslConnector, SslFiletype, SslMethod, SslVerifyMode};
use protocol::{AsciiProtocol, BinaryProtocol, Protocol};
use stream::Stream;
use stream::UdpStream;

/// a connection to the memcached server
pub struct Connection {
    pub protocol: Protocol,
    pub url: String,
}

enum Transport {
    Tcp(TcpOptions),
    Udp,
    #[cfg(unix)]
    Unix,
    #[cfg(feature = "tls")]
    Tls(TlsOptions),
}

#[cfg(feature = "tls")]
struct TlsOptions {
    tcp_options: TcpOptions,
    ca_path: Option<String>,
    key_path: Option<String>,
    cert_path: Option<String>,
    verify_mode: SslVerifyMode,
}

struct TcpOptions {
    timeout: Option<Duration>,
    nodelay: bool,
}

#[cfg(feature = "tls")]
fn get_param(url: &Url, key: &str) -> Option<String> {
    return url
        .query_pairs()
        .find(|&(ref k, ref _v)| k == key)
        .map(|(_k, v)| v.to_string());
}

#[cfg(feature = "tls")]
impl TlsOptions {
    fn from_url(url: &Url) -> Result<Self, MemcacheError> {
        let verify_mode = match get_param(url, "verify_mode").as_ref().map(String::as_str) {
            Some("none") => SslVerifyMode::NONE,
            Some("peer") => SslVerifyMode::PEER,
            Some(_) => {
                return Err(MemcacheError::BadURL(
                    "unknown verify_mode, expected 'none' or 'peer'".into(),
                ))
            }
            None => SslVerifyMode::PEER,
        };

        let ca_path = get_param(url, "ca_path");
        let key_path = get_param(url, "key_path");
        let cert_path = get_param(url, "cert_path");

        if key_path.is_some() && cert_path.is_none() {
            return Err(MemcacheError::BadURL(
                "cert_path must be specified when key_path is specified".into(),
            ));
        } else if key_path.is_none() && cert_path.is_some() {
            return Err(MemcacheError::BadURL(
                "key_path must be specified when cert_path is specified".into(),
            ));
        }

        Ok(TlsOptions {
            tcp_options: TcpOptions::from_url(url),
            ca_path: ca_path,
            key_path: key_path,
            cert_path: cert_path,
            verify_mode: verify_mode,
        })
    }
}

impl TcpOptions {
    fn from_url(url: &Url) -> Self {
        let nodelay = !url
            .query_pairs()
            .any(|(ref k, ref v)| k == "tcp_nodelay" && v == "false");
        let timeout = url
            .query_pairs()
            .find(|&(ref k, ref _v)| k == "timeout")
            .and_then(|(ref _k, ref v)| v.parse::<u64>().ok())
            .map(Duration::from_secs);
        TcpOptions {
            nodelay: nodelay,
            timeout: timeout,
        }
    }
}

impl Transport {
    fn from_url(url: &Url) -> Result<Self, MemcacheError> {
        let parts: Vec<&str> = url.scheme().split("+").collect();
        if parts.len() != 1 && parts.len() != 2 || parts[0] != "memcache" {
            return Err(MemcacheError::BadURL(
                "memcache URL's scheme should start with 'memcache'".into(),
            ));
        }

        // scheme has highest priority
        if parts.len() == 2 {
            return match parts[1] {
                "tcp" => Ok(Transport::Tcp(TcpOptions::from_url(url))),
                "udp" => Ok(Transport::Udp),
                #[cfg(unix)]
                "unix" => Ok(Transport::Unix),
                #[cfg(feature = "tls")]
                "tls" => Ok(Transport::Tls(TlsOptions::from_url(url)?)),
                _ => Err(MemcacheError::BadURL(
                    "memcache URL's scheme should be 'memcache+tcp' or 'memcache+udp' or 'memcache+unix' or 'memcache+tls'".into(),
                )),
            };
        }

        let is_udp = url.query_pairs().any(|(ref k, ref v)| k == "udp" && v == "true");
        if is_udp {
            return Ok(Transport::Udp);
        }

        #[cfg(unix)]
        {
            if url.host() == Some(Host::Domain("")) && url.port() == None {
                return Ok(Transport::Unix);
            }
        }

        Ok(Transport::Tcp(TcpOptions::from_url(url)))
    }
}

fn tcp_stream(url: &Url, opts: &TcpOptions) -> Result<TcpStream, MemcacheError> {
    let tcp_stream = TcpStream::connect(url.clone())?;
    if opts.timeout.is_some() {
        tcp_stream.set_read_timeout(opts.timeout)?;
        tcp_stream.set_write_timeout(opts.timeout)?;
    }
    tcp_stream.set_nodelay(opts.nodelay)?;
    Ok(tcp_stream)
}

impl Connection {
    pub(crate) fn connect(url: &Url) -> Result<Self, MemcacheError> {
        let transport = Transport::from_url(url)?;
        let is_ascii = url.query_pairs().any(|(ref k, ref v)| k == "protocol" && v == "ascii");
        let stream: Stream = match transport {
            Transport::Tcp(options) => Stream::Tcp(tcp_stream(url, &options)?),
            Transport::Udp => Stream::Udp(UdpStream::new(url.clone())?),
            #[cfg(unix)]
            Transport::Unix => Stream::Unix(UnixStream::connect(url.path())?),
            #[cfg(feature = "tls")]
            Transport::Tls(options) => {
                let host = url
                    .host_str()
                    .ok_or(MemcacheError::BadURL("host required for TLS connection".into()))?;

                let mut builder = SslConnector::builder(SslMethod::tls())?;
                builder.set_verify(options.verify_mode);

                if options.ca_path.is_some() {
                    builder.set_ca_file(&options.ca_path.unwrap())?;
                }

                if options.key_path.is_some() {
                    builder.set_private_key_file(options.key_path.unwrap(), SslFiletype::PEM)?;
                }

                if options.cert_path.is_some() {
                    builder.set_certificate_chain_file(options.cert_path.unwrap())?;
                }

                let tls_conn = builder.build();
                let tcp_stream = tcp_stream(url, &options.tcp_options)?;
                let tls_stream = tls_conn.connect(host, tcp_stream)?;
                Stream::Tls(tls_stream)
            }
        };

        let protocol = if is_ascii {
            Protocol::Ascii(AsciiProtocol {
                reader: BufReader::new(stream),
            })
        } else {
            Protocol::Binary(BinaryProtocol { stream: stream })
        };

        Ok(Connection {
            url: url.to_string(),
            protocol: protocol,
        })
    }
}
