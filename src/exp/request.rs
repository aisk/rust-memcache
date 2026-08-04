//! Lazy request builders returned by the client verbs.
//!
//! `client.get("foo")` and friends return a [`Request`] holding the client
//! and an operation under construction. Option methods forward to the
//! operation's builders; nothing touches the network until
//! [`send`](Request::send). `Request` is generic over the client, so the
//! option surface is written once and shared by the blocking and tokio
//! clients - only `send` is implemented per client.

use super::meta_api::SetMode;
use super::operation::{Arithmetic, Delete, Get, Meta, Set};

/// An operation bound to a client, awaiting [`send`](Request::send).
#[must_use = "a request does nothing until you call .send()"]
pub struct Request<'a, C, O> {
    pub(crate) client: &'a C,
    pub(crate) operation: O,
}

impl<'a, C, O> Request<'a, C, O> {
    pub(crate) fn new(client: &'a C, operation: O) -> Request<'a, C, O> {
        Request { client, operation }
    }

    /// Detach the operation as a standalone value, dropping the client
    /// binding. Useful to build operations for a future batch API.
    pub fn into_operation(self) -> O {
        self.operation
    }
}

impl<'a, C> Request<'a, C, Get> {
    /// Fetch this metadata alongside the value.
    pub fn meta(mut self, meta: Meta) -> Self {
        self.operation = self.operation.meta(meta);
        self
    }

    /// Update the item TTL while reading.
    pub fn touch(mut self, ttl: u32) -> Self {
        self.operation = self.operation.touch(ttl);
        self
    }

    /// Don't bump the item in the LRU.
    pub fn no_lru_bump(mut self) -> Self {
        self.operation = self.operation.no_lru_bump();
        self
    }

    /// Suppress the value when the item CAS still matches.
    pub fn unless_cas(mut self, cas: u64) -> Self {
        self.operation = self.operation.unless_cas(cas);
        self
    }

    /// Fetch metadata only, without the value.
    pub fn without_value(mut self) -> Self {
        self.operation = self.operation.without_value();
        self
    }

    /// Vivify a missing key with this TTL and request a lease.
    pub fn lease_ttl(mut self, ttl: u32) -> Self {
        self.operation = self.operation.lease_ttl(ttl);
        self
    }

    /// Also win the lease when the remaining TTL drops below this.
    pub fn refresh_before(mut self, ttl: u32) -> Self {
        self.operation = self.operation.refresh_before(ttl);
        self
    }
}

impl<'a, C> Request<'a, C, Set> {
    pub fn ttl(mut self, ttl: u32) -> Self {
        self.operation = self.operation.ttl(ttl);
        self
    }

    /// Override the client flags stored with the item (normally chosen by
    /// [`ToValue`](super::ToValue)) - for interop with other clients' flag
    /// conventions.
    pub fn client_flags(mut self, flags: u32) -> Self {
        self.operation = self.operation.client_flags(flags);
        self
    }

    pub fn mode(mut self, mode: SetMode) -> Self {
        self.operation = self.operation.mode(mode);
        self
    }

    /// Store only when the item does not exist.
    pub fn add(mut self) -> Self {
        self.operation = self.operation.add();
        self
    }

    /// Store only when the item exists.
    pub fn replace(mut self) -> Self {
        self.operation = self.operation.replace();
        self
    }

    /// Append raw bytes to the stored value.
    pub fn append(mut self) -> Self {
        self.operation = self.operation.append();
        self
    }

    /// Prepend raw bytes to the stored value.
    pub fn prepend(mut self) -> Self {
        self.operation = self.operation.prepend();
        self
    }

    /// Store only when the item CAS matches.
    pub fn compare_cas(mut self, cas: u64) -> Self {
        self.operation = self.operation.compare_cas(cas);
        self
    }

    /// Overwrite the item CAS with this value (protocol `E` flag) - for
    /// replicating or restoring items, not for normal CAS loops.
    pub fn force_cas(mut self, cas: u64) -> Self {
        self.operation = self.operation.force_cas(cas);
        self
    }

    /// Return the new item CAS in the result.
    pub fn return_cas(mut self) -> Self {
        self.operation = self.operation.return_cas();
        self
    }

    /// For append/prepend, vivify a missing item with this TTL.
    pub fn vivify_ttl(mut self, ttl: u32) -> Self {
        self.operation = self.operation.vivify_ttl(ttl);
        self
    }
}

impl<'a, C> Request<'a, C, Delete> {
    /// Delete only when the item CAS matches.
    pub fn compare_cas(mut self, cas: u64) -> Self {
        self.operation = self.operation.compare_cas(cas);
        self
    }

    /// Mark the item stale instead of removing it.
    pub fn invalidate(mut self) -> Self {
        self.operation = self.operation.invalidate();
        self
    }

    /// For invalidate, how long the stale item stays readable.
    pub fn stale_for(mut self, ttl: u32) -> Self {
        self.operation = self.operation.stale_for(ttl);
        self
    }
}

impl<'a, C> Request<'a, C, Arithmetic> {
    pub fn delta(mut self, delta: u64) -> Self {
        self.operation = self.operation.delta(delta);
        self
    }

    /// Vivify a missing item with this value and TTL.
    pub fn initial(mut self, value: u64, ttl: u32) -> Self {
        self.operation = self.operation.initial(value, ttl);
        self
    }

    /// Update the item TTL while applying the delta.
    pub fn ttl(mut self, ttl: u32) -> Self {
        self.operation = self.operation.ttl(ttl);
        self
    }

    /// Apply only when the item CAS matches.
    pub fn compare_cas(mut self, cas: u64) -> Self {
        self.operation = self.operation.compare_cas(cas);
        self
    }

    /// Overwrite the item CAS with this value (protocol `E` flag) - for
    /// replicating or restoring items, not for normal CAS loops.
    pub fn force_cas(mut self, cas: u64) -> Self {
        self.operation = self.operation.force_cas(cas);
        self
    }

    /// Return the new item CAS in the result.
    pub fn return_cas(mut self) -> Self {
        self.operation = self.operation.return_cas();
        self
    }

    /// Return the remaining TTL in the result.
    pub fn return_ttl(mut self) -> Self {
        self.operation = self.operation.return_ttl();
        self
    }
}
