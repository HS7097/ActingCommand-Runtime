// SPDX-License-Identifier: AGPL-3.0-only

use super::{
    ArtifactId, CorrelationId, FrameId, IssuedCorrelationId, IssuedFrameId, IssuedRunId, RunId,
    SanitizationError, Sensitivity,
};
#[cfg(test)]
use super::{IdentifierIssuanceError, IdentifierIssuer};
use serde::de::{self, Visitor};
use serde::{Deserialize, Deserializer, Serialize};
use std::fmt;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum ArtifactKind {
    #[serde(rename = "capture.frame")]
    CaptureFrame,
}

impl ArtifactKind {
    const fn media_type(self) -> ArtifactMediaType {
        match self {
            Self::CaptureFrame => ArtifactMediaType::ImagePng,
        }
    }

    const fn retention_class(self) -> RetentionClass {
        match self {
            Self::CaptureFrame => RetentionClass::Adaptive,
        }
    }

    const fn extension(self) -> &'static str {
        match self {
            Self::CaptureFrame => "png",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum ArtifactMediaType {
    #[serde(rename = "image/png")]
    ImagePng,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ArtifactProducer {
    CaptureStore,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RetentionClass {
    DebugFull,
    Adaptive,
    Light,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ArtifactRedactionState {
    NotRequired,
    Applied,
    Pending,
}

macro_rules! non_disclosing_enum_deserialize {
    ($name:ident { $($wire:literal => $variant:ident),+ $(,)? }) => {
        impl<'de> Deserialize<'de> for $name {
            fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
            where
                D: Deserializer<'de>,
            {
                struct EnumVisitor;

                impl Visitor<'_> for EnumVisitor {
                    type Value = $name;

                    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                        formatter.write_str("a schema-owned artifact value")
                    }

                    fn visit_str<E>(self, value: &str) -> Result<Self::Value, E>
                    where
                        E: de::Error,
                    {
                        match value {
                            $($wire => Ok($name::$variant)),+,
                            _ => Err(E::custom("invalid schema-owned artifact value")),
                        }
                    }
                }

                deserializer.deserialize_str(EnumVisitor)
            }
        }
    };
}

non_disclosing_enum_deserialize!(ArtifactKind {
    "capture.frame" => CaptureFrame,
});
non_disclosing_enum_deserialize!(ArtifactMediaType {
    "image/png" => ImagePng,
});
non_disclosing_enum_deserialize!(ArtifactProducer {
    "capture_store" => CaptureStore,
});
non_disclosing_enum_deserialize!(RetentionClass {
    "debug_full" => DebugFull,
    "adaptive" => Adaptive,
    "light" => Light,
});
non_disclosing_enum_deserialize!(ArtifactRedactionState {
    "not_required" => NotRequired,
    "applied" => Applied,
    "pending" => Pending,
});

/// Store-facing correlations. Transport IDs cannot be promoted into these slots.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ArtifactLinksDraft {
    run_id: Option<IssuedRunId>,
    frame_id: Option<IssuedFrameId>,
    correlation_id: Option<IssuedCorrelationId>,
}

impl ArtifactLinksDraft {
    pub fn with_run_id(mut self, value: IssuedRunId) -> Self {
        self.run_id = Some(value);
        self
    }

    pub fn with_frame_id(mut self, value: IssuedFrameId) -> Self {
        self.frame_id = Some(value);
        self
    }

    pub fn with_correlation_id(mut self, value: IssuedCorrelationId) -> Self {
        self.correlation_id = Some(value);
        self
    }
}

/// Owner-only store boundary. It is intentionally private until C2 supplies the real durable
/// artifact-store and verification boundary.
#[cfg(test)]
struct ArtifactStoreBoundary {
    identifiers: IdentifierIssuer,
}

#[cfg(test)]
impl ArtifactStoreBoundary {
    fn new() -> Result<Self, IdentifierIssuanceError> {
        Ok(Self {
            identifiers: IdentifierIssuer::new()?,
        })
    }

    fn issue_pending(
        &self,
        kind: ArtifactKind,
        links: ArtifactLinksDraft,
        bytes: &[u8],
        created_at_unix_ms: u64,
    ) -> Result<StoreIssuedArtifact, SanitizationError> {
        if bytes.is_empty() {
            return Err(SanitizationError::new(
                "invalid_artifact_byte_count",
                "byte_count",
            ));
        }
        if created_at_unix_ms == 0 {
            return Err(SanitizationError::new(
                "invalid_artifact_timestamp",
                "created_at_unix_ms",
            ));
        }
        let byte_count = u64::try_from(bytes.len())
            .map_err(|_| SanitizationError::new("invalid_artifact_byte_count", "byte_count"))?;
        let artifact_id = self
            .identifiers
            .mint_artifact_id()
            .map_err(|_| SanitizationError::new("artifact_id_issuance_failed", "artifact_id"))?
            .into_transport();
        let sha256 = canonical_sha256(bytes);
        let object_key = object_key_for(&artifact_id, kind, &sha256);
        let reference = ArtifactReference {
            artifact_id,
            kind,
            run_id: links.run_id.map(IssuedRunId::into_transport),
            frame_id: links.frame_id.map(IssuedFrameId::into_transport),
            correlation_id: links
                .correlation_id
                .map(IssuedCorrelationId::into_transport),
            object_key,
            media_type: kind.media_type(),
            byte_count,
            sha256,
            created_at_unix_ms,
            producer: ArtifactProducer::CaptureStore,
            retention_class: kind.retention_class(),
            redaction_state: ArtifactRedactionState::Pending,
        };
        reference.validate()?;
        Ok(StoreIssuedArtifact { reference })
    }
}

#[cfg(test)]
impl fmt::Debug for ArtifactStoreBoundary {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("ArtifactStoreBoundary(<opaque>)")
    }
}

#[cfg(test)]
pub(super) fn issue_pending_for_tests(
    kind: ArtifactKind,
    links: ArtifactLinksDraft,
    bytes: &[u8],
    created_at_unix_ms: u64,
) -> Result<StoreIssuedArtifact, SanitizationError> {
    ArtifactStoreBoundary::new()
        .map_err(|_| SanitizationError::new("artifact_id_issuance_failed", "artifact_id"))?
        .issue_pending(kind, links, bytes, created_at_unix_ms)
}

/// An attachment capability returned by the artifact-store issuer. It is neither serializable nor
/// deserializable, so a transport reference cannot be promoted back into producer ingress.
#[derive(Clone, PartialEq, Eq)]
pub struct StoreIssuedArtifact {
    reference: ArtifactReference,
}

impl StoreIssuedArtifact {
    pub const fn reference(&self) -> &ArtifactReference {
        &self.reference
    }

