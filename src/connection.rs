use std::fmt;
use std::io::BufRead;
use std::io::Write;
use std::io::Read;
use std::io;
use std::net;

use types::MemcacheError;

pub struct Connection {
    reader: io::BufReader<net::TcpStream>,
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
    fn store(&mut self,
             command: StoreCommand,
             key: &str,
             value: &[u8],
             flags: u16,
             exptime: u32)
             -> Result<bool, MemcacheError> {
        write!(self.reader.get_ref(),
               "{} {} {} {} {}\r\n",
               command,
               key,
               flags,
               exptime,
               value.len())?;
        self.reader.get_ref().write(value)?;
        self.reader.get_ref().write(b"\r\n")?;
        self.reader.get_ref().flush()?;
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
            Ok(_) => {}
            Err(err) => return Err(MemcacheError::from(err)),
        }
        self.reader.get_ref().flush()?;
        let mut s = String::new();
        let _ = self.reader.read_line(&mut s);
        if s == "ERROR\r\n" {
            return Err(MemcacheError::Error);
        } else if s.starts_with("CLIENT_ERROR") {
            return Err(MemcacheError::ClientError(s));
        } else if s.starts_with("SERVER_ERROR") {
            return Err(MemcacheError::ServerError(s));
        } else if s != "OK\r\n" {
            return Err(MemcacheError::Error);
        }
        return Ok(());
    }

    pub fn set(&mut self,
               key: &str,
               value: &[u8],
               flags: u16,
               exptime: u32)
               -> Result<bool, MemcacheError> {
        return self.store(StoreCommand::Set, key, value, flags, exptime);
    }

    pub fn replace(&mut self,
                   key: &str,
                   value: &[u8],
                   flags: u16,
                   exptime: u32)
                   -> Result<bool, MemcacheError> {
        return self.store(StoreCommand::Replace, key, value, flags, exptime);
    }

    pub fn get(&mut self, keys: &[&str]) -> Result<Vec<(u16, Vec<u8>)>, MemcacheError> {
        let mut result: Vec<(u16, Vec<u8>)> = vec![];

        write!(self.reader.get_ref(), "get {}\r\n", keys.join(" "))?;

        while true {
            let mut s = String::new();
            let _ = self.reader.read_line(&mut s)?;
            if s == "END\r\n" {
                break;
            }
            if s == "ERROR\r\n" {
                return Err(MemcacheError::Error);
            } else if s.starts_with("CLIENT_ERROR") {
                return Err(MemcacheError::ClientError(s));
            } else if s.starts_with("SERVER_ERROR") {
                return Err(MemcacheError::ServerError(s));
            } else if !s.starts_with("VALUE") {
                return Err(MemcacheError::Error);
            }
            let header: Vec<_> = s.trim_right_matches("\r\n").split(" ").collect();
            print!("header: {}", s);
            if header.len() != 4 {
                return Err(MemcacheError::Error);
            }
            let key = header[1];
            let flags: u16;
            let length: usize;
            match header[2].parse() {
                Ok(n) => flags = n,
                Err(e) => return Err(MemcacheError::Error),
            };
            match header[3].parse() {
                Ok(n) => length = n,
                Err(e) => return Err(MemcacheError::Error),
            };
            let mut buffer = vec![0; length];
            self.reader.read_exact(buffer.as_mut_slice())?;
            let mut t = (flags, buffer);
            result.push(t);

            // read the rest \r\n
            let mut s = String::new();
            let _ = self.reader.read_line(&mut s)?;
            if s != "\r\n" {
                return Err(MemcacheError::Error);
            }
        }
        return Ok(result);
    }

    pub fn delete(&mut self, key: &str) -> Result<bool, MemcacheError> {
        write!(self.reader.get_ref(), "delete {}\r\n", key)?;
        self.reader.get_ref().flush()?;
        let mut s = String::new();
        let _ = self.reader.read_line(&mut s);
        if s == "ERROR\r\n" {
            return Err(MemcacheError::Error);
        } else if s.starts_with("CLIENT_ERROR") {
            return Err(MemcacheError::ClientError(s));
        } else if s.starts_with("SERVER_ERROR") {
            return Err(MemcacheError::ServerError(s));
        } else if s == "DELETED\r\n" {
            return Ok(true);
        } else if s == "NOT_FOUND\r\n" {
            return Ok(false);
        } else {
            return Err(MemcacheError::Error);
        }
    }

    pub fn version(&mut self) -> Result<String, MemcacheError> {
        self.reader.get_ref().write(b"version\r\n")?;
        self.reader.get_ref().flush()?;
        let mut s = String::new();
        let _ = self.reader.read_line(&mut s);
        if s == "ERROR\r\n" {
            return Err(MemcacheError::Error);
        } else if s.starts_with("CLIENT_ERROR") {
            return Err(MemcacheError::ClientError(s));
        } else if s.starts_with("SERVER_ERROR") {
            return Err(MemcacheError::ServerError(s));
        } else if !s.starts_with("VERSION") {
            return Err(MemcacheError::Error);
        }
        let s = s.trim_left_matches("VERSION ");
        let s = s.trim_right_matches("\r\n");

        return Ok(s.to_string());
    }
}

pub fn connect() -> Result<Connection, MemcacheError> {
    let stream = net::TcpStream::connect("127.0.0.1:11211")?;
    return Ok(Connection { reader: io::BufReader::new(stream) });
}
