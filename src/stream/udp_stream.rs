use crate::error::MemcacheError;
use byteorder::{BigEndian, ByteOrder, WriteBytesExt};
use std::collections::HashMap;
use std::io;
use std::io::{Error, ErrorKind, Read, Write};
use std::net::UdpSocket;
use std::time::Duration;
use std::u16;
use url::Url;

pub struct UdpStream {
    socket: UdpSocket,
    read_buf: Vec<u8>,
    write_buf: Vec<u8>,
    request_id: u16,
}

impl UdpStream {
    pub fn new(addr: &Url, bind_addr: Option<&str>) -> Result<Self, MemcacheError> {
        let remote_addrs = addr.socket_addrs(|| None)?;

        let bind = match bind_addr {
            Some(addr) => format!("{}:0", addr),
            None => {
                // Auto-detect: use loopback if target is loopback
                if remote_addrs.iter().any(|a| a.ip().is_loopback()) {
                    "127.0.0.1:0".to_string()
                } else {
                    "0.0.0.0:0".to_string()
                }
            }
        };

        let socket = UdpSocket::bind(&bind)?;
        socket.connect(&*remote_addrs)?;
        return Ok(UdpStream {
            socket,
            read_buf: Vec::new(),
            write_buf: Vec::new(),
            request_id: rand::random::<u16>(),
        });
    }

    pub(crate) fn set_read_timeout(&self, duration: Option<Duration>) -> Result<(), MemcacheError> {
        Ok(self.socket.set_read_timeout(duration)?)
    }

    pub(crate) fn set_write_timeout(&self, duration: Option<Duration>) -> Result<(), MemcacheError> {
        Ok(self.socket.set_write_timeout(duration)?)
    }
}

impl Read for UdpStream {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let mut buf_len = buf.len();
        if buf_len > self.read_buf.len() {
            buf_len = self.read_buf.len();
        }
        buf[0..buf_len].copy_from_slice(&(self.read_buf[0..buf_len]));
        self.read_buf.drain(0..buf_len);
        Ok(buf_len)
    }
}

impl Write for UdpStream {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.write_buf.extend_from_slice(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        // udp header is 8 bytes in the begining of each datagram
        let mut udp_header: Vec<u8> = Vec::new();

        udp_header.write_u16::<BigEndian>(self.request_id)?; // request id to uniquely identify response for this request
        udp_header.write_u16::<BigEndian>(0)?; // 0 indicates this is the first datagram for this request
        udp_header.write_u16::<BigEndian>(1)?; // total datagrams in this request (requests can only be 1 datagram long)
        udp_header.write_u16::<BigEndian>(0)?; // reserved bytes
        self.write_buf.splice(0..0, udp_header.iter().cloned());
        self.socket.send(self.write_buf.as_slice())?;
        self.write_buf.clear(); // clear the buffer for the next command

        let mut response_datagrams: HashMap<u16, Vec<u8>> = HashMap::new();
        let mut total_datagrams = 0;
        self.read_buf.clear();
        loop {
            // for large values, response can span multiple datagrams, so gather them all
            let mut buf: [u8; 1400] = [0; 1400]; // memcache udp response payload can not be longer than 1400 bytes
            let bytes_read = self.socket.recv(&mut buf)?;
            if bytes_read < 8 {
                // make an error here to avoid panic below
                return Err(Error::new(ErrorKind::Other, "Invalid UDP header received"));
            }

            let request_id = BigEndian::read_u16(&buf[0..]);
            if self.request_id != request_id {
                // ideally this shouldn't happen as we wait to read out response before sending another request
                continue;
            }
            let sequence_no = BigEndian::read_u16(&buf[2..]);
            let datagrams = BigEndian::read_u16(&buf[4..]);
            if datagrams == 0 || sequence_no >= datagrams || (total_datagrams != 0 && datagrams != total_datagrams) {
                return Err(Error::new(ErrorKind::InvalidData, "Invalid UDP header received"));
            }
            total_datagrams = datagrams;

            response_datagrams.insert(sequence_no, buf[8..bytes_read].to_vec());
            if response_datagrams.len() == usize::from(total_datagrams) {
                break;
            }
        }
        for i in 0..total_datagrams {
            self.read_buf.append(&mut response_datagrams.remove(&i).unwrap());
        }

        self.request_id = (self.request_id % (u16::MAX)) + 1;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::thread;

    fn flush(datagrams: Vec<(u16, u16, &'static [u8])>) -> io::Result<Vec<u8>> {
        let server = UdpSocket::bind("127.0.0.1:0")?;
        let url = Url::parse(&format!("memcache+udp://127.0.0.1:{}", server.local_addr()?.port())).unwrap();
        thread::spawn(move || {
            let mut buf = [0; 1400];
            let (_, peer) = server.recv_from(&mut buf).unwrap();
            for (sequence_no, total_datagrams, payload) in datagrams {
                let mut datagram = buf[0..2].to_vec();
                datagram.write_u16::<BigEndian>(sequence_no).unwrap();
                datagram.write_u16::<BigEndian>(total_datagrams).unwrap();
                datagram.write_u16::<BigEndian>(0).unwrap();
                datagram.extend_from_slice(payload);
                server.send_to(&datagram, peer).unwrap();
            }
        });

        let mut stream = UdpStream::new(&url, None).unwrap();
        stream.set_read_timeout(Some(Duration::from_secs(1))).unwrap();
        stream.write_all(b"version\r\n")?;
        stream.flush()?;
        let mut response = Vec::new();
        stream.read_to_end(&mut response)?;
        Ok(response)
    }

    #[test]
    fn flush_reassembles_datagrams_in_sequence_order() {
        assert_eq!(
            flush(vec![(1, 2, b"world"), (0, 2, b"hello ")]).unwrap(),
            b"hello world"
        );
    }

    #[test]
    fn flush_rejects_invalid_datagram_headers() {
        assert!(flush(vec![(0, 0, b"x")]).is_err());
        assert!(flush(vec![(2, 2, b"x"), (1, 2, b"x")]).is_err());
        assert!(flush(vec![(0, 2, b"x"), (1, 3, b"x")]).is_err());
    }
}
