use client::Stats;
use std::collections::HashMap;
use std::net::TcpStream;
#[cfg(unix)]
use std::os::unix::net::UnixStream;
use std::sync::Arc;
use std::time::Duration;
use url::Url;

use error::MemcacheError;

#[cfg(feature = "tls")]
use openssl::ssl::{SslConnector, SslFiletype, SslMethod, SslVerifyMode};
use protocol::{AsciiProtocol, BinaryProtocol, Protocol, ProtocolTrait};
use r2d2::ManageConnection;
use stream::Stream;
use stream::UdpStream;
use value::{FromMemcacheValueExt, ToMemcacheValue};

/// A connection to the memcached server
pub struct Connection {
    pub protocol: Protocol,
    is_dirty: bool,
    pub url: Arc<String>,
}

#[derive(Debug)]
pub(crate) struct ConnCustomizer {
    read_timeout: Option<Duration>,
    write_timeout: Option<Duration>,
}

impl r2d2::CustomizeConnection<Connection, MemcacheError> for ConnCustomizer {
    fn on_acquire(&self, conn: &mut Connection) -> Result<(), MemcacheError> {
        conn.set_read_timeout(self.read_timeout)?;
        conn.set_write_timeout(self.write_timeout)?;
        Ok(())
    }
}

impl ConnCustomizer {
    pub(crate) fn new(read_timeout: Option<Duration>, write_timeout: Option<Duration>) -> Self {
        Self {
            read_timeout,
            write_timeout,
        }
    }
}

pub(crate) struct ConnectionManager {
    url: Url,
}

impl ConnectionManager {
    pub(crate) fn new(url: Url) -> Self {
        Self { url }
    }
}

impl ManageConnection for ConnectionManager {
    type Connection = Connection;
    type Error = MemcacheError;

    fn connect(&self) -> Result<Self::Connection, Self::Error> {
        let url = &self.url;
        let mut connection = Connection::connect(url)?;
        if url.has_authority() && !url.username().is_empty() && url.password().is_some() {
            let username = url.username();
            let password = url.password().unwrap();
            connection.auth(username, password)?;
        }
        Ok(connection)
    }

    fn is_valid(&self, conn: &mut Self::Connection) -> Result<(), Self::Error> {
        conn.version().map(|_| ())
    }

    fn has_broken(&self, conn: &mut Self::Connection) -> bool {
        conn.is_dirty
    }
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
        let mut parts = url.scheme().splitn(2, "+");
        match parts.next() {
            Some(part) if part == "memcache" => (),
            _ => {
                return Err(MemcacheError::BadURL(
                    "memcache URL's scheme should start with 'memcache'".into(),
                ))
            }
        }

        // scheme has highest priority
        if let Some(proto) = parts.next() {
            return match proto {
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
            if url.host().is_none() && url.port() == None {
                return Ok(Transport::Unix);
            }
        }

        Ok(Transport::Tcp(TcpOptions::from_url(url)))
    }
}

fn tcp_stream(url: &Url, opts: &TcpOptions) -> Result<TcpStream, MemcacheError> {
    let tcp_stream = TcpStream::connect(&*url.socket_addrs(|| None)?)?;
    if opts.timeout.is_some() {
        tcp_stream.set_read_timeout(opts.timeout)?;
        tcp_stream.set_write_timeout(opts.timeout)?;
    }
    tcp_stream.set_nodelay(opts.nodelay)?;
    Ok(tcp_stream)
}

impl ProtocolTrait for Connection {
    fn auth(&mut self, username: &str, password: &str) -> Result<(), MemcacheError> {
        self.protocol.auth(username, password).map_err(|e| {
            self.is_dirty = !e.is_recoverable();
            e
        })
    }

    fn version(&mut self) -> Result<String, MemcacheError> {
        self.protocol.version().map_err(|e| {
            self.is_dirty = !e.is_recoverable();
            e
        })
    }

    fn flush(&mut self) -> Result<(), MemcacheError> {
        self.protocol.flush().map_err(|e| {
            self.is_dirty = !e.is_recoverable();
            e
        })
    }
    fn flush_with_delay(&mut self, delay: u32) -> Result<(), MemcacheError> {
        self.protocol.flush_with_delay(delay).map_err(|e| {
            self.is_dirty = !e.is_recoverable();
            e
        })
    }

