//! Tokio single-server client over the semantic layer.

use std::collections::HashMap;

use tokio::net::ToSocketAddrs;

use crate::error::{MemcacheError, ServerError};

use super::async_connection::AsyncMetaConnection;
use super::core;
use super::meta_api::{MetaCommandResult, build_debug, build_noop, parse_debug_result, parse_meta_result};
use super::meta_command::{MetaCommand, ReturnCode};
use super::operation::{Arithmetic, Delete, Get, Set};
use super::result::{ArithmeticResult, GetResult, MutationResult};

/// The async counterpart of [`MetaClient`](super::MetaClient); same
/// semantics over a tokio connection.
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

    async fn run(&mut self, command: &MetaCommand) -> Result<MetaCommandResult, MemcacheError> {
        parse_meta_result(self.connection.execute(command).await?)
    }

    pub async fn get(&mut self, operation: &Get) -> Result<GetResult, MemcacheError> {
        let command = core::prepare_get(operation)?;
        let wire = self.run(&command).await?;
        core::parse_get(operation, wire)
    }

    pub async fn set(&mut self, operation: &Set) -> Result<MutationResult, MemcacheError> {
        let command = core::prepare_set(operation)?;
        let wire = self.run(&command).await?;
        core::parse_set(operation, wire)
    }

    pub async fn delete(&mut self, operation: &Delete) -> Result<MutationResult, MemcacheError> {
        let command = core::prepare_delete(operation)?;
        let wire = self.run(&command).await?;
        core::parse_delete(operation, wire)
    }

    pub async fn arithmetic(&mut self, operation: &Arithmetic) -> Result<ArithmeticResult, MemcacheError> {
        let command = core::prepare_arithmetic(operation)?;
        let wire = self.run(&command).await?;
        core::parse_arithmetic(operation, wire)
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
