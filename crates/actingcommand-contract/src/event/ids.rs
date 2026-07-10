// SPDX-License-Identifier: AGPL-3.0-only

use serde::de::{self, Visitor};
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::error::Error;
use std::fmt;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

static NEXT_ISSUER_NONCE: AtomicU64 = AtomicU64::new(1);

macro_rules! typed_id {
    ($name:ident, $issued:ident, $prefix:literal, $mint:ident) => {
        #[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
        pub struct $name([u8; 16]);

        impl $name {
            fn from_issued_bytes(bytes: [u8; 16]) -> Self {
                Self(bytes)
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

            pub(super) fn canonical(&self) -> String {
                let mut value = String::with_capacity($prefix.len() + 32);
                value.push_str($prefix);
                for byte in self.0 {
                    use std::fmt::Write as _;
                    write!(value, "{byte:02x}").expect("writing to a String cannot fail");
                }
                value
            }
        }

        impl fmt::Debug for $name {
            fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str(concat!(stringify!($name), "(<opaque>)"))
            }
        }

        impl Serialize for $name {
            fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
            where
                S: Serializer,
            {
                serializer.serialize_str(&self.canonical())
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

        #[derive(Clone, Copy, PartialEq, Eq)]
        pub struct $issued($name);

        impl $issued {
            pub const fn transport(&self) -> &$name {
                &self.0
            }

            pub(super) const fn into_transport(self) -> $name {
                self.0
            }
        }

        impl fmt::Debug for $issued {
            fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str(concat!(stringify!($issued), "(<opaque>)"))
            }
        }

        impl IdentifierIssuer {
            pub fn $mint(&self) -> Result<$issued, IdentifierIssuanceError> {
                self.next_bytes().map($name::from_issued_bytes).map($issued)
            }
        }
    };
}

typed_id!(EventId, IssuedEventId, "evt_", mint_event_id);
typed_id!(InstanceId, IssuedInstanceId, "instance_", mint_instance_id);
typed_id!(RequestId, IssuedRequestId, "request_", mint_request_id);
typed_id!(
    CorrelationId,
    IssuedCorrelationId,
    "correlation_",
    mint_correlation_id
);
typed_id!(
    CausationId,
    IssuedCausationId,
    "causation_",
    mint_causation_id
);
typed_id!(TaskId, IssuedTaskId, "task_", mint_task_id);
typed_id!(RunId, IssuedRunId, "run_", mint_run_id);
typed_id!(LeaseId, IssuedLeaseId, "lease_", mint_lease_id);
typed_id!(FrameId, IssuedFrameId, "frame_", mint_frame_id);
typed_id!(ActionId, IssuedActionId, "action_", mint_action_id);
typed_id!(
    RecognitionId,
    IssuedRecognitionId,
    "recognition_",
    mint_recognition_id
);
typed_id!(ArtifactId, IssuedArtifactId, "artifact_", mint_artifact_id);

/// Mints producer capabilities without accepting caller-selected identifier bytes or strings.
pub struct IdentifierIssuer {
    namespace: u64,
    next_sequence: AtomicU64,
}

impl IdentifierIssuer {
    pub fn new() -> Result<Self, IdentifierIssuanceError> {
        let elapsed = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|_| IdentifierIssuanceError::new("identifier_clock_invalid"))?;
        let nonce = NEXT_ISSUER_NONCE
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |value| {
                value.checked_add(1)
            })
            .map_err(|_| IdentifierIssuanceError::new("identifier_issuer_exhausted"))?;
        let nanos = elapsed.as_nanos();
        let folded_time = (nanos as u64) ^ ((nanos >> 64) as u64).rotate_left(17);
        let namespace =
            folded_time ^ u64::from(std::process::id()).rotate_left(29) ^ nonce.rotate_left(43);
        Ok(Self {
            namespace,
            next_sequence: AtomicU64::new(1),
        })
    }

    fn next_bytes(&self) -> Result<[u8; 16], IdentifierIssuanceError> {
        let sequence = self
            .next_sequence
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |value| {
                value.checked_add(1)
            })
            .map_err(|_| IdentifierIssuanceError::new("identifier_sequence_exhausted"))?;
        let mut bytes = [0_u8; 16];
        bytes[..8].copy_from_slice(&self.namespace.to_be_bytes());
        bytes[8..].copy_from_slice(&sequence.to_be_bytes());
        Ok(bytes)
    }
}

impl fmt::Debug for IdentifierIssuer {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("IdentifierIssuer(<opaque>)")
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct IdentifierIssuanceError {
    code: &'static str,
}

impl IdentifierIssuanceError {
    const fn new(code: &'static str) -> Self {
        Self { code }
    }

    pub const fn code(self) -> &'static str {
        self.code
    }
}

impl fmt::Display for IdentifierIssuanceError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "identifier issuance failed with {}", self.code)
    }
}

impl Error for IdentifierIssuanceError {}

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
