#![crate_name = "memcache"]
#![crate_type = "rlib"]

pub mod ffi;
mod error;
mod client;

pub use client::connect;
pub use error::MemcacheError;
pub use error::MemcacheResult;
