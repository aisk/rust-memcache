use std::io;
use std::net;

pub struct Connection {
    stream: net::TcpStream
}

impl Connection {
}

pub fn connect()-> Result<Connection, io::Error>{
    match net::TcpStream::connect("127.0.0.1:11211") {
        Ok(stream) => return Ok(Connection{stream: stream}),
        Err(err) => return Err(err),
    }
}
