//! Value encoding for the semantic layer.
//!
//! [`ToValue`] turns a value into raw bytes plus the client flags stored
//! with the item; [`FromValue`] decodes them back, driven by the type the
//! caller requests. The built-in implementations store strings and bytes
//! verbatim and numbers as their decimal ASCII form (which keeps them
//! usable by `ma` arithmetic), tagged with the conventional flag for the
//! type: [`FLAG_INT`] for integers, [`FLAG_STR`] for strings and
//! [`FLAG_BYTES`] (zero) for everything else.
//!
//! Decoding is driven purely by the requested type; the built-in
//! implementations ignore the stored flags, so anything a client wrote —
//! flagged or not — decodes as long as the bytes parse. Flags will start
//! participating in decoding once flag-bearing encodings (compression,
//! JSON, ...) are supported; custom implementations can already use them.

use std::str::FromStr;

use crate::error::MemcacheError;

/// Untyped raw bytes (the protocol default of zero).
pub const FLAG_BYTES: u32 = 0;
/// An integer stored as decimal ASCII.
pub const FLAG_INT: u32 = 1 << 1;
/// A UTF-8 string.
pub const FLAG_STR: u32 = 1 << 4;

/// Encode a value into `(bytes, client_flags)` for storage.
pub trait ToValue {
    fn to_value(&self) -> (Vec<u8>, u32);
}

/// Decode a stored `(bytes, client_flags)` back into a value.
pub trait FromValue: Sized {
    fn from_value(value: Vec<u8>, flags: u32) -> Result<Self, MemcacheError>;
}

impl ToValue for Vec<u8> {
    fn to_value(&self) -> (Vec<u8>, u32) {
        (self.clone(), FLAG_BYTES)
    }
}

impl ToValue for &[u8] {
    fn to_value(&self) -> (Vec<u8>, u32) {
        (self.to_vec(), FLAG_BYTES)
    }
}

impl<const N: usize> ToValue for &[u8; N] {
    fn to_value(&self) -> (Vec<u8>, u32) {
        (self.to_vec(), FLAG_BYTES)
    }
}

impl ToValue for String {
    fn to_value(&self) -> (Vec<u8>, u32) {
        (self.as_bytes().to_vec(), FLAG_STR)
    }
}

impl ToValue for &String {
    fn to_value(&self) -> (Vec<u8>, u32) {
        (self.as_bytes().to_vec(), FLAG_STR)
    }
}

impl ToValue for &str {
    fn to_value(&self) -> (Vec<u8>, u32) {
        (self.as_bytes().to_vec(), FLAG_STR)
    }
}

impl FromValue for Vec<u8> {
    fn from_value(value: Vec<u8>, _flags: u32) -> Result<Self, MemcacheError> {
        Ok(value)
    }
}

impl FromValue for String {
    fn from_value(value: Vec<u8>, _flags: u32) -> Result<Self, MemcacheError> {
        Ok(String::from_utf8(value)?)
    }
}

macro_rules! impl_value_for_integer {
    ($ty:ident) => {
        impl ToValue for $ty {
            fn to_value(&self) -> (Vec<u8>, u32) {
                (self.to_string().into_bytes(), FLAG_INT)
            }
        }

        impl FromValue for $ty {
            fn from_value(value: Vec<u8>, _flags: u32) -> Result<Self, MemcacheError> {
                Ok(Self::from_str(std::str::from_utf8(&value)?)?)
            }
        }
    };
}

impl_value_for_integer!(u8);
impl_value_for_integer!(u16);
impl_value_for_integer!(u32);
impl_value_for_integer!(u64);
impl_value_for_integer!(i8);
impl_value_for_integer!(i16);
impl_value_for_integer!(i32);
impl_value_for_integer!(i64);

// bool and floats have no conventional flag; they are stored as untyped
// ASCII text.
macro_rules! impl_value_for_text_scalar {
    ($ty:ident) => {
        impl ToValue for $ty {
            fn to_value(&self) -> (Vec<u8>, u32) {
                (self.to_string().into_bytes(), FLAG_BYTES)
            }
        }

        impl FromValue for $ty {
            fn from_value(value: Vec<u8>, _flags: u32) -> Result<Self, MemcacheError> {
                Ok(Self::from_str(std::str::from_utf8(&value)?)?)
            }
        }
    };
}

impl_value_for_text_scalar!(bool);
impl_value_for_text_scalar!(f32);
impl_value_for_text_scalar!(f64);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encode_builtin_types() {
        assert_eq!("bar".to_value(), (b"bar".to_vec(), FLAG_STR));
        assert_eq!(String::from("bar").to_value(), (b"bar".to_vec(), FLAG_STR));
        assert_eq!(b"bar".to_value(), (b"bar".to_vec(), FLAG_BYTES));
        assert_eq!(42u64.to_value(), (b"42".to_vec(), FLAG_INT));
        assert_eq!((-1i32).to_value(), (b"-1".to_vec(), FLAG_INT));
        assert_eq!(true.to_value(), (b"true".to_vec(), FLAG_BYTES));
        assert_eq!(1.5f64.to_value(), (b"1.5".to_vec(), FLAG_BYTES));
    }

    #[test]
    fn decode_is_type_driven() {
        assert_eq!(String::from_value(b"bar".to_vec(), FLAG_STR).unwrap(), "bar");
        assert_eq!(u64::from_value(b"42".to_vec(), FLAG_INT).unwrap(), 42);
        assert_eq!(f64::from_value(b"1.5".to_vec(), FLAG_BYTES).unwrap(), 1.5);
        assert!(bool::from_value(b"true".to_vec(), FLAG_BYTES).unwrap());
        assert!(u64::from_value(b"x".to_vec(), FLAG_INT).is_err());

        // Flags are ignored: any value decodes as long as the bytes parse.
        assert_eq!(String::from_value(b"42".to_vec(), FLAG_INT).unwrap(), "42");
        assert_eq!(u64::from_value(b"42".to_vec(), FLAG_STR).unwrap(), 42);
        assert_eq!(Vec::<u8>::from_value(b"x".to_vec(), 1).unwrap(), b"x".to_vec());
    }
}
