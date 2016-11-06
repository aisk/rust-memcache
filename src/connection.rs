use std::net;
use std::io;
use std::io::Write;
use std::io::BufRead;

use types::MemcacheError;

pub struct Connection {
    reader: io::BufReader<net::TcpStream>
}

impl Connection {
    pub fn flush(&mut self) -> Result<(), MemcacheError> {
        match self.reader.get_ref().write(b"flush_all\r\n") {
            Ok(_) => {},
            Err(err) => return Err(MemcacheError::from(err)),
        }
        try!(self.reader.get_ref().flush());
        let mut s = String::new();
        let _ = self.reader.read_line(&mut s);
        if s == "ERROR\r\n" {
            return Err(MemcacheError::Error);
        } else if s.starts_with("CLIENT_ERROR") {
            return Err(MemcacheError::ClientError(s));
        } else if s.starts_with("SERVER_ERROR") {
            return Err(MemcacheError::ServerError(s));
        } else if s != "OK\r\n" {
            return Err(MemcacheError::Error)
        }
        return Ok(());
    }

    pub fn version(&mut self) -> Result<String, MemcacheError> {
        match self.reader.get_ref().write(b"version\r\n") {
            Ok(_) => {},
            Err(err) => return Err(MemcacheError::from(err)),
        }

        try!(self.reader.get_ref().flush());
        let mut s = String::new();
        let _ = self.reader.read_line(&mut s);
        if s == "ERROR\r\n" {
            return Err(MemcacheError::Error);
        } else if s.starts_with("CLIENT_ERROR") {
            return Err(MemcacheError::ClientError(s));
        } else if s.starts_with("SERVER_ERROR") {
            return Err(MemcacheError::ServerError(s));
        } else if ! s.starts_with("VERSION") {
            return Err(MemcacheError::Error);
        }
        let s = s.trim_left_matches("VERSION ");
        let s = s.trim_right_matches("\r\n");

        return Ok(s.to_string());
    }
}

pub fn connect()-> Result<Connection, MemcacheError>{
    match net::TcpStream::connect("127.0.0.1:11211") {
        Ok(stream) => return Ok(Connection{reader: io::BufReader::new(stream)}),
        Err(err) => return Err(MemcacheError::from(err)),
    }
}
