//! Fixed-size byte-string identifiers used across the envelope wire format.
//!
//! Each type serializes as a CBOR byte string (major type 2), not as an
//! array of integers, so the on-wire encoding stays compact and canonical:
//! field order is fixed by struct declaration order and there is no map to
//! reorder.

use std::fmt;

use rand::RngCore;
use serde::de::{self, Visitor};
use serde::{Deserialize, Deserializer, Serialize, Serializer};

macro_rules! fixed_bytes_id {
    ($name:ident, $len:expr) => {
        #[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
        pub struct $name(pub [u8; $len]);

        impl $name {
            pub const LEN: usize = $len;

            pub fn from_bytes(bytes: [u8; $len]) -> Self {
                Self(bytes)
            }

            pub fn as_bytes(&self) -> &[u8; $len] {
                &self.0
            }

            pub fn random(rng: &mut impl RngCore) -> Self {
                let mut bytes = [0u8; $len];
                rng.fill_bytes(&mut bytes);
                Self(bytes)
            }
        }

        impl fmt::Debug for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                write!(f, "{}({})", stringify!($name), hex(&self.0))
            }
        }

        impl Serialize for $name {
            fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
            where
                S: Serializer,
            {
                serializer.serialize_bytes(&self.0)
            }
        }

        impl<'de> Deserialize<'de> for $name {
            fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
            where
                D: Deserializer<'de>,
            {
                struct FixedBytesVisitor;

                impl<'de> Visitor<'de> for FixedBytesVisitor {
                    type Value = [u8; $len];

                    fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                        write!(f, "a {}-byte string", $len)
                    }

                    fn visit_bytes<E>(self, v: &[u8]) -> Result<Self::Value, E>
                    where
                        E: de::Error,
                    {
                        v.try_into().map_err(|_| E::invalid_length(v.len(), &self))
                    }

                    fn visit_byte_buf<E>(self, v: Vec<u8>) -> Result<Self::Value, E>
                    where
                        E: de::Error,
                    {
                        self.visit_bytes(&v)
                    }
                }

                Ok($name(deserializer.deserialize_bytes(FixedBytesVisitor)?))
            }
        }
    };
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fixed_bytes_id!(EventId, 16);
fixed_bytes_id!(DeviceId, 16);
fixed_bytes_id!(OperationId, 16);
fixed_bytes_id!(RequestId, 16);
fixed_bytes_id!(Signature, 64);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_through_cbor_as_a_byte_string() {
        let id = OperationId([9u8; 16]);
        let mut buf = Vec::new();
        ciborium::into_writer(&id, &mut buf).unwrap();
        // CBOR byte string major type (0x40..0x5b) with length 16 -> 0x50.
        assert_eq!(buf[0], 0x50);
        let back: OperationId = ciborium::from_reader(&buf[..]).unwrap();
        assert_eq!(back, id);
    }

    #[test]
    fn rejects_the_wrong_length() {
        let mut buf = Vec::new();
        ciborium::into_writer(&serde_bytes::ByteBuf::from(vec![1u8; 8]), &mut buf).unwrap();
        let result: Result<OperationId, _> = ciborium::from_reader(&buf[..]);
        assert!(result.is_err());
    }
}