    pub(crate) fn into_reference(self) -> ArtifactReference {
        self.reference
    }
}

impl fmt::Debug for StoreIssuedArtifact {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("StoreIssuedArtifact(<opaque>)")
    }
}

#[derive(Clone, PartialEq, Eq, Serialize)]
pub struct ArtifactReference {
    artifact_id: ArtifactId,
    kind: ArtifactKind,
    #[serde(skip_serializing_if = "Option::is_none")]
    run_id: Option<RunId>,
    #[serde(skip_serializing_if = "Option::is_none")]
    frame_id: Option<FrameId>,
    #[serde(skip_serializing_if = "Option::is_none")]
    correlation_id: Option<CorrelationId>,
    object_key: String,
    media_type: ArtifactMediaType,
    byte_count: u64,
    sha256: String,
    created_at_unix_ms: u64,
    producer: ArtifactProducer,
    retention_class: RetentionClass,
    redaction_state: ArtifactRedactionState,
}

impl ArtifactReference {
    pub const fn artifact_id(&self) -> &ArtifactId {
        &self.artifact_id
    }

    pub const fn kind(&self) -> ArtifactKind {
        self.kind
    }

    pub const fn run_id(&self) -> Option<&RunId> {
        self.run_id.as_ref()
    }

    pub const fn frame_id(&self) -> Option<&FrameId> {
        self.frame_id.as_ref()
    }

    pub const fn correlation_id(&self) -> Option<&CorrelationId> {
        self.correlation_id.as_ref()
    }

    pub fn object_key(&self) -> &str {
        &self.object_key
    }

    pub const fn media_type(&self) -> ArtifactMediaType {
        self.media_type
    }

    pub const fn byte_count(&self) -> u64 {
        self.byte_count
    }

    pub fn sha256(&self) -> &str {
        &self.sha256
    }

    pub const fn created_at_unix_ms(&self) -> u64 {
        self.created_at_unix_ms
    }

