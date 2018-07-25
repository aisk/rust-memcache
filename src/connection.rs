use std::io;
use std::io::{Read, Write, ErrorKind, Error};
use std::net::TcpStream;
#[cfg(unix)]
use std::os::unix::net::UnixStream;
use url::Url;
#[cfg(unix)]
use url::Host;
use error::MemcacheError;
use std::net::UdpSocket;
use rand;
use byteorder::{BigEndian, WriteBytesExt, ByteOrder};
use std::collections::HashMap;

enum Stream {
    TcpStream(TcpStream),
    UdpSocket(UdpStream),
    #[cfg(unix)] UnixStream(UnixStream),
}

pub enum ConnectionType {
    TCP = 0,
    UDP = 1,
    UNIXSTREAM = 2,
}

struct UdpStream {
    socket: UdpSocket,
    read_buf: Vec<u8>,
    write_buf: Vec<u8>,
    request_id: u16,
}

/// a connection to the memcached server
pub struct Connection {
    stream: Stream,
    pub url: String,
}

impl Connection {
    pub fn connect(addr: &str, connection_type: u32) -> Result<Self, MemcacheError> {
        let addr = match Url::parse(addr) {
            Ok(v) => v,
            Err(_) => return Err(MemcacheError::ClientError("Invalid memcache URL".into())),
        };
        if addr.scheme() != "memcache" {
            return Err(MemcacheError::ClientError(
                "memcache URL should start with 'memcache://'".into(),
            ));
        }
        if connection_type == (ConnectionType::UDP as u32) {
            let socket = UdpSocket::bind("0.0.0.0:0")?;
            socket.connect(addr.clone())?;
            return Ok(Connection {
                stream: Stream::UdpSocket(UdpStream {
                    socket: socket,
                    read_buf: Vec::new(),
                    write_buf: Vec::new(),
                    request_id:  rand::random::<u16>()
                }),
                url: addr.into_string()
            });
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
            .find(|&(ref k, ref v)| k == "tcp_nodelay" && v == "true")
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
            Stream::UdpSocket(ref mut stream) => {
                let mut buf_len = buf.len();
                if buf_len > stream.read_buf.len() {
                    buf_len = stream.read_buf.len();
                }
                buf[0..buf_len].copy_from_slice(&(stream.read_buf[0..buf_len]));
                stream.read_buf.drain(0..buf_len);
                Ok(buf_len)
            }
            #[cfg(unix)]
            Stream::UnixStream(ref mut stream) => stream.read(buf),
        }
    }
}

impl Write for Connection {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        match self.stream {
            Stream::TcpStream(ref mut stream) => stream.write(buf),
            Stream::UdpSocket(ref mut stream) => { 
                stream.write_buf.extend_from_slice(buf);
                Ok(buf.len())
            }
            #[cfg(unix)]
            Stream::UnixStream(ref mut stream) => stream.write(buf),
        }
    }

    fn flush(&mut self) -> io::Result<()> {
        match self.stream {
            Stream::TcpStream(ref mut stream) => stream.flush(),
            Stream::UdpSocket(ref mut stream) => { 
                //udp header is 8 bytes in the begining of each datagram
                let mut udp_header: Vec<u8> = Vec::new();
                udp_header.write_u16::<BigEndian>(stream.request_id)?; // request id to uniquely identify response for this request
                udp_header.write_u16::<BigEndian>(0)?; // 0 indicates this is the first datagram for this request
                udp_header.write_u16::<BigEndian>(1)?; // total datagrams in this request (requests can only be 1 datagram long)
                udp_header.write_u16::<BigEndian>(256)?; // reserved bytes (tested to also work with zero)
                udp_header.reverse();
                udp_header.iter().for_each(|i| stream.write_buf.insert(0, *i));
                stream.socket.send(stream.write_buf.as_slice())?;
                stream.write_buf.clear(); // clear the buffer for the next command

                let mut response_datagrams: HashMap<u16, Vec<u8>> = HashMap::new();
                let mut total_datagrams;
                let mut remaining_datagrams = 0;
                stream.read_buf.clear();
                loop { // for large values, response can span multiple datagrams, so gather them all
                    let mut buf: [u8 ; 1400] = [0 ; 1400]; // memcache udp response payload can not be longer than 1400 bytes
                    let mut bytes_read = stream.socket.recv(&mut buf)?;
                    if bytes_read < 8 {
                        // make an error here to avoid panic below
                        return Err(Error::new(ErrorKind::Other, "Invalid UDP header received"));
                    }

                    let request_id = BigEndian::read_u16(&mut buf[0..]);
                    if stream.request_id != request_id {
                        continue;
                    }
                    let sequence_no = BigEndian::read_u16(&mut buf[2..]);
                    total_datagrams = BigEndian::read_u16(&mut buf[4..]);
                    if remaining_datagrams == 0 {
                        remaining_datagrams = total_datagrams;
                    }

                    let mut v: Vec<u8> = Vec::new();
                    v.extend_from_slice(&mut buf[8..bytes_read]);
                    response_datagrams.insert(sequence_no, v);
                    remaining_datagrams = remaining_datagrams - 1;
                    if remaining_datagrams == 0 {
                        break;
                    }
                }
                for i in 0..total_datagrams {
                    stream.read_buf.append(&mut (response_datagrams[&i].clone()));
                }
                stream.request_id = stream.request_id + 1;
                Ok(())
            }
            #[cfg(unix)]
            Stream::UnixStream(ref mut stream) => stream.flush(),
        }
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn tcp_nodelay() {
        super::Connection::connect("memcache://localhost:12345?tcp_nodelay=true", super::ConnectionType::TCP as u32).unwrap();
    }
}
