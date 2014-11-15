use std::error::FromError;
use std::io::{BufferedStream, IoError};
use std::io::net::tcp::TcpStream;

#[deriving(Show)]
pub enum MemcacheError {
    InternalIoError(IoError),
    ServerError
}

impl FromError<IoError> for MemcacheError {
    fn from_error(err: IoError) -> MemcacheError {
        InternalIoError(err)
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
        try!(self.stream.write_str("flush_all\r\n"));
        try!(self.stream.flush());
        let result = try!(self.stream.read_line());
        if result != "OK\r\n".to_string() {
            return Err(ServerError);
        }
        return Ok(());
    }

    pub fn connect(host: &str, port: u16) -> MemcacheResult<Connection> {
        let stream = try!(TcpStream::connect((host, port)));
        return Ok(Connection{
            host: host.to_string(),
            port: port,
            stream: BufferedStream::new(stream)
        });
    }
}

#[test]
fn test_connect() -> () {
    Connection::connect("localhost", 11211).unwrap();
}

#[test]
fn test_flush() ->() {
    let mut conn = Connection::connect("localhost", 11211).unwrap();
    conn.flush().unwrap();
}