    pub const fn producer(&self) -> ArtifactProducer {
        self.producer
    }

    pub const fn retention_class(&self) -> RetentionClass {
        self.retention_class
    }

    pub const fn redaction_state(&self) -> ArtifactRedactionState {
        self.redaction_state
    }

    pub const fn sensitivity(&self) -> Sensitivity {
        match self.redaction_state {
            ArtifactRedactionState::Pending => Sensitivity::Secret,
            ArtifactRedactionState::Applied => Sensitivity::Sensitive,
            ArtifactRedactionState::NotRequired => Sensitivity::Internal,
        }
    }

    pub fn validate(&self) -> Result<(), SanitizationError> {
        let valid = self.byte_count > 0
            && self.created_at_unix_ms > 0
            && is_sha256(&self.sha256)
            && self.media_type == self.kind.media_type()
            && self.producer == ArtifactProducer::CaptureStore
            && self.retention_class == self.kind.retention_class()
            && self.redaction_state == ArtifactRedactionState::Pending
            && self.object_key == object_key_for(&self.artifact_id, self.kind, &self.sha256);
        if valid {
            Ok(())
        } else {
            Err(SanitizationError::new(
                "invalid_artifact_reference",
                "artifact",
            ))
        }
    }

    pub fn project(&self, include_object_key: bool) -> ProjectedArtifactReference {
        ProjectedArtifactReference {
            artifact_id: self.artifact_id,
            kind: self.kind,
            run_id: self.run_id,
            frame_id: self.frame_id,
            correlation_id: self.correlation_id,
            object_key: include_object_key.then(|| self.object_key.clone()),
            media_type: self.media_type,
            byte_count: self.byte_count,
            sha256: self.sha256.clone(),
            created_at_unix_ms: self.created_at_unix_ms,
            producer: self.producer,
            retention_class: self.retention_class,
            redaction_state: self.redaction_state,
        }
    }
}

