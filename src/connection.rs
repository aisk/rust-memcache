use std::error::FromError;
use std::io::{BufferedStream, IoError};
use std::io::net::tcp::TcpStream;

#[derive(Show)]
pub enum MemcacheError {
    InternalIoError(IoError),
    ServerError
}

impl FromError<IoError> for MemcacheError {
    fn from_error(err: IoError) -> MemcacheError {
        MemcacheError::InternalIoError(err)
    }
}

pub type MemcacheResult<T> = Result<T, MemcacheError>;

pub struct Connection {
    pub host: String,
    pub port: u16,
    stream: BufferedStream<TcpStream>,
}

impl Connection {
    pub fn flush(&mut self) -> MemcacheResult<()> {
        try!{ self.stream.write_str("flush_all\r\n") };
        try!{ self.stream.flush() };
        let result = try!{ self.stream.read_line() };
        if result.as_slice() != "OK\r\n" {
            return Err(MemcacheError::ServerError);
        }
        return Ok(());
    }

    pub fn delete(&mut self, key: &str) -> MemcacheResult<bool> {
        try!{ self.stream.write_str(format!("delete {}\r\n", key).as_slice()) };
        try!{ self.stream.flush() };
        let result = try! { self.stream.read_line() };
        if result.as_slice() == "DELETED\r\n" {
            return Ok(true);
        } else if result.as_slice() == "NOT_FOUND\r\n" {
            return Ok(false);
        } else {
            return Err(MemcacheError::ServerError);
        }
    }

    pub fn get(&mut self, key: &str) -> MemcacheResult<Option<(Vec<u8>, u16)>> {
        try!{ self.stream.write_str(format!("get {}\r\n", key).as_slice()) };
        try!{ self.stream.flush() };
        let result = try!{ self.stream.read_line() };
        if result.as_slice() == "END\r\n" {
            return Ok(None);
        }
        let header: Vec<&str> = result.split(' ').collect();
        if header.len() != 4 || header[0] != "VALUE" || header[1] != key {
            return Err(MemcacheError::ServerError);
        }
        let flags: u16 = match header[2].trim().parse() {
            Some(flags) => { flags }
            None => { return Err(MemcacheError::ServerError); }
        };
        let length: usize = match header[3].trim().parse() {
            Some(length) => { length }
            None => { return Err(MemcacheError::ServerError); }
        };
        let value = try!{ self.stream.read_exact(length) };
        return Ok(Some((value, flags)));
    }

    pub fn set(&mut self, key: &str, value: &[u8], exptime: isize, flags: u16) -> MemcacheResult<bool> {
        try!{ self.stream.write_str(format!("set {} {} {} {}\r\n", key, flags, exptime, value.len()).as_slice()) };
        try!{ self.stream.write(value) };
        try!{ self.stream.write_str("\r\n") };
        try!{ self.stream.flush() };
        let result = try!{ self.stream.read_line() };
        if result.as_slice() == "STORED\r\n" {
            return Ok(true);
        } else if result.as_slice() == "NOT_STORED\r\n" {
            return Ok(false);
        }
        return Err(MemcacheError::ServerError);
    }

    pub fn incr(&mut self, key: &str, value: u64) -> MemcacheResult<Option<(isize)>> {
        try!{ self.stream.write_str(format!("incr {} {:b}\r\n", key, value).as_slice()) };
        try!{ self.stream.flush() };
        let result = try!{ self.stream.read_line() };
        if result.as_slice() == "NOT_FOUND\r\n" {
            return Ok(None);
        }
        let x: &[_] = &['\r', '\n'];
        // let trimed_result = result.trim_right_matches(x);
        let value: isize = match result.trim_right_matches(x).parse() {
            Some(value) => { value }
            None => { return Err(MemcacheError::ServerError) }
        };
        return Ok(Some(value));
    }

    pub fn decr(&mut self, key: &str, value: u64) -> MemcacheResult<Option<(isize)>> {
        try!{ self.stream.write_str(format!("decr {} {:b}\r\n", key, value).as_slice()) };
        try!{ self.stream.flush() };
        let result = try!{ self.stream.read_line() };
        if result.as_slice() == "NOT_FOUND\r\n" {
            return Ok(None);
        }
        let x: &[_] = &['\r', '\n'];
        // let trimed_result = result.trim_right_matches(x);
        let value: isize = match result.trim_right_matches(x).parse() {
            Some(value) => { value }
            None => { return Err(MemcacheError::ServerError) }
        };
        return Ok(Some(value));
    }

    pub fn connect(host: &str, port: u16) -> MemcacheResult<Connection> {
        let stream = try!{ TcpStream::connect((host, port)) };
        return Ok(Connection{
            host: host.to_string(),
            port: port,
            stream: BufferedStream::new(stream)
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

// #[test]
// fn test_get() {
//     let mut conn = Connection::connect("localhost", 2333).ok().unwrap();
//     assert!{ conn.flush().is_ok() };
//     assert!{ conn.get("foo").ok().unwrap() == None };
// 
//     assert!{ conn.set("foo", b"bar", 0, 10).ok().unwrap() == true };
//     let result = conn.get("foo").ok().unwrap();
//     assert!{ result.unwrap().0 == b"bar" };
//     assert!{ result.unwrap().1 == 10 };
// }

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
