use std::collections::HashMap;
use std::fmt;
use std::io::{BufRead, BufReader, Read, Write};

use super::check_key_len;
use client::Stats;
use error::{ClientError, CommandError, MemcacheError, ServerError};
use std::borrow::Cow;
use stream::Stream;
use value::{FromMemcacheValueExt, ToMemcacheValue};

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
pub struct AsciiProtocol<C: Read + Write + Sized> {
    pub reader: BufReader<C>,
}

impl AsciiProtocol<Stream> {
    pub(super) fn auth(&mut self, username: &str, password: &str) -> Result<(), MemcacheError> {
        return self.set("auth", format!("{} {}", username, password), 0);
    }

    fn store<V: ToMemcacheValue<Stream>>(
        &mut self,
        command: StoreCommand,
        key: &str,
        value: V,
        options: &Options,
    ) -> Result<bool, MemcacheError> {
        check_key_len(key)?;
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

        let mut s = String::new();
        self.reader.read_line(&mut s)?;
        match MemcacheError::try_from(s) {
            Ok(s) if s == "STORED\r\n" => Ok(true),
            Ok(s) if s == "NOT_STORED\r\n" => Ok(false),
            Ok(s) => {
                if s == "EXISTS\r\n" {
                    Err(CommandError::KeyExists)?
                } else if s == "NOT_FOUND\r\n" {
                    Err(CommandError::KeyNotFound)?
                } else {
                    Err(ServerError::BadResponse(s))?
                }
            }
            Err(e) => Err(e),
        }
    }

    pub(super) fn version(&mut self) -> Result<String, MemcacheError> {
        self.reader.get_mut().write(b"version\r\n")?;
        self.reader.get_mut().flush()?;
        let mut s = String::new();
        self.reader.read_line(&mut s)?;
        let s = MemcacheError::try_from(s)?;
        if !s.starts_with("VERSION") {
            return Err(ServerError::BadResponse(s).into());
        }
        let s = s.trim_start_matches("VERSION ");
        let s = s.trim_end_matches("\r\n");

        return Ok(s.to_string());
    }

    pub(super) fn flush(&mut self) -> Result<(), MemcacheError> {
        match self.reader.get_mut().write(b"flush_all\r\n") {
            Ok(_) => {}
            Err(err) => return Err(MemcacheError::from(err)),
        }
        self.reader.get_mut().flush()?;
        let mut s = String::new();
        self.reader.read_line(&mut s)?;
        let s = MemcacheError::try_from(s)?;
        if s != "OK\r\n" {
            return Err(ServerError::BadResponse(s).into());
        }
        return Ok(());
    }

    pub(super) fn flush_with_delay(&mut self, delay: u32) -> Result<(), MemcacheError> {
        write!(self.reader.get_mut(), "flush_all {}\r\n", delay)?;
        self.reader.get_mut().flush()?;
        let mut s = String::new();
        self.reader.read_line(&mut s)?;
        let s = MemcacheError::try_from(s)?;
        if s != "OK\r\n" {
            return Err(ServerError::BadResponse(s).into());
        }
        return Ok(());
    }

    pub(super) fn get<V: FromMemcacheValueExt>(&mut self, key: &str) -> Result<Option<V>, MemcacheError> {
        write!(self.reader.get_mut(), "get {}\r\n", key)?;

        let mut s = String::new();
        self.reader.read_line(&mut s)?;

        let s = MemcacheError::try_from(s)?;
        if s.starts_with("END") {
            return Ok(None);
        } else if !s.starts_with("VALUE") {
            return Err(ServerError::BadResponse(s).into());
        }

        let header: Vec<_> = s.trim_end_matches("\r\n").split(" ").collect();
        if header.len() != 4 {
            return Err(ServerError::BadResponse(s).into());
        }

        if key != header[1] {
            return Err(ServerError::BadResponse(s).into());
        }
        let flags = header[2].parse()?;
        let length = header[3].parse()?;

        let mut buffer = vec![0; length];
        self.reader.read_exact(buffer.as_mut_slice())?;

        // read the rest \r\n and END\r\n
        let mut s = String::new();
        self.reader.read_line(&mut s)?;
        if s != "\r\n" {
            return Err(ServerError::BadResponse(s).into());
        }
        s = String::new();
        self.reader.read_line(&mut s)?;
        if s != "END\r\n" {
            return Err(ServerError::BadResponse(s).into());
        }

        return Ok(Some(FromMemcacheValueExt::from_memcache_value(buffer, flags, None)?));
    }

    pub(super) fn gets<V: FromMemcacheValueExt>(&mut self, keys: &[&str]) -> Result<HashMap<String, V>, MemcacheError> {
        write!(self.reader.get_mut(), "gets {}\r\n", keys.join(" "))?;

        let mut result: HashMap<String, V> = HashMap::new();
        loop {
            let mut s = String::new();
            self.reader.read_line(&mut s)?;

            let s = MemcacheError::try_from(s)?;
            if s.starts_with("END") {
                break;
            } else if !s.starts_with("VALUE") {
                return Err(ServerError::BadResponse(s).into());
            }

            let header: Vec<_> = s.trim_end_matches("\r\n").split(" ").collect();
            if header.len() != 5 {
                return Err(ServerError::BadResponse(s).into());
            }

            let key = header[1];
            let flags = header[2].parse()?;
            let length = header[3].parse()?;
            let cas = header[4].parse()?;

            let mut buffer = vec![0; length];
            self.reader.read_exact(buffer.as_mut_slice())?;

            result.insert(
                key.to_string(),
                FromMemcacheValueExt::from_memcache_value(buffer, flags, Some(cas))?,
            );

            // read the rest \r\n
            let mut s = String::new();
            self.reader.read_line(&mut s)?;
            if s != "\r\n" {
                return Err(ServerError::BadResponse(s).into());
            }
        }

        return Ok(result);
    }