    fn get<V: FromMemcacheValueExt>(&mut self, key: &str) -> Result<Option<V>, MemcacheError> {
        self.protocol.get(key).map_err(|e| {
            self.is_dirty = !e.is_recoverable();
            e
        })
    }
    fn gets<V: FromMemcacheValueExt>(&mut self, keys: &[&str]) -> Result<HashMap<String, V>, MemcacheError> {
        self.protocol.gets(keys).map_err(|e| {
            self.is_dirty = !e.is_recoverable();
            e
        })
    }
    fn set<V: ToMemcacheValue<Stream>>(&mut self, key: &str, value: V, expiration: u32) -> Result<(), MemcacheError> {
        self.protocol.set(key, value, expiration).map_err(|e| {
            self.is_dirty = !e.is_recoverable();
            e
        })
    }
    fn cas<V: ToMemcacheValue<Stream>>(
        &mut self,
        key: &str,
        value: V,
        expiration: u32,
        cas: u64,
    ) -> Result<bool, MemcacheError> {
        self.protocol.cas(key, value, expiration, cas).map_err(|e| {
            self.is_dirty = !e.is_recoverable();
            e
        })
    }
    fn add<V: ToMemcacheValue<Stream>>(&mut self, key: &str, value: V, expiration: u32) -> Result<(), MemcacheError> {
        self.protocol.add(key, value, expiration).map_err(|e| {
            self.is_dirty = !e.is_recoverable();
            e
        })
    }
    fn replace<V: ToMemcacheValue<Stream>>(
        &mut self,
        key: &str,
        value: V,
        expiration: u32,
    ) -> Result<(), MemcacheError> {
        self.protocol.replace(key, value, expiration).map_err(|e| {
            self.is_dirty = !e.is_recoverable();
            e
        })
    }
    fn append<V: ToMemcacheValue<Stream>>(&mut self, key: &str, value: V) -> Result<(), MemcacheError> {
        self.protocol.append(key, value).map_err(|e| {
            self.is_dirty = !e.is_recoverable();
            e
        })
    }
    fn prepend<V: ToMemcacheValue<Stream>>(&mut self, key: &str, value: V) -> Result<(), MemcacheError> {
        self.protocol.prepend(key, value).map_err(|e| {
            self.is_dirty = !e.is_recoverable();
            e
        })
    }
    fn delete(&mut self, key: &str) -> Result<bool, MemcacheError> {
        self.protocol.delete(key).map_err(|e| {
            self.is_dirty = !e.is_recoverable();
            e
        })
    }
    fn increment(&mut self, key: &str, amount: u64) -> Result<u64, MemcacheError> {
        self.protocol.increment(key, amount).map_err(|e| {
            self.is_dirty = !e.is_recoverable();
            e
        })
    }
    fn decrement(&mut self, key: &str, amount: u64) -> Result<u64, MemcacheError> {
        self.protocol.decrement(key, amount).map_err(|e| {
            self.is_dirty = !e.is_recoverable();
            e
        })
    }
    fn touch(&mut self, key: &str, expiration: u32) -> Result<bool, MemcacheError> {
        self.protocol.touch(key, expiration).map_err(|e| {
            self.is_dirty = !e.is_recoverable();
            e
        })
    }
    fn stats(&mut self) -> Result<Stats, MemcacheError> {
        self.protocol.stats().map_err(|e| {
            self.is_dirty = !e.is_recoverable();
            e
        })
    }
}

impl Connection {
    pub(crate) fn set_write_timeout(&mut self, timeout: Option<Duration>) -> Result<(), MemcacheError> {
        match self.protocol {
            Protocol::Ascii(ref mut protocol) => protocol.stream().set_write_timeout(timeout)?,
            Protocol::Binary(ref mut protocol) => protocol.stream.set_write_timeout(timeout)?,
        }
        Ok(())
    }

    pub(crate) fn set_read_timeout(&mut self, timeout: Option<Duration>) -> Result<(), MemcacheError> {
        match self.protocol {
            Protocol::Ascii(ref mut protocol) => protocol.stream().set_read_timeout(timeout)?,
            Protocol::Binary(ref mut protocol) => protocol.stream.set_read_timeout(timeout)?,
        }
        Ok(())
    }

    pub(crate) fn get_url(&self) -> String {
        self.url.to_string()
    }

    pub(crate) fn connect(url: &Url) -> Result<Self, MemcacheError> {
        let transport = Transport::from_url(url)?;
        let is_ascii = url.query_pairs().any(|(ref k, ref v)| k == "protocol" && v == "ascii");
        let stream: Stream = match transport {
            Transport::Tcp(options) => Stream::Tcp(tcp_stream(url, &options)?),
            Transport::Udp => Stream::Udp(UdpStream::new(url)?),
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
            Protocol::Ascii(AsciiProtocol::new(stream))
        } else {
            Protocol::Binary(BinaryProtocol { stream: stream })
        };

        Ok(Connection {
            url: Arc::new(url.to_string()),
            is_dirty: false,
            protocol: protocol,
        })
    }
}

#[cfg(test)]
mod tests {
    #[cfg(unix)]
    #[test]
    fn test_transport_url() {
        use super::Transport;
        use url::Url;
        match Transport::from_url(&Url::parse("memcache:///tmp/memcached.sock").unwrap()).unwrap() {
            Transport::Unix => (),
            _ => assert!(false, "transport is not unix"),
        }
    }
}
