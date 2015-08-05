#![crate_name = "memcache"]
#![crate_type = "rlib"]

pub mod ffi;
mod error;
mod client;
mod connectable;

pub use client::Client;
pub use error::MemcacheError;
pub use error::MemcacheResult;

#[inline]
pub fn connect(p: &connectable::Connectable) -> MemcacheResult<Client> {
    return Client::connect(p);
}
