mod udp_stream;

use std::net::TcpStream;
#[cfg(unix)]
use std::os::unix::net::UnixStream;

pub(crate) use self::udp_stream::UdpStream;

pub(crate) enum Stream {
    TcpStream(TcpStream),
    UdpStream(UdpStream),
    #[cfg(unix)]
    UnixStream(UnixStream),
}
