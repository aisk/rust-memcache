//! Blocking TCP transport for meta protocol commands.

use std::borrow::Cow;
use std::io::{self, BufRead, BufReader, Read, Write};
use std::net::{TcpStream, ToSocketAddrs};

use crate::error::{MemcacheError, ServerError};

use super::meta_command::{MetaCommand, MetaResponse};

/// A buffered, blocking TCP connection that speaks the meta protocol.
///
/// This is a thin transport: it writes encoded commands and reads framed
/// responses, one at a time. Quiet-mode (`q`) commands that suppress
/// successful responses are not handled here; [`receive`](Self::receive)
/// always expects a response line.
pub struct MetaConnection {
    reader: BufReader<TcpStream>,
}

impl MetaConnection {
    pub fn connect<A: ToSocketAddrs>(addr: A) -> Result<MetaConnection, MemcacheError> {
        let stream = TcpStream::connect(addr)?;
        stream.set_nodelay(true)?;
        Ok(MetaConnection::from_stream(stream))
    }

    pub fn from_stream(stream: TcpStream) -> MetaConnection {
        MetaConnection {
            reader: BufReader::new(stream),
        }
    }

    /// Encode and write a single command.
    pub fn send(&mut self, command: &MetaCommand) -> Result<(), MemcacheError> {
        let payload = command.encode()?;
        self.reader.get_mut().write_all(&payload)?;
        Ok(())
    }

    /// Read one framed response, including the data block of a `VA` response.
    pub fn receive(&mut self) -> Result<MetaResponse, MemcacheError> {
        let line = self.read_line()?;
        let mut response = MetaResponse::parse_header(&line)?;
        if let Some(datalen) = response.datalen {
            let mut value = vec![0u8; datalen + 2];
            self.reader.read_exact(&mut value)?;
            if &value[datalen..] != b"\r\n" {
                return Err(ServerError::BadResponse(Cow::Borrowed("data block missing CRLF terminator")).into());
            }
            value.truncate(datalen);
            response.value = Some(value);
        }
        Ok(response)
    }

    /// Send a command and read its response.
    pub fn execute(&mut self, command: &MetaCommand) -> Result<MetaResponse, MemcacheError> {
        self.send(command)?;
        self.receive()
    }

    /// Write all commands in one payload, then read one response per
    /// command. Quiet-mode (`q`) commands would desynchronize the stream and
    /// must not be used here.
    pub fn execute_batch(&mut self, commands: &[MetaCommand]) -> Result<Vec<MetaResponse>, MemcacheError> {
        let mut payload = Vec::new();
        for command in commands {
            command.encode_into(&mut payload)?;
        }
        self.reader.get_mut().write_all(&payload)?;
        let mut responses = Vec::with_capacity(commands.len());
        for _ in commands {
            responses.push(self.receive()?);
        }
        Ok(responses)
    }

    fn read_line(&mut self) -> Result<Vec<u8>, MemcacheError> {
        let mut line = Vec::new();
        self.reader.read_until(b'\n', &mut line)?;
        if !line.ends_with(b"\n") {
            return Err(io::Error::from(io::ErrorKind::UnexpectedEof).into());
        }
        line.pop();
        if line.ends_with(b"\r") {
            line.pop();
        }
        Ok(line)
    }
}
