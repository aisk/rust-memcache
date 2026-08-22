//! Tokio TCP transport for meta protocol commands.

use std::io;

use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::net::{TcpStream, ToSocketAddrs};

use super::error::{Error, Result};

use super::meta_command::{MetaCommand, MetaResponse};

/// A buffered tokio TCP connection that speaks the meta protocol.
///
/// The async counterpart of [`MetaConnection`](super::MetaConnection), with
/// the same framing rules: one response per [`receive`](Self::receive),
/// quiet-mode (`q`) commands are not handled here.
pub struct AsyncMetaConnection {
    reader: BufReader<TcpStream>,
    /// Bytes of the current (or last) exchange handed to the socket.
    written: usize,
}

impl AsyncMetaConnection {
    pub async fn connect<A: ToSocketAddrs>(addr: A) -> Result<AsyncMetaConnection> {
        let stream = TcpStream::connect(addr).await?;
        stream.set_nodelay(true)?;
        Ok(AsyncMetaConnection::from_stream(stream))
    }

    pub fn from_stream(stream: TcpStream) -> AsyncMetaConnection {
        AsyncMetaConnection {
            reader: BufReader::new(stream),
            written: 0,
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

    /// Bytes of the last exchange handed to the socket. Zero after a
    /// failure means the request never left this process.
    pub(crate) fn written(&self) -> usize {
        self.written
    }

    /// Encode and write a single command.
    pub async fn send(&mut self, command: &MetaCommand) -> Result<()> {
        let payload = command.encode()?;
        self.write_payload(&payload).await
    }

    /// Write in one go, counting what the socket accepted. `write_all` on
    /// a raw tokio stream hands bytes to the kernel as it goes, so a
    /// failure part way still means some of the payload may have landed;
    /// the count is tracked per chunk to keep the attribution honest.
    async fn write_payload(&mut self, mut payload: &[u8]) -> Result<()> {
        self.written = 0;
        while !payload.is_empty() {
            let written = self.reader.write(payload).await?;
            if written == 0 {
                return Err(io::Error::from(io::ErrorKind::WriteZero).into());
            }
            self.written += written;
            payload = &payload[written..];
        }
        self.reader.flush().await?;
        Ok(())
    }

    /// Read one framed response, including the data block of a `VA` response.
    pub async fn receive(&mut self) -> Result<MetaResponse> {
        let line = self.read_line().await?;
        let mut response = MetaResponse::parse_header(&line)?;
        if let Some(datalen) = response.datalen {
            let mut value = vec![0u8; datalen + 2];
            self.reader.read_exact(&mut value).await?;
            if &value[datalen..] != b"\r\n" {
                return Err(Error::protocol("data block missing CRLF terminator"));
            }
            value.truncate(datalen);
            response.value = Some(value);
        }
        Ok(response)
    }

    /// Send a command and read its response.
    pub async fn execute(&mut self, command: &MetaCommand) -> Result<MetaResponse> {
        self.send(command).await?;
        self.receive().await
    }

    /// Write all commands in one payload, then read one response per
    /// command. Quiet-mode (`q`) commands would desynchronize the stream and
    /// must not be used here.
    pub async fn execute_batch(&mut self, commands: &[MetaCommand]) -> Result<Vec<MetaResponse>> {
        let mut payload = Vec::new();
        for command in commands {
            command.encode_into(&mut payload)?;
        }
        let (responses, error) = self.execute_payload(&payload, commands.len()).await;
        match error {
            Some(error) => Err(error),
            None => Ok(responses),
        }
    }

    /// Write a pre-encoded batch payload and read `count` responses,
    /// keeping the responses read before a failure so the caller can
    /// attribute it; [`written`](Self::written) says how much of the
    /// payload left the process.
    pub(crate) async fn execute_payload(&mut self, payload: &[u8], count: usize) -> (Vec<MetaResponse>, Option<Error>) {
        let mut responses = Vec::with_capacity(count);
        if let Err(error) = self.write_payload(payload).await {
            return (responses, Some(error));
        }
        for _ in 0..count {
            match self.receive().await {
                Ok(response) => responses.push(response),
                Err(error) => return (responses, Some(error)),
            }
        }
        (responses, None)
    }

    async fn read_line(&mut self) -> Result<Vec<u8>> {
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
