//! Tokio TCP transport for meta protocol commands.

use std::borrow::Cow;
use std::io;

use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::net::{TcpStream, ToSocketAddrs};

use crate::error::{MemcacheError, ServerError};

use super::meta_command::{MetaCommand, MetaResponse};

/// A buffered tokio TCP connection that speaks the meta protocol.
///
/// The async counterpart of [`MetaConnection`](super::MetaConnection), with
/// the same framing rules: one response per [`receive`](Self::receive),
/// quiet-mode (`q`) commands are not handled here.
pub struct AsyncMetaConnection {
    reader: BufReader<TcpStream>,
}

impl AsyncMetaConnection {
    pub async fn connect<A: ToSocketAddrs>(addr: A) -> Result<AsyncMetaConnection, MemcacheError> {
        let stream = TcpStream::connect(addr).await?;
        stream.set_nodelay(true)?;
        Ok(AsyncMetaConnection::from_stream(stream))
    }

    pub fn from_stream(stream: TcpStream) -> AsyncMetaConnection {
        AsyncMetaConnection {
            reader: BufReader::new(stream),
        }
    }

    /// Whether an idle connection is still fit for reuse. A synchronized
    /// stream is quiet between exchanges, so any buffered or pending bytes
    /// mean desynchronization, and a readable EOF means the peer (server
    /// restart, LB, NAT) closed the connection while it sat in the pool.
    /// The check is a non-blocking read: no round trip.
    pub(crate) fn is_reusable(&self) -> bool {
        if !self.reader.buffer().is_empty() {
            return false;
        }
        let mut probe = [0u8; 1];
        match self.reader.get_ref().try_read(&mut probe) {
            // 0 is EOF, anything else is a stray byte: dead either way.
            Ok(_) => false,
            Err(error) => error.kind() == io::ErrorKind::WouldBlock,
        }
    }

    /// Encode and write a single command.
    pub async fn send(&mut self, command: &MetaCommand) -> Result<(), MemcacheError> {
        let payload = command.encode()?;
        self.reader.write_all(&payload).await?;
        self.reader.flush().await?;
        Ok(())
    }

    /// Read one framed response, including the data block of a `VA` response.
    pub async fn receive(&mut self) -> Result<MetaResponse, MemcacheError> {
        let line = self.read_line().await?;
        let mut response = MetaResponse::parse_header(&line)?;
        if let Some(datalen) = response.datalen {
            let mut value = vec![0u8; datalen + 2];
            self.reader.read_exact(&mut value).await?;
            if &value[datalen..] != b"\r\n" {
                return Err(ServerError::BadResponse(Cow::Borrowed("data block missing CRLF terminator")).into());
            }
            value.truncate(datalen);
            response.value = Some(value);
        }
        Ok(response)
    }

    /// Send a command and read its response.
    pub async fn execute(&mut self, command: &MetaCommand) -> Result<MetaResponse, MemcacheError> {
        self.send(command).await?;
        self.receive().await
    }

    /// Write all commands in one payload, then read one response per
    /// command. Quiet-mode (`q`) commands would desynchronize the stream and
    /// must not be used here.
    pub async fn execute_batch(&mut self, commands: &[MetaCommand]) -> Result<Vec<MetaResponse>, MemcacheError> {
        let mut payload = Vec::new();
        for command in commands {
            command.encode_into(&mut payload)?;
        }
        self.reader.write_all(&payload).await?;
        self.reader.flush().await?;
        let mut responses = Vec::with_capacity(commands.len());
        for _ in commands {
            responses.push(self.receive().await?);
        }
        Ok(responses)
    }

    async fn read_line(&mut self) -> Result<Vec<u8>, MemcacheError> {
        let mut line = Vec::new();
        self.reader.read_until(b'\n', &mut line).await?;
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