impl fmt::Debug for ArtifactReference {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ArtifactReference")
            .field("artifact_id", &self.artifact_id)
            .field("kind", &self.kind)
            .field("object_key", &"<redacted-object-key>")
            .field("media_type", &self.media_type)
            .field("byte_count", &self.byte_count)
            .field("sha256", &"<redacted-digest>")
            .field("retention_class", &self.retention_class)
            .field("redaction_state", &self.redaction_state)
            .finish()
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ArtifactReferenceRecord {
    artifact_id: ArtifactId,
    kind: ArtifactKind,
    run_id: Option<RunId>,
    frame_id: Option<FrameId>,
    correlation_id: Option<CorrelationId>,
    object_key: String,
    media_type: ArtifactMediaType,
    byte_count: u64,
    sha256: String,
    created_at_unix_ms: u64,
    producer: ArtifactProducer,
    retention_class: RetentionClass,
    redaction_state: ArtifactRedactionState,
}

impl<'de> Deserialize<'de> for ArtifactReference {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let record = ArtifactReferenceRecord::deserialize(deserializer)?;
        let reference = Self {
            artifact_id: record.artifact_id,
            kind: record.kind,
            run_id: record.run_id,
            frame_id: record.frame_id,
            correlation_id: record.correlation_id,
            object_key: record.object_key,
            media_type: record.media_type,
            byte_count: record.byte_count,
            sha256: record.sha256,
            created_at_unix_ms: record.created_at_unix_ms,
            producer: record.producer,
            retention_class: record.retention_class,
            redaction_state: record.redaction_state,
        };
        reference.validate().map_err(serde::de::Error::custom)?;
        Ok(reference)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProjectedArtifactReference {
    pub artifact_id: ArtifactId,
    pub kind: ArtifactKind,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub run_id: Option<RunId>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub frame_id: Option<FrameId>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub correlation_id: Option<CorrelationId>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub object_key: Option<String>,
    pub media_type: ArtifactMediaType,
    pub byte_count: u64,
    pub sha256: String,
    pub created_at_unix_ms: u64,
    pub producer: ArtifactProducer,
    pub retention_class: RetentionClass,
    pub redaction_state: ArtifactRedactionState,
}

fn object_key_for(artifact_id: &ArtifactId, kind: ArtifactKind, sha256: &str) -> String {
    let shard = &sha256[7..9];
    format!(
        "artifacts/{shard}/{}.{}",
        artifact_id.canonical(),
        kind.extension()
    )
}

fn is_sha256(value: &str) -> bool {
    value.strip_prefix("sha256:").is_some_and(|hex| {
        hex.len() == 64
            && hex
                .bytes()
                .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
    })
}

#[cfg(test)]
fn canonical_sha256(bytes: &[u8]) -> String {
    let digest = sha256(bytes);
    let mut value = String::with_capacity(71);
    value.push_str("sha256:");
    for byte in digest {
        use std::fmt::Write as _;
        write!(value, "{byte:02x}").expect("writing to a String cannot fail");
    }
    value
}

// FIPS 180-4 SHA-256 compression. This keeps the contract dependency budget unchanged.
#[cfg(test)]
fn sha256(input: &[u8]) -> [u8; 32] {
    const INITIAL: [u32; 8] = [
        0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a, 0x510e527f, 0x9b05688c, 0x1f83d9ab,
        0x5be0cd19,
    ];
    const ROUND: [u32; 64] = [
        0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4,
        0xab1c5ed5, 0xd807aa98, 0x12835b01, 0x243185be, 0x550c7dc3, 0x72be5d74, 0x80deb1fe,
        0x9bdc06a7, 0xc19bf174, 0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc, 0x2de92c6f,
        0x4a7484aa, 0x5cb0a9dc, 0x76f988da, 0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7,
        0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967, 0x27b70a85, 0x2e1b2138, 0x4d2c6dfc,
        0x53380d13, 0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85, 0xa2bfe8a1, 0xa81a664b,
        0xc24b8b70, 0xc76c51a3, 0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070, 0x19a4c116,
        0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f, 0x682e6ff3,
        0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208, 0x90befffa, 0xa4506ceb, 0xbef9a3f7,
        0xc67178f2,
    ];

    let bit_len = (input.len() as u64).wrapping_mul(8);
    let mut padded = Vec::with_capacity(input.len() + 72);
    padded.extend_from_slice(input);
    padded.push(0x80);
    while padded.len() % 64 != 56 {
        padded.push(0);
    }
    padded.extend_from_slice(&bit_len.to_be_bytes());

    let mut state = INITIAL;
    for chunk in padded.chunks_exact(64) {
        let mut schedule = [0_u32; 64];
        for (index, word) in schedule.iter_mut().take(16).enumerate() {
            let offset = index * 4;
            *word = u32::from_be_bytes([
                chunk[offset],
                chunk[offset + 1],
                chunk[offset + 2],
                chunk[offset + 3],
            ]);
        }
        for index in 16..64 {
            let s0 = schedule[index - 15].rotate_right(7)
                ^ schedule[index - 15].rotate_right(18)
                ^ (schedule[index - 15] >> 3);
            let s1 = schedule[index - 2].rotate_right(17)
                ^ schedule[index - 2].rotate_right(19)
                ^ (schedule[index - 2] >> 10);
            schedule[index] = schedule[index - 16]
                .wrapping_add(s0)
                .wrapping_add(schedule[index - 7])
                .wrapping_add(s1);
        }

        let [mut a, mut b, mut c, mut d, mut e, mut f, mut g, mut h] = state;
        for index in 0..64 {
            let upper_e = e.rotate_right(6) ^ e.rotate_right(11) ^ e.rotate_right(25);
            let choose = (e & f) ^ ((!e) & g);
            let temporary_one = h
                .wrapping_add(upper_e)
                .wrapping_add(choose)
                .wrapping_add(ROUND[index])
                .wrapping_add(schedule[index]);
            let upper_a = a.rotate_right(2) ^ a.rotate_right(13) ^ a.rotate_right(22);
            let majority = (a & b) ^ (a & c) ^ (b & c);
            let temporary_two = upper_a.wrapping_add(majority);
            h = g;
            g = f;
            f = e;
            e = d.wrapping_add(temporary_one);
            d = c;
            c = b;
            b = a;
            a = temporary_one.wrapping_add(temporary_two);
        }
        for (slot, value) in state.iter_mut().zip([a, b, c, d, e, f, g, h]) {
            *slot = slot.wrapping_add(value);
        }
    }

    let mut digest = [0_u8; 32];
    for (index, word) in state.into_iter().enumerate() {
        digest[index * 4..index * 4 + 4].copy_from_slice(&word.to_be_bytes());
    }
    digest
}
