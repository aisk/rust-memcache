use std::net;
use std::io;
use std::io::Write;
use std::io::BufRead;

use types::MemcacheError;

pub struct Connection {
    reader: io::BufReader<net::TcpStream>
}

impl Connection {
    pub fn flush(&mut self) -> io::Result<()> {
        match self.reader.get_ref().write(b"flush_all\r\n") {
            Ok(_) => {},
            Err(err) => return Err(err),
        }
        try!(self.reader.get_ref().flush());
        let mut s = String::new();
        let _ = self.reader.read_line(&mut s);
        s == "OK\r\n";  // TODO: assert it
        return Ok(());
    }
}

pub fn connect()-> Result<Connection, MemcacheError>{
    match net::TcpStream::connect("127.0.0.1:11211") {
        Ok(stream) => return Ok(Connection{reader: io::BufReader::new(stream)}),
        Err(err) => return Err(MemcacheError::from(err)),
    }
}
