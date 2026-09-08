use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::thread;
use std::time::Duration;

use byteorder::{BigEndian, ByteOrder, WriteBytesExt};

fn write_response(stream: &mut TcpStream, opcode: u8, extras: &[u8], value: &[u8]) {
    let mut packet = Vec::new();
    packet.write_u8(0x81).unwrap();
    packet.write_u8(opcode).unwrap();
    packet.write_u16::<BigEndian>(0).unwrap();
    packet.write_u8(extras.len() as u8).unwrap();
    packet.write_u8(0).unwrap();
    packet.write_u16::<BigEndian>(0).unwrap();
    packet
        .write_u32::<BigEndian>((extras.len() + value.len()) as u32)
        .unwrap();
    packet.write_u32::<BigEndian>(0).unwrap();
    packet.write_u64::<BigEndian>(0).unwrap();
    packet.extend_from_slice(extras);
    packet.extend_from_slice(value);
    stream.write_all(&packet).unwrap();
}

fn serve(mut stream: TcpStream) {
    let mut header = [0u8; 24];
    while stream.read_exact(&mut header).is_ok() {
        let opcode = header[1];
        let key_length = BigEndian::read_u16(&header[2..]) as usize;
        let extras_length = header[4] as usize;
        let body_length = BigEndian::read_u32(&header[8..]) as usize;
        let mut body = vec![0u8; body_length];
        stream.read_exact(&mut body).unwrap();
        let key = &body[extras_length..extras_length + key_length];
        match opcode {
            0x0b => write_response(&mut stream, opcode, &[], b"1.6.45"),
            0x00 => {
                if key.starts_with(b"slow") {
                    thread::sleep(Duration::from_millis(500));
                }
                let value = [b"value-of-", key].concat();
                write_response(&mut stream, opcode, &[0, 0, 0, 0], &value);
            }
            _ => write_response(&mut stream, opcode, &[], &[]),
        }
    }
}

#[test]
fn test_connection_dropped_after_read_timeout() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    thread::spawn(move || {
        for stream in listener.incoming() {
            thread::spawn(move || serve(stream.unwrap()));
        }
    });

    let client = memcache::Client::builder()
        .add_server(format!("memcache://127.0.0.1:{}", port))
        .unwrap()
        .with_read_timeout(Duration::from_millis(100))
        .build()
        .unwrap();

    assert!(client.get::<String>("slow").is_err());
    thread::sleep(Duration::from_millis(600));

    let value: Option<String> = client.get("fast").unwrap();
    assert_eq!(value, Some("value-of-fast".into()));
    assert_eq!(client.delete("fast").unwrap(), true);
    let value: Option<String> = client.get("other").unwrap();
    assert_eq!(value, Some("value-of-other".into()));
}
