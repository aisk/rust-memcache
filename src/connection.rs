use std::convert::From;
use std::io::{BufStream, Error, Write, Read, BufRead};
use std::net::TcpStream;

#[derive(Debug)]
pub enum MemcacheError {
    InternalIoError(Error),
    ServerError
}

impl From<Error> for MemcacheError {
    fn from(err: Error) -> MemcacheError {
        MemcacheError::InternalIoError(err)
    }
}

pub type MemcacheResult<T> = Result<T, MemcacheError>;

pub struct Connection {
    pub host: String,
    pub port: u16,
    stream: BufStream<TcpStream>,
}

impl Connection {
    pub fn flush(&mut self) -> MemcacheResult<()> {
        try!{ self.stream.write("flush_all\r\n".as_bytes()) };
        try!{ self.stream.flush() };
        let mut line : String = String::new();
        try!{ self.stream.read_line(&mut line) };
        if line != "OK\r\n" {
            return Err(MemcacheError::ServerError);
        }
        return Ok(());
    }

    pub fn delete(&mut self, key: &str) -> MemcacheResult<bool> {
        try!{ self.stream.write(format!("delete {}\r\n", key).as_bytes()) };
        try!{ self.stream.flush() };
        let mut line : String = String::new();
        try! { self.stream.read_line(&mut line) };
        if line == "DELETED\r\n" {
            return Ok(true);
        } else if line == "NOT_FOUND\r\n" {
            return Ok(false);
        } else {
            return Err(MemcacheError::ServerError);
        }
    }

    pub fn get(&mut self, key: &str) -> MemcacheResult<Option<(Vec<u8>, u16)>> {
        try!{ self.stream.write(format!("get {}\r\n", key).as_bytes()) };
        try!{ self.stream.flush() };
        let mut line : String = String::new();
        try! { self.stream.read_line(&mut line) };
        if line == "END\r\n" {
            return Ok(None);
        }
        let header: Vec<&str> = line.split(' ').collect();
        if header.len() != 4 || header[0] != "VALUE" || header[1] != key {
            return Err(MemcacheError::ServerError);
        }
        let flags: u16 = match header[2].trim().parse() {
            Ok(flags) => { flags }
            Err(err) => { return Err(MemcacheError::ServerError); }
        };
        let length: usize = match header[3].trim().parse() {
            Ok(length) => { length }
            Err(err) => { return Err(MemcacheError::ServerError); }
        };
        let mut buf : Vec<u8> = Vec::with_capacity(length);
        self.stream.read(&mut buf);
        return Ok(Some((buf, flags)));
    }

    pub fn set(&mut self, key: &str, value: &[u8], exptime: isize, flags: u16) -> MemcacheResult<bool> {
        try!{ self.stream.write(format!("set {} {} {} {}\r\n", key, flags, exptime, value.len()).as_bytes()) };
        try!{ self.stream.write(value) };
        try!{ self.stream.write("\r\n".as_bytes()) };
        try!{ self.stream.flush() };
        let mut line : String = String::new();
        try! { self.stream.read_line(&mut line) };
        if line == "STORED\r\n" {
            return Ok(true);
        } else if line == "NOT_STORED\r\n" {
            return Ok(false);
        }
        return Err(MemcacheError::ServerError);
    }

    pub fn incr(&mut self, key: &str, value: u64) -> MemcacheResult<Option<(isize)>> {
        try!{ self.stream.write(format!("incr {} {:b}\r\n", key, value).as_bytes()) };
        try!{ self.stream.flush() };
        let mut line : String = String::new();
        try! { self.stream.read_line(&mut line) };
        if line == "NOT_FOUND\r\n" {
            return Ok(None);
        }
        let x: &[_] = &['\r', '\n'];
        // let trimed_result = result.trim_right_matches(x);
        let value: isize = match line.trim_right_matches(x).parse() {
            Ok(value) => { value }
            Err(err) => { return Err(MemcacheError::ServerError) }
        };
        return Ok(Some(value));
    }

    pub fn decr(&mut self, key: &str, value: u64) -> MemcacheResult<Option<(isize)>> {
        try!{ self.stream.write(format!("decr {} {:b}\r\n", key, value).as_bytes()) };
        try!{ self.stream.flush() };
        let mut line : String = String::new();
        try! { self.stream.read_line(&mut line) };
        if line == "NOT_FOUND\r\n" {
            return Ok(None);
        }
        let x: &[_] = &['\r', '\n'];
        // let trimed_result = result.trim_right_matches(x);
        let value: isize = match line.trim_right_matches(x).parse() {
            Ok(value) => { value }
            Err(err) => { return Err(MemcacheError::ServerError) }
        };
        return Ok(Some(value));
    }

    pub fn connect(host: &str, port: u16) -> MemcacheResult<Connection> {
        let stream = try!{ TcpStream::connect((host, port)) };
        return Ok(Connection{
            host: host.to_string(),
            port: port,
            stream: BufStream::new(stream)
        });
    }
}

#[test]
fn test_connect() {
    assert!{ Connection::connect("localhost", 2333).is_ok() };
}

#[test]
fn test_flush() {
    let mut conn = Connection::connect("localhost", 2333).ok().unwrap();
    assert!{ conn.flush().is_ok() };
}

#[test]
fn test_set() {
    let mut conn = Connection::connect("localhost", 2333).ok().unwrap();
    assert!{ conn.flush().is_ok() };
    assert!{ conn.set("foo", b"bar", 10, 0).ok().unwrap() == true };
}

#[test]
fn test_get() {
    let mut conn = Connection::connect("localhost", 2333).ok().unwrap();
    assert!{ conn.flush().is_ok() };
    assert!{ conn.get("foo").ok().unwrap() == None };

    assert!{ conn.set("foo", b"bar", 0, 10).ok().unwrap() == true };
    let result = conn.get("foo");
    let result_tuple = result.ok().unwrap().unwrap();
    assert!{ result_tuple.0 == b"bar" };
    assert!{ result_tuple.1 == 10 };
}

#[test]
fn test_delete() {
    let mut conn = Connection::connect("localhost", 2333).ok().unwrap();
    assert!{ conn.flush().is_ok() };
    assert!{ conn.delete("foo").ok().unwrap() == false };
}

#[test]
fn test_incr() {
    let mut conn = Connection::connect("localhost", 2333).ok().unwrap();
    assert!{ conn.flush().is_ok() };
    let mut result = conn.incr("lie", 42);
    assert!{ result.ok().unwrap() == None };

    assert!{ conn.flush().is_ok() };
    conn.set("truth", b"42", 0, 0).ok().unwrap();
    result = conn.incr("truth", 1);
    assert!{ result.ok().unwrap().unwrap() == 43 };
}

#[test]
fn test_decr() {
    let mut conn = Connection::connect("localhost", 2333).ok().unwrap();
    assert!{ conn.flush().is_ok() };
    let mut result = conn.decr("lie", 42);
    assert!{ result.ok().unwrap() == None };

    assert!{ conn.flush().is_ok() };
    conn.set("truth", b"42", 0, 0).ok().unwrap();
    result = conn.decr("truth", 1);
    assert!{ result.ok().unwrap().unwrap() == 41 };
}
