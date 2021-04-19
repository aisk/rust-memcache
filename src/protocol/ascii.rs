use std::collections::HashMap;
use std::fmt;
use std::io::{Read, Write};

use super::ProtocolTrait;
use crate::client::Stats;
use crate::error::{ClientError, CommandError, MemcacheError, ServerError};
use crate::stream::Stream;
use crate::value::{FromMemcacheValueExt, ToMemcacheValue};
use std::borrow::Cow;

#[derive(Default)]
pub struct Options {
    pub noreply: bool,
    pub exptime: u32,
    pub flags: u32,
    pub cas: Option<u64>,
}

#[derive(PartialEq)]
enum StoreCommand {
    Cas,
    Set,
    Add,
    Replace,
    Append,
    Prepend,
}

const END: &'static str = "END\r\n";

impl fmt::Display for StoreCommand {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match *self {
            StoreCommand::Set => write!(f, "set"),
            StoreCommand::Add => write!(f, "add"),
            StoreCommand::Replace => write!(f, "replace"),
            StoreCommand::Append => write!(f, "append"),
            StoreCommand::Prepend => write!(f, "prepend"),
            StoreCommand::Cas => write!(f, "cas"),
        }
    }
}

struct CappedLineReader<C> {
    inner: C,
    filled: usize,
    buf: [u8; 2048],
}

fn get_line(buf: &[u8]) -> Option<usize> {
    for (i, r) in buf.iter().enumerate() {
        if *r == b'\r' {
            if buf.get(i + 1) == Some(&b'\n') {
                return Some(i + 2);
            }
        }
    }
    None
}

impl<C: Read> CappedLineReader<C> {
    fn new(inner: C) -> Self {
        Self {
            inner,
            filled: 0,
            buf: [0x0; 2048],
        }
    }

    pub(crate) fn get_mut(&mut self) -> &mut C {
        &mut self.inner
    }

    fn read_exact(&mut self, buf: &mut [u8]) -> Result<(), MemcacheError> {
        let min = std::cmp::min(buf.len(), self.filled);
        let (to_fill, rest) = buf.split_at_mut(min);
        to_fill.copy_from_slice(&self.buf[..min]);
        self.consume(min);
        if rest.len() != 0 {
            self.inner.read_exact(&mut rest[..])?;
        }
        Ok(())
    }

    /// Try to read a CRLF terminated line from the underlying reader.
    /// The length of the line is expected to be <= the length of the
    /// internal buffer, suited for reading headers or short responses.
    fn read_line<T, F>(&mut self, mut cb: F) -> Result<T, MemcacheError>
    where
        F: FnMut(&str) -> Result<T, MemcacheError>,
    {
        // check if the buf already has a new line
        if let Some(n) = get_line(&self.buf[..self.filled]) {
            let result = cb(std::str::from_utf8(&self.buf[..n])?);
            self.consume(n);
            return result;
        }
        loop {
            let (filled, buf) = self.buf.split_at_mut(self.filled);
            if buf.len() == 0 {
                return Err(ClientError::Error(Cow::Borrowed("Ascii protocol response too long")))?;
            }
            let filled = filled.len();
            let read = self.inner.read(&mut buf[..])?;
            if read == 0 {
                return Err(ClientError::Error(Cow::Borrowed("Ascii protocol no line found")))?;
            }
            self.filled += read;
            if let Some(n) = get_line(&buf[..read]) {
                let result = cb(std::str::from_utf8(&self.buf[..filled + n])?);
                self.consume(n);
                return result;
            }
        }
    }

    fn consume(&mut self, amount: usize) {
        let amount = std::cmp::min(self.filled, amount);
        self.buf.copy_within(amount..self.filled, 0);
        self.filled -= amount;
    }
}

pub struct AsciiProtocol<C: Read + Write + Sized> {
    reader: CappedLineReader<C>,
}

