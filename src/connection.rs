use std::io;
use std::net;
use std::io::Write;

pub struct Connection {
    stream: net::TcpStream
}

impl Connection {
    pub fn flush(&mut self) -> io::Result<()> {
        match self.stream.write(b"flush_all\r\n") {
            Ok(_) => {},
            Err(err) => return Err(err),
        }
        return self.stream.flush();
    }
}

pub fn connect()-> Result<Connection, io::Error>{
    match net::TcpStream::connect("127.0.0.1:11211") {
        Ok(stream) => return Ok(Connection{stream: stream}),
        Err(err) => return Err(err),
    }
}
