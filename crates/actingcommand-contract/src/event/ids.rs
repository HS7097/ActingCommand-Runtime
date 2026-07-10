// SPDX-License-Identifier: AGPL-3.0-only

use super::SanitizationError;
use serde::de::{self, Visitor};
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::fmt;

macro_rules! typed_id {
    ($name:ident, $prefix:literal) => {
        #[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
        pub struct $name([u8; 16]);

        impl $name {
            pub const fn new(bytes: [u8; 16]) -> Self {
                Self(bytes)
            }

            pub const fn as_bytes(&self) -> &[u8; 16] {
                &self.0
            }

            fn parse(value: &str) -> Result<Self, ()> {
                let Some(hex) = value.strip_prefix($prefix) else {
                    return Err(());
                };
                if hex.len() != 32
                    || !hex
                        .bytes()
                        .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
                {
                    return Err(());
                }
                let mut bytes = [0_u8; 16];
                for (index, slot) in bytes.iter_mut().enumerate() {
                    let offset = index * 2;
                    *slot = decode_hex_pair(&hex.as_bytes()[offset..offset + 2]).ok_or(())?;
                }
                Ok(Self(bytes))
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str($prefix)?;
                for byte in self.0 {
                    write!(formatter, "{byte:02x}")?;
                }
                Ok(())
            }
        }

        impl fmt::Debug for $name {
            fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                fmt::Display::fmt(self, formatter)
            }
        }

        impl Serialize for $name {
            fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
            where
                S: Serializer,
            {
                serializer.collect_str(self)
            }
        }

        impl<'de> Deserialize<'de> for $name {
            fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
            where
                D: Deserializer<'de>,
            {
                struct IdVisitor;

                impl Visitor<'_> for IdVisitor {
                    type Value = $name;

                    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                        formatter.write_str("a canonical typed identifier")
                    }

                    fn visit_str<E>(self, value: &str) -> Result<Self::Value, E>
                    where
                        E: de::Error,
                    {
                        $name::parse(value).map_err(|()| E::custom("invalid typed identifier"))
                    }
                }

                deserializer.deserialize_str(IdVisitor)
            }
        }
    };
}

typed_id!(EventId, "evt_");
typed_id!(InstanceId, "instance_");
typed_id!(RequestId, "request_");
typed_id!(CorrelationId, "correlation_");
typed_id!(CausationId, "causation_");
typed_id!(TaskId, "task_");
typed_id!(RunId, "run_");
typed_id!(LeaseId, "lease_");
typed_id!(FrameId, "frame_");
typed_id!(ActionId, "action_");
typed_id!(RecognitionId, "recognition_");
typed_id!(ArtifactId, "artifact_");

#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(transparent)]
pub struct StaticCode(String);

impl StaticCode {
    pub fn new(value: &'static str) -> Result<Self, SanitizationError> {
        Self::parse(value).map_err(|()| SanitizationError::new("invalid_static_code", "code"))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    fn parse(value: &str) -> Result<Self, ()> {
        if is_static_code(value) {
            Ok(Self(value.to_string()))
        } else {
            Err(())
        }
    }
}

impl fmt::Display for StaticCode {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl fmt::Debug for StaticCode {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.debug_tuple("StaticCode").field(&self.0).finish()
    }
}

impl<'de> Deserialize<'de> for StaticCode {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        Self::parse(&value).map_err(|()| de::Error::custom("invalid static code"))
    }
}

fn decode_hex_pair(pair: &[u8]) -> Option<u8> {
    let high = decode_hex(pair[0])?;
    let low = decode_hex(pair[1])?;
    Some((high << 4) | low)
}

fn decode_hex(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        _ => None,
    }
}

fn is_static_code(value: &str) -> bool {
    let bytes = value.as_bytes();
    !value.is_empty()
        && value.len() <= 128
        && bytes
            .first()
            .is_some_and(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit())
        && bytes
            .last()
            .is_some_and(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit())
        && value.bytes().all(|byte| {
            byte.is_ascii_lowercase() || byte.is_ascii_digit() || matches!(byte, b'-' | b'_' | b'.')
        })
}
