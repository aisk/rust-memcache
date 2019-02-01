use std::collections::HashMap;
use std::io::Write;

use byteorder::{WriteBytesExt, BigEndian};
use client::Stats;
use error::MemcacheError;
use protocol::binary_packet::{self, Opcode, PacketHeader, Magic};
use stream::Stream;
use value::{ToMemcacheValue, FromMemcacheValue};

pub struct AsciiProtocol {
    pub stream: Stream,
}