    pub(super) fn cas<V: ToMemcacheValue<Stream>>(
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

    pub(super) fn set<V: ToMemcacheValue<Stream>>(
        &mut self,
        key: &str,
        value: V,
        expiration: u32,
    ) -> Result<(), MemcacheError> {
        let options = Options {
            exptime: expiration,
            ..Default::default()
        };
        self.store(StoreCommand::Set, key, value, &options).map(|_| ())
    }

    pub(super) fn add<V: ToMemcacheValue<Stream>>(
        &mut self,
        key: &str,
        value: V,
        expiration: u32,
    ) -> Result<(), MemcacheError> {
        let options = Options {
            exptime: expiration,
            ..Default::default()
        };
        self.store(StoreCommand::Add, key, value, &options).map(|_| ())
    }

    pub(super) fn replace<V: ToMemcacheValue<Stream>>(
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

    pub(super) fn append<V: ToMemcacheValue<Stream>>(&mut self, key: &str, value: V) -> Result<(), MemcacheError> {
        check_key_len(key)?;
        self.store(StoreCommand::Append, key, value, &Default::default())
            .map(|_| ())
    }

    pub(super) fn prepend<V: ToMemcacheValue<Stream>>(&mut self, key: &str, value: V) -> Result<(), MemcacheError> {
        check_key_len(key)?;
        self.store(StoreCommand::Prepend, key, value, &Default::default())
            .map(|_| ())
    }

    pub(super) fn delete(&mut self, key: &str) -> Result<bool, MemcacheError> {
        check_key_len(key)?;
        write!(self.reader.get_mut(), "delete {}\r\n", key)?;
        self.reader.get_mut().flush()?;
        let mut s = String::new();
        self.reader.read_line(&mut s)?;
        match MemcacheError::try_from(s) {
            Ok(s) => {
                if s == "DELETED\r\n" {
                    Ok(true)
                } else {
                    Err(ServerError::BadResponse(s))?
                }
            }
            Err(MemcacheError::CommandError(CommandError::KeyNotFound)) => Ok(false),
            Err(e) => Err(e),
        }
    }

    pub(super) fn increment(&mut self, key: &str, amount: u64) -> Result<u64, MemcacheError> {
        check_key_len(key)?;
        write!(self.reader.get_mut(), "incr {} {}\r\n", key, amount)?;
        let mut s = String::new();
        self.reader.read_line(&mut s)?;
        let s = MemcacheError::try_from(s)?;
        Ok(s.trim_end_matches("\r\n").parse::<u64>()?)
    }

    pub(super) fn decrement(&mut self, key: &str, amount: u64) -> Result<u64, MemcacheError> {
        check_key_len(key)?;
        write!(self.reader.get_mut(), "decr {} {}\r\n", key, amount)?;
        let mut s = String::new();
        self.reader.read_line(&mut s)?;
        let s = MemcacheError::try_from(s)?;
        Ok(s.trim_end_matches("\r\n").parse::<u64>()?)
    }

    pub(super) fn touch(&mut self, key: &str, expiration: u32) -> Result<bool, MemcacheError> {
        check_key_len(key)?;
        write!(self.reader.get_mut(), "touch {} {}\r\n", key, expiration)?;
        self.reader.get_mut().flush()?;
        let mut s = String::new();
        self.reader.read_line(&mut s)?;
        match MemcacheError::try_from(s) {
            Ok(s) => {
                if s == "TOUCHED\r\n" {
                    Ok(true)
                } else {
                    Err(ServerError::BadResponse(s))?
                }
            }
            Err(MemcacheError::CommandError(CommandError::KeyNotFound)) => Ok(false),
            Err(e) => Err(e),
        }
    }

    pub(super) fn stats(&mut self) -> Result<Stats, MemcacheError> {
        self.reader.get_mut().write(b"stats\r\n")?;
        self.reader.get_mut().flush()?;

        let mut result: Stats = HashMap::new();
        loop {
            let mut s = String::new();
            self.reader.read_line(&mut s)?;

            let s = MemcacheError::try_from(s)?;
            // FIXME: what if a stat starts with END?
            if s.starts_with("END") {
                break;
            } else if !s.starts_with("STAT") {
                return Err(ServerError::BadResponse(s).into());
            }

            let stat: Vec<_> = s.trim_end_matches("\r\n").split(" ").collect();
            if stat.len() < 3 {
                return Err(ServerError::BadResponse(s).into());
            }
            let key = stat[1];
            let value = s.trim_start_matches(format!("STAT {}", key).as_str());
            result.insert(key.into(), value.into());
        }

        return Ok(result);
    }
}