impl ProtocolTrait for AsciiProtocol<Stream> {
    fn auth(&mut self, username: &str, password: &str) -> Result<(), MemcacheError> {
        return self.set("auth", format!("{} {}", username, password), 0);
    }

    fn version(&mut self) -> Result<String, MemcacheError> {
        self.reader.get_mut().write(b"version\r\n")?;
        self.reader.get_mut().flush()?;
        self.reader.read_line(|response| {
            let response = MemcacheError::try_from(response)?;
            if !response.starts_with("VERSION") {
                Err(ServerError::BadResponse(Cow::Owned(response.into())))?
            }
            let version = response.trim_start_matches("VERSION ").trim_end_matches("\r\n");
            Ok(version.to_string())
        })
    }

    fn flush(&mut self) -> Result<(), MemcacheError> {
        write!(self.reader.get_mut(), "flush_all\r\n")?;
        self.parse_ok_response()
    }

    fn flush_with_delay(&mut self, delay: u32) -> Result<(), MemcacheError> {
        write!(self.reader.get_mut(), "flush_all {}\r\n", delay)?;
        self.reader.get_mut().flush()?;
        self.parse_ok_response()
    }

    fn get<V: FromMemcacheValueExt>(&mut self, key: &str) -> Result<Option<V>, MemcacheError> {
        write!(self.reader.get_mut(), "get {}\r\n", key)?;

        if let Some((k, v)) = self.parse_get_response(false)? {
            if k != key {
                Err(ServerError::BadResponse(Cow::Borrowed(
                    "key doesn't match in the response",
                )))?
            } else if self.parse_get_response::<V>(false)?.is_none() {
                Ok(Some(v))
            } else {
                Err(ServerError::BadResponse(Cow::Borrowed("Expected end of get response")))?
            }
        } else {
            Ok(None)
        }
    }

    fn gets<V: FromMemcacheValueExt>(&mut self, keys: &[&str]) -> Result<HashMap<String, V>, MemcacheError> {
        write!(self.reader.get_mut(), "gets {}\r\n", keys.join(" "))?;

        let mut result: HashMap<String, V> = HashMap::with_capacity(keys.len());
        // there will be atmost keys.len() "VALUE <...>" responses and one END response
        for _ in 0..=keys.len() {
            match self.parse_get_response(true)? {
                Some((key, value)) => {
                    result.insert(key, value);
                }
                None => return Ok(result),
            }
        }

        Err(ServerError::BadResponse(Cow::Borrowed("Expected end of gets response")))?
    }

    fn cas<V: ToMemcacheValue<Stream>>(
        &mut self,
        key: &str,
        value: V,
        expiration: u32,
        cas: u64,
    ) -> Result<bool, MemcacheError> {
        let options = Options {
            exptime: expiration,
            cas: Some(cas),
            ..Default::default()
        };
        match self.store(StoreCommand::Cas, key, value, &options) {
            Ok(t) => Ok(t),
            Err(MemcacheError::CommandError(e)) if e == CommandError::KeyExists || e == CommandError::KeyNotFound => {
                Ok(false)
            }
            e => e,
        }
    }

    fn set<V: ToMemcacheValue<Stream>>(&mut self, key: &str, value: V, expiration: u32) -> Result<(), MemcacheError> {
        let options = Options {
            exptime: expiration,
            ..Default::default()
        };
        self.store(StoreCommand::Set, key, value, &options).map(|_| ())
    }

    fn add<V: ToMemcacheValue<Stream>>(&mut self, key: &str, value: V, expiration: u32) -> Result<(), MemcacheError> {
        let options = Options {
            exptime: expiration,
            ..Default::default()
        };
        self.store(StoreCommand::Add, key, value, &options).map(|_| ())
    }

    fn replace<V: ToMemcacheValue<Stream>>(
        &mut self,
        key: &str,
        value: V,
        expiration: u32,
    ) -> Result<(), MemcacheError> {
        let options = Options {
            exptime: expiration,
            ..Default::default()
        };
        self.store(StoreCommand::Replace, key, value, &options).map(|_| ())
    }

