extern mod extra;

use std::io;
use extra::net;
use extra::uv;

struct Client {
    writer: @io::WriterUtil,
    reader: @io::ReaderUtil
}

impl Client {
    fn flush(&self) -> Result<~str, ~str> {
        self.writer.write_str("flush_all\r\n");
        let result_str = self.reader.read_line();
        if result_str == ~"OK\r" {  // XXX: read_line dose not remove \r, is it a bug?
            return Ok(result_str);
        }
        else {
            return Err(result_str);
        }
    }
    
    fn _store(&self, action: ~str, key: ~str, value: ~str, exp_time: uint) -> Result<~str, ~str> {
        self.writer.write_str(fmt!("%s %s 0 %u %u\r\n", action, key, exp_time, value.len()));
        self.writer.write_str(fmt!("%s\r\n", value));
        let result_str = self.reader.read_line();
        if result_str == ~"STORED\r" {
            return Ok(result_str);
        }
        else {
            return Err(result_str);
        }
    }
    
    fn set(&self, key: ~str, value: ~str, exp_time: uint) -> Result<~str, ~str> {
        self._store(~"set", key, value, exp_time)
    }
    
    fn add(&self, key: ~str, value: ~str, exp_time: uint) -> Result<~str, ~str> {
        self._store(~"add", key, value, exp_time)
    }
    
    fn replace(&self, key: ~str, value: ~str, exp_time: uint) -> Result<~str, ~str> {
        self._store(~"replace", key, value, exp_time)
    }
}

fn main() {
    let client = connect(~"127.0.0.1", 11211).get();

    client.flush();

    client.set(~"foo", ~"bar", 0);
    client.add(~"foo", ~"bar", 0);
    client.replace(~"foo", ~"bar", 0);

}

fn connect(addr: ~str, port: uint) -> Result<Client, net::tcp::TcpConnectErrData> {
    match net::tcp::connect(net::ip::v4::parse_addr(addr), port, &uv::global_loop::get()) {
        Err(err) => {
            //match err {
            //    net::tcp::GenericConnectErr(name,msg) => io::println(fmt!("Connection error %s: %s", name, msg)),
            //    net::tcp::ConnectionRefused => io::println("Connection refused")
            //}
            return Err(err);
        },
        Ok(socket) => {
            let socket_buf = net::tcp::socket_buf(socket);
            let writer = @socket_buf as @io::WriterUtil;
            let reader = @socket_buf as @io::ReaderUtil;
            let client = Client{writer: writer, reader: reader};
            return Ok(client);
        }
    }

}
