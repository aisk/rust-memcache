//! Blocking TCP transport for meta protocol commands.

use std::borrow::Cow;
use std::io::{self, BufRead, BufReader, Read, Write};
use std::net::{TcpStream, ToSocketAddrs};
use std::time::{Duration, Instant};

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
    io_timeout: Option<Duration>,
}

impl MetaConnection {
    /// Connect without a time limit; combine `TcpStream::connect_timeout`
    /// with [`from_stream`](Self::from_stream) to bound the dial.
    pub fn connect<A: ToSocketAddrs>(addr: A) -> Result<MetaConnection, MemcacheError> {
        let stream = TcpStream::connect(addr)?;
        stream.set_nodelay(true)?;
        Ok(MetaConnection::from_stream(stream))
    }

    pub fn from_stream(stream: TcpStream) -> MetaConnection {
        MetaConnection {
            reader: BufReader::new(stream),
            io_timeout: None,
        }
    }

    /// Set (or clear) the I/O time limit. It bounds each exchange as a
    /// whole - one command or batch, request write plus response reads -
    /// matching the tokio transport, not individual socket operations.
    pub fn set_io_timeout(&mut self, timeout: Option<Duration>) {
        self.io_timeout = timeout;
    }

    /// The deadline for an exchange starting now.
    fn deadline(&self) -> Option<Instant> {
        self.io_timeout.map(|timeout| Instant::now() + timeout)
    }

    /// Arm the socket timeouts for the next syscall with the time left
    /// until the deadline; an already-spent deadline is a timeout.
    fn arm(&self, deadline: Option<Instant>) -> Result<(), MemcacheError> {
        let remaining = match deadline {
            Some(deadline) => match deadline.checked_duration_since(Instant::now()) {
                // A zero socket timeout would mean "unlimited"; a spent
                // deadline is a timeout either way.
                Some(remaining) if !remaining.is_zero() => Some(remaining),
                _ => return Err(io::Error::from(io::ErrorKind::TimedOut).into()),
            },
            None => None,
        };
        let stream = self.reader.get_ref();
        stream.set_read_timeout(remaining)?;
        stream.set_write_timeout(remaining)?;
        Ok(())
    }

    /// Whether an idle connection is still fit for reuse. A synchronized
    /// stream is quiet between exchanges, so any buffered or pending bytes
    /// mean desynchronization, and a readable EOF means the peer (server
    /// restart, LB, NAT) closed the connection while it sat in the pool.
    /// The check is a non-blocking peek: no round trip.
    pub(crate) fn is_reusable(&mut self) -> bool {
        if !self.reader.buffer().is_empty() {
            return false;
        }
        let stream = self.reader.get_ref();
        if stream.set_nonblocking(true).is_err() {
            return false;
        }
        let mut probe = [0u8; 1];
        let reusable = match stream.peek(&mut probe) {
            // 0 is EOF, anything else is a stray byte: dead either way.
            Ok(_) => false,
            Err(error) => error.kind() == io::ErrorKind::WouldBlock,
        };
        stream.set_nonblocking(false).is_ok() && reusable
    }

    /// Encode and write a single command under its own deadline.
    pub fn send(&mut self, command: &MetaCommand) -> Result<(), MemcacheError> {
        self.send_by(self.deadline(), command)
    }

    /// Read one framed response, including the data block of a `VA`
    /// response, under its own deadline.
    pub fn receive(&mut self) -> Result<MetaResponse, MemcacheError> {
        self.receive_by(self.deadline())
    }

    /// Send a command and read its response under one shared deadline.
    pub fn execute(&mut self, command: &MetaCommand) -> Result<MetaResponse, MemcacheError> {
        let deadline = self.deadline();
        self.send_by(deadline, command)?;
        self.receive_by(deadline)
    }

    /// Write all commands in one payload, then read one response per
    /// command, all under one shared deadline. Quiet-mode (`q`) commands
    /// would desynchronize the stream and must not be used here.
    pub fn execute_batch(&mut self, commands: &[MetaCommand]) -> Result<Vec<MetaResponse>, MemcacheError> {
        let deadline = self.deadline();
        let mut payload = Vec::new();
        for command in commands {
            command.encode_into(&mut payload)?;
        }
        self.write_all_by(deadline, &payload)?;
        let mut responses = Vec::with_capacity(commands.len());
        for _ in commands {
            responses.push(self.receive_by(deadline)?);
        }
        Ok(responses)
    }

    fn send_by(&mut self, deadline: Option<Instant>, command: &MetaCommand) -> Result<(), MemcacheError> {
        let payload = command.encode()?;
        self.write_all_by(deadline, &payload)
    }

    fn receive_by(&mut self, deadline: Option<Instant>) -> Result<MetaResponse, MemcacheError> {
        let line = self.read_line(deadline)?;
        let mut response = MetaResponse::parse_header(&line)?;
        if let Some(datalen) = response.datalen {
            let mut value = vec![0u8; datalen + 2];
            self.read_exact_by(deadline, &mut value)?;
            if &value[datalen..] != b"\r\n" {
                return Err(ServerError::BadResponse(Cow::Borrowed("data block missing CRLF terminator")).into());
            }
            value.truncate(datalen);
            response.value = Some(value);
        }
        Ok(response)
    }

    fn write_all_by(&mut self, deadline: Option<Instant>, mut payload: &[u8]) -> Result<(), MemcacheError> {
        while !payload.is_empty() {
            self.arm(deadline)?;
            let written = self.reader.get_mut().write(payload)?;
            if written == 0 {
                return Err(io::Error::from(io::ErrorKind::WriteZero).into());
            }
            payload = &payload[written..];
        }
        Ok(())
    }

    fn read_exact_by(&mut self, deadline: Option<Instant>, mut buffer: &mut [u8]) -> Result<(), MemcacheError> {
        while !buffer.is_empty() {
            self.arm(deadline)?;
            let read = self.reader.read(buffer)?;
            if read == 0 {
                return Err(io::Error::from(io::ErrorKind::UnexpectedEof).into());
            }
            buffer = &mut buffer[read..];
        }
        Ok(())
    }

    fn read_line(&mut self, deadline: Option<Instant>) -> Result<Vec<u8>, MemcacheError> {
        let mut line = Vec::new();
        loop {
            self.arm(deadline)?;
            let buffer = self.reader.fill_buf()?;
            if buffer.is_empty() {
                return Err(io::Error::from(io::ErrorKind::UnexpectedEof).into());
            }
            match buffer.iter().position(|&byte| byte == b'\n') {
                Some(position) => {
                    line.extend_from_slice(&buffer[..position]);
                    self.reader.consume(position + 1);
                    break;
                }
                None => {
                    let length = buffer.len();
                    line.extend_from_slice(buffer);
                    self.reader.consume(length);
                }
            }
        }
        if line.ends_with(b"\r") {
            line.pop();
        }
        Ok(line)
    }
}