    fn append<V: ToMemcacheValue<Stream>>(&mut self, key: &str, value: V) -> Result<(), MemcacheError> {
        self.store(StoreCommand::Append, key, value, &Default::default())
            .map(|_| ())
    }

    fn prepend<V: ToMemcacheValue<Stream>>(&mut self, key: &str, value: V) -> Result<(), MemcacheError> {
        self.store(StoreCommand::Prepend, key, value, &Default::default())
            .map(|_| ())
    }

    fn delete(&mut self, key: &str) -> Result<bool, MemcacheError> {
        write!(self.reader.get_mut(), "delete {}\r\n", key)?;
        self.reader.get_mut().flush()?;
        self.reader
            .read_line(|response| match MemcacheError::try_from(response) {
                Ok(s) => {
                    if s == "DELETED\r\n" {
                        Ok(true)
                    } else {
                        Err(ServerError::BadResponse(Cow::Owned(s.into())).into())
                    }
                }
                Err(MemcacheError::CommandError(CommandError::KeyNotFound)) => Ok(false),
                Err(e) => Err(e),
            })
    }

    fn increment(&mut self, key: &str, amount: u64) -> Result<u64, MemcacheError> {
        write!(self.reader.get_mut(), "incr {} {}\r\n", key, amount)?;
        self.parse_u64_response()
    }

    fn decrement(&mut self, key: &str, amount: u64) -> Result<u64, MemcacheError> {
        write!(self.reader.get_mut(), "decr {} {}\r\n", key, amount)?;
        self.parse_u64_response()
    }

    fn touch(&mut self, key: &str, expiration: u32) -> Result<bool, MemcacheError> {
        write!(self.reader.get_mut(), "touch {} {}\r\n", key, expiration)?;
        self.reader.get_mut().flush()?;
        self.reader
            .read_line(|response| match MemcacheError::try_from(response) {
                Ok(s) => {
                    if s == "TOUCHED\r\n" {
                        Ok(true)
                    } else {
                        Err(ServerError::BadResponse(Cow::Owned(s.into())).into())
                    }
                }
                Err(MemcacheError::CommandError(CommandError::KeyNotFound)) => Ok(false),
                Err(e) => Err(e),
            })
    }

    fn stats(&mut self) -> Result<Stats, MemcacheError> {
        self.reader.get_mut().write(b"stats\r\n")?;
        self.reader.get_mut().flush()?;

        enum Loop {
            Break,
            Continue,
        }

        let mut stats: Stats = HashMap::new();
        loop {
            let status = self.reader.read_line(|response| {
                if response != END {
                    return Ok(Loop::Break);
                }
                let s = MemcacheError::try_from(response)?;
                if !s.starts_with("STAT") {
                    return Err(ServerError::BadResponse(Cow::Owned(s.into())))?;
                }
                let stat: Vec<_> = s.trim_end_matches("\r\n").split(" ").collect();
                if stat.len() < 3 {
                    return Err(ServerError::BadResponse(Cow::Owned(s.into())).into());
                }
                let key = stat[1];
                let value = s.trim_start_matches(format!("STAT {}", key).as_str());
                stats.insert(key.into(), value.into());

                Ok(Loop::Continue)
            })?;

            if let Loop::Break = status {
                break Ok(stats);
            }
        }
    }
}

impl AsciiProtocol<Stream> {
    pub(crate) fn new(stream: Stream) -> Self {
        Self {
            reader: CappedLineReader::new(stream),
        }
    }

    pub(crate) fn stream(&mut self) -> &mut Stream {
        self.reader.get_mut()
    }

