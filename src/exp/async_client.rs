//! Tokio single-server client over the semantic layer.

use std::collections::HashMap;

use tokio::net::ToSocketAddrs;

use crate::error::{MemcacheError, ServerError};

use super::async_connection::AsyncMetaConnection;
use super::core::Operation;
use super::meta_api::{ArithmeticMode, build_debug, build_noop, parse_debug_result, parse_meta_result};
use super::meta_command::ReturnCode;
use super::operation::{Arithmetic, Delete, Get, Set};
use super::request::Request;

/// The async counterpart of [`MetaClient`](super::MetaClient); the same
/// request-builder surface over a tokio connection.
///
/// ```no_run
/// # use memcache::exp::AsyncMetaClient;
/// # async fn example() -> Result<(), memcache::MemcacheError> {
/// let mut client = AsyncMetaClient::connect("127.0.0.1:11211").await?;
/// client.set("foo", "bar").ttl(60).send().await?;
/// let result = client.get("foo").send().await?;
/// # Ok(())
/// # }
/// ```
pub struct AsyncMetaClient {
    connection: AsyncMetaConnection,
}

impl AsyncMetaClient {
    pub async fn connect<A: ToSocketAddrs>(addr: A) -> Result<AsyncMetaClient, MemcacheError> {
        Ok(AsyncMetaClient::from_connection(
            AsyncMetaConnection::connect(addr).await?,
        ))
    }

    pub fn from_connection(connection: AsyncMetaConnection) -> AsyncMetaClient {
        AsyncMetaClient { connection }
    }

    /// Read a key.
    pub fn get(&mut self, key: impl Into<Vec<u8>>) -> Request<'_, AsyncMetaClient, Get> {
        Request::new(self, Get::new(key))
    }

    /// Store a value under a key.
    pub fn set(&mut self, key: impl Into<Vec<u8>>, value: impl Into<Vec<u8>>) -> Request<'_, AsyncMetaClient, Set> {
        Request::new(self, Set::new(key, value))
    }

    /// Delete a key.
    pub fn delete(&mut self, key: impl Into<Vec<u8>>) -> Request<'_, AsyncMetaClient, Delete> {
        Request::new(self, Delete::new(key))
    }

    /// Increment a counter (delta defaults to 1).
    pub fn increment(&mut self, key: impl Into<Vec<u8>>) -> Request<'_, AsyncMetaClient, Arithmetic> {
        Request::new(self, Arithmetic::new(key))
    }

    /// Decrement a counter (delta defaults to 1); saturates at zero.
    pub fn decrement(&mut self, key: impl Into<Vec<u8>>) -> Request<'_, AsyncMetaClient, Arithmetic> {
        let operation = Arithmetic {
            mode: ArithmeticMode::Decrement,
            ..Arithmetic::new(key)
        };
        Request::new(self, operation)
    }

    /// Run a standalone operation value; [`send`](Request::send) is sugar
    /// for this.
    pub async fn run<O: Operation>(&mut self, operation: O) -> Result<O::Output, MemcacheError> {
        let command = operation.prepare()?;
        let wire = parse_meta_result(self.connection.execute(&command).await?)?;
        operation.parse(wire)
    }

    /// Round-trip an `mn` no-op; useful as a connection health check.
    pub async fn noop(&mut self) -> Result<(), MemcacheError> {
        let response = self.connection.execute(&build_noop()).await?;
        if response.rc != ReturnCode::Mn {
            return Err(ServerError::BadResponse("unexpected no-op response".into()).into());
        }
        Ok(())
    }

    /// Fetch `me` debug fields for a key; `None` on a miss.
    pub async fn debug(&mut self, key: impl Into<Vec<u8>>) -> Result<Option<HashMap<String, String>>, MemcacheError> {
        let response = self.connection.execute(&build_debug(key)?).await?;
        parse_debug_result(&response)
    }
}

impl<'a, O: Operation> Request<'a, AsyncMetaClient, O> {
    /// Execute the request and return its typed result.
    pub async fn send(self) -> Result<O::Output, MemcacheError> {
        let Request { client, operation } = self;
        client.run(operation).await
    }
}
