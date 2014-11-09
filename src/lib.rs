use std::io::{BufferedStream, IoResult};
use std::io::net::tcp::TcpStream;

#[deriving(Show)]
pub struct MemcacheError {
    kind: MemcacheErrorKind,
}

#[deriving(Show)]
pub enum MemcacheErrorKind {
    Other,
}

impl MemcacheError {
    pub fn new<T: IntoMaybeOwned<'static>>(kind: MemcacheErrorKind) -> MemcacheError {
        MemcacheError {
            kind: Other,
        }
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
        self.stream.write_str("flush_all\r\n").unwrap();
        self.stream.flush().unwrap();
        let result = self.stream.read_line().unwrap();
        assert!(result == "OK\r\n".to_string());
        if result == "OK\r\n".to_string() {
            return Err(MemcacheError::new(Other));
        }
        return Ok(());
    }

    pub fn connect(host: &str, port: u16) -> IoResult<Connection> {
        match TcpStream::connect((host, port)) {
            Ok(stream) => {
                return Ok(Connection{
                    host: host.to_string(),
                    port: port,
                    stream: BufferedStream::new(stream)
                });
            }
            Err(err) => {
                return Err(err);
            }
        }
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
