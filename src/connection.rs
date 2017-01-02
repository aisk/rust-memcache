use std::fmt;
use std::io::BufRead;
use std::io::Write;
use std::io::Read;
use std::io;
use std::net;

use value::{
    ToMemcacheValue,
    Raw,
};
use error::{
    MemcacheError,
    is_memcache_error,
};

pub struct Connection {
    reader: io::BufReader<net::TcpStream>,
}

enum StoreCommand {
    Set,
    Add,
    Replace,
    Append,
    Prepend,
}

impl fmt::Display for StoreCommand {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match *self {
            StoreCommand::Set => write!(f, "set"),
            StoreCommand::Add => write!(f, "add"),
            StoreCommand::Replace => write!(f, "replace"),
            StoreCommand::Append => write!(f, "append"),
            StoreCommand::Prepend => write!(f, "prepend")
        }
    }
}

impl Connection {
    fn store<V>(&mut self,
                command: StoreCommand,
                key: &str,
                value: V,
                exptime: u32)
                -> Result<bool, MemcacheError>
        where V: ToMemcacheValue
    {
        let bytes = value.get_bytes();
        write!(self.reader.get_ref(),
               "{} {} {} {} {}\r\n",
               command,
               key,
               value.get_flags(),
               exptime,
               bytes.len())?;
        self.reader.get_ref().write(bytes)?;
        self.reader.get_ref().write(b"\r\n")?;
        self.reader.get_ref().flush()?;
        let mut s = String::new();
        let _ = self.reader.read_line(&mut s);
        if is_memcache_error(s.as_str()) {
            return Err(MemcacheError::from(s));
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
        if is_memcache_error(s.as_str()) {
            return Err(MemcacheError::from(s));
        } else if s != "OK\r\n" {
            return Err(MemcacheError::Error);
        }
        return Ok(());
    }

    pub fn set<V>(&mut self, key: &str, value: V, exptime: u32) -> Result<bool, MemcacheError>
        where V: ToMemcacheValue
    {
        return self.store(StoreCommand::Set, key, value, exptime);
    }

    pub fn add<V>(&mut self, key: &str, value: V, exptime: u32) -> Result<bool, MemcacheError>
        where V: ToMemcacheValue
    {
        return self.store(StoreCommand::Add, key, value, exptime);
    }

    pub fn replace<V>(&mut self, key: &str, value: V, exptime: u32) -> Result<bool, MemcacheError>
        where V: ToMemcacheValue
    {
        return self.store(StoreCommand::Replace, key, value, exptime);
    }

    pub fn append<V>(&mut self, key: &str, value: V, exptime: u32) -> Result<bool, MemcacheError>
        where V: ToMemcacheValue
    {
        return self.store(StoreCommand::Append, key, value, exptime);
    }

    pub fn prepend<V>(&mut self, key: &str, value: V, exptime: u32) -> Result<bool, MemcacheError>
        where V: ToMemcacheValue
    {
        return self.store(StoreCommand::Prepend, key, value, exptime);
    }

    pub fn get_raw(&mut self, keys: &[&str]) -> Result<Vec<(String, Raw)>, MemcacheError> {
        let mut result: Vec<(String, Raw)> = vec![];

        write!(self.reader.get_ref(), "get {}\r\n", keys.join(" "))?;

        let mut loop_count = 0;
        while loop_count < 1000000 { // prevent infinity loop
            loop_count += 1;
            let mut s = String::new();
            let _ = self.reader.read_line(&mut s)?;
            if s == "END\r\n" {
                break;
            }
            if is_memcache_error(s.as_str()) {
                return Err(MemcacheError::from(s));
            } else if !s.starts_with("VALUE") {
                return Err(MemcacheError::Error);
            }
            let header: Vec<_> = s.trim_right_matches("\r\n").split(" ").collect();
            if header.len() != 4 {
                return Err(MemcacheError::Error);
            }
            let key = header[1];
            let flags: u16;
            let length: usize;
            match header[2].parse() {
                Ok(n) => flags = n,
                Err(_) => return Err(MemcacheError::Error),
            };
            match header[3].parse() {
                Ok(n) => length = n,
                Err(_) => return Err(MemcacheError::Error),
            };
            let mut buffer = vec![0; length];
            self.reader.read_exact(buffer.as_mut_slice())?;
            let raw = Raw{bytes: b"aaa", flags: flags};
            let t = (key.to_string(), raw);
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
        if is_memcache_error(s.as_str()) {
            return Err(MemcacheError::from(s));
        } else if s == "DELETED\r\n" {
            return Ok(true);
        } else if s == "NOT_FOUND\r\n" {
            return Ok(false);
        } else {
            return Err(MemcacheError::Error);
        }
    }

    pub fn incr(&mut self, key: &str, value: u32) -> Result<Option<u32>, MemcacheError> {
        write!(self.reader.get_ref(), "incr {} {}\r\n", key, value)?;
        let mut s = String::new();
        let _ = self.reader.read_line(&mut s);
        if is_memcache_error(s.as_str()) {
            return Err(MemcacheError::from(s));
        } else if s == "NOT_FOUND\r\n" {
            return Ok(None);
        } else {
            match s.trim_right_matches("\r\n").parse::<u32>() {
                Ok(n) => return Ok(Some(n)),
                Err(_) => return Err(MemcacheError::Error),
            }
        }
    }

    pub fn decr(&mut self, key: &str, value: u32) -> Result<Option<u32>, MemcacheError> {
        write!(self.reader.get_ref(), "decr {} {}\r\n", key, value)?;
        let mut s = String::new();
        let _ = self.reader.read_line(&mut s);
        if is_memcache_error(s.as_str()) {
            return Err(MemcacheError::from(s));
        } else if s == "NOT_FOUND\r\n" {
            return Ok(None);
        } else {
            match s.trim_right_matches("\r\n").parse::<u32>() {
                Ok(n) => return Ok(Some(n)),
                Err(_) => return Err(MemcacheError::Error),
            }
        }
    }

    pub fn version(&mut self) -> Result<String, MemcacheError> {
        self.reader.get_ref().write(b"version\r\n")?;
        self.reader.get_ref().flush()?;
        let mut s = String::new();
        let _ = self.reader.read_line(&mut s);
        if is_memcache_error(s.as_str())  {
            return Err(MemcacheError::from(s));
        } else if !s.starts_with("VERSION") {
            return Err(MemcacheError::Error);
        }
        let s = s.trim_left_matches("VERSION ");
        let s = s.trim_right_matches("\r\n");

        return Ok(s.to_string());
    }
}

pub fn connect<A: net::ToSocketAddrs>(addr: A) -> Result<Connection, MemcacheError> {
    let stream = net::TcpStream::connect(addr)?;
    return Ok(Connection { reader: io::BufReader::new(stream) });
}
