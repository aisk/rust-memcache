use std::fmt;
use std::io::BufRead;
use std::io::Write;
use std::io;
use std::net;

use types::MemcacheError;

pub struct Connection {
    reader: io::BufReader<net::TcpStream>
}

enum StoreCommand {
    Set,
    Add,
    Replace,
}

impl fmt::Display for StoreCommand {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match *self {
            StoreCommand::Set => write!(f, "set"),
            StoreCommand::Add => write!(f, "add"),
            StoreCommand::Replace => write!(f, "replace"),
        }
    }
}

impl Connection {
    fn store(&mut self, command: StoreCommand, key: &str, value: &[u8], flags: u16, exptime: u32) -> Result<bool, MemcacheError> {
        try!(write!(self.reader.get_ref(), "{} {} {} {} {}\r\n", command, key, flags, exptime, value.len()));
        try!(self.reader.get_ref().write(value));
        try!(self.reader.get_ref().write(b"\r\n"));
        try!(self.reader.get_ref().flush());
        let mut s = String::new();
        let _ = self.reader.read_line(&mut s);
        if s == "ERROR\r\n" {
            return Err(MemcacheError::Error);
        } else if s.starts_with("CLIENT_ERROR") {
            return Err(MemcacheError::ClientError(s));
        } else if s.starts_with("SERVER_ERROR") {
            return Err(MemcacheError::ServerError(s));
        } else if s == "STORED\r\n" {
            return Ok(true);
        } else if s == "NOT_STORED\r\n" {
            return Ok(false);
        } else {
            return Err(MemcacheError::Error);
        }
    }

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

    pub fn set(&mut self, key: &str, value: &[u8], flags: u16, exptime: u32) -> Result<bool, MemcacheError> {
        return self.store(StoreCommand::Set, key, value, flags, exptime);
    }

    pub fn replace(&mut self, key: &str, value: &[u8], flags: u16, exptime: u32) -> Result<bool, MemcacheError> {
        return self.store(StoreCommand::Replace, key, value, flags, exptime);
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
