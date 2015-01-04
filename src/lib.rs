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

    pub fn get(&mut self, key: &str) -> MemcacheResult<Option<String>> {
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
        let length: uint = match header[3].trim().parse() {
            Some(length) => { length }
            None => { return Err(MemcacheError::ServerError); }
        };
        let body = try!{ self.stream.read_exact(length) };
        let value = match String::from_utf8(body) {
            Ok(value) => { value }
            Err(_) => { return Err(MemcacheError::ServerError); }
        };
        return Ok(Some(value));
    }

    pub fn set(&mut self, key: &str, value: &str, exptime: int) -> MemcacheResult<bool> {
        try!{ self.stream.write_str(format!("set {} 0 {} {}\r\n", key, exptime, value.len()).as_slice()) };
        try!{ self.stream.write_str(value) };
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
fn test_connect() -> () {
    Connection::connect("localhost", 2333).unwrap();
}

#[test]
fn test_flush() -> () {
    let mut conn = Connection::connect("localhost", 2333).unwrap();
    conn.flush().unwrap();
}

#[test]
fn test_set() -> () {
    let mut conn = Connection::connect("localhost", 2333).unwrap();
    conn.flush().unwrap();  // ensure memcached is clean
    assert!{ conn.set("foo", "bar", 10).unwrap() == true };
}

#[test]
fn test_get() -> () {
    let mut conn = Connection::connect("localhost", 2333).unwrap();
    conn.flush().unwrap();  // ensure memcached is clean
    assert!{ conn.get("foo").unwrap() == None };

    assert!{ conn.set("foo", "bar", 0).unwrap() == true };
    assert!{ conn.get("foo").unwrap().unwrap().as_slice() == "bar" };
}

#[test]
fn test_delete() -> () {
    let mut conn = Connection::connect("localhost", 2333).unwrap();
    conn.flush().unwrap();  // ensure memcached is clean
    assert!{ conn.delete("foo").unwrap() == false };
}
