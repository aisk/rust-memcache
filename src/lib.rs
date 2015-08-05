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
pub fn connect(host: &str, port: u16) -> MemcacheResult<Client> {
    return Client::connect(host, port);
}