    fn store<V: ToMemcacheValue<Stream>>(
        &mut self,
        command: StoreCommand,
        key: &str,
        value: V,
        options: &Options,
    ) -> Result<bool, MemcacheError> {
        if command == StoreCommand::Cas {
            if options.cas.is_none() {
                Err(ClientError::Error(Cow::Borrowed(
                    "cas_id should be present when using cas command",
                )))?;
            }
        }
        let noreply = if options.noreply { " noreply" } else { "" };
        if options.cas.is_some() {
            write!(
                self.reader.get_mut(),
                "{command} {key} {flags} {exptime} {vlen} {cas}{noreply}\r\n",
                command = command,
                key = key,
                flags = value.get_flags(),
                exptime = options.exptime,
                vlen = value.get_length(),
                cas = options.cas.unwrap(),
                noreply = noreply
            )?;
        } else {
            write!(
                self.reader.get_mut(),
                "{command} {key} {flags} {exptime} {vlen}{noreply}\r\n",
                command = command,
                key = key,
                flags = value.get_flags(),
                exptime = options.exptime,
                vlen = value.get_length(),
                noreply = noreply
            )?;
        }

        value.write_to(self.reader.get_mut())?;
        self.reader.get_mut().write(b"\r\n")?;
        self.reader.get_mut().flush()?;

        if options.noreply {
            return Ok(true);
        }

        self.reader.read_line(|response| {
            let response = MemcacheError::try_from(response)?;
            match response {
                "STORED\r\n" => Ok(true),
                "NOT_STORED\r\n" => Ok(false),
                "EXISTS\r\n" => Err(CommandError::KeyExists)?,
                "NOT_FOUND\r\n" => Err(CommandError::KeyNotFound)?,
                response => Err(ServerError::BadResponse(Cow::Owned(response.into())))?,
            }
        })
    }

    fn parse_ok_response(&mut self) -> Result<(), MemcacheError> {
        self.reader.read_line(|response| {
            let response = MemcacheError::try_from(response)?;
            if response == "OK\r\n" {
                Ok(())
            } else {
                Err(ServerError::BadResponse(Cow::Owned(response.into())))?
            }
        })
    }

    fn parse_get_response<V: FromMemcacheValueExt>(
        &mut self,
        has_cas: bool,
    ) -> Result<Option<(String, V)>, MemcacheError> {
        let result = self.reader.read_line(|buf| {
            let buf = MemcacheError::try_from(buf)?;
            if buf == END {
                return Ok(None);
            }
            if !buf.starts_with("VALUE") {
                return Err(ServerError::BadResponse(Cow::Owned(buf.into())))?;
            }
            let mut header = buf.trim_end_matches("\r\n").split(" ");
            let mut next_or_err = || {
                header
                    .next()
                    .ok_or_else(|| ServerError::BadResponse(Cow::Owned(buf.into())))
            };
            let _ = next_or_err()?;
            let key = next_or_err()?;
            let flags: u32 = next_or_err()?.parse()?;
            let length: usize = next_or_err()?.parse()?;
            let cas: Option<u64> = if has_cas { Some(next_or_err()?.parse()?) } else { None };
            if header.next().is_some() {
                return Err(ServerError::BadResponse(Cow::Owned(buf.into())))?;
            }
            Ok(Some((key.to_string(), flags, length, cas)))
        })?;
        match result {
            Some((key, flags, length, cas)) => {
                let mut value = vec![0u8; length + 2];
                self.reader.read_exact(value.as_mut_slice())?;
                if &value[length..] != b"\r\n" {
                    return Err(ServerError::BadResponse(Cow::Owned(String::from_utf8(value)?)))?;
                }
                // remove the trailing \r\n
                value.pop();
                value.pop();
                value.shrink_to_fit();
                let value = FromMemcacheValueExt::from_memcache_value(value, flags, cas)?;
                Ok(Some((key.to_string(), value)))
            }
            None => Ok(None),
        }
    }

    fn parse_u64_response(&mut self) -> Result<u64, MemcacheError> {
        self.reader.read_line(|response| {
            let s = MemcacheError::try_from(response)?;
            Ok(s.trim_end_matches("\r\n").parse::<u64>()?)
        })
    }
}
