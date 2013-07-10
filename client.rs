extern mod extra;

use std::io;
use extra::net;
use extra::uv;

struct Client {
    writer: @io::WriterUtil,
    reader: @io::ReaderUtil
}

impl Client {
    fn flush(&self) {
        self.writer.write_str("flush_all\r\n");
    }
}

fn main() {
    let socket = get_connection().get();
    let writer = @socket as @io::WriterUtil;
    let reader = @socket as @io::ReaderUtil;

    let client = Client{writer: writer, reader: reader};

    client.flush();

}

fn get_connection() -> Result<net::tcp::TcpSocketBuf, ()> {
    match net::tcp::connect(net::ip::v4::parse_addr("127.0.0.1"), 11211, &uv::global_loop::get()) {
        Err(err) => {
            match err {
                net::tcp::GenericConnectErr(name,msg) => io::println(fmt!("Connection error %s: %s", name, msg)),
                net::tcp::ConnectionRefused => io::println("Connection refused")
            }
            return Err(());
        },
        Ok(socket) => {
            let socket_buf = net::tcp::socket_buf(socket);
            return Ok(socket_buf);
        }
    }

}
