// SPDX-License-Identifier: AGPL-3.0-only

//! Workflow #477 CV-1 (register §1.507; `contracts/pack-catalog.md`, "Version 2"): the one
//! definition of a V2 scheduling task's resource effect, an entry of its `consumes` or `produces`
//! (`contracts/scheduling/v2/common.schema.json`, `resourceEffect`), and of the bounds of its own
//! members. The policy crate's catalog compiler reads a rule's effects with these types (its
//! `schema` module re-exports them; they lived there until 0.12.78) and checks them in a compiled
//! catalog with the bounds below; a game root's `pack-catalog.json` of version 2 carries a
//! package's effects in the same form, read with the same types and held to the same bounds
//! ([`ResourceEffectSpec::form_fault`]), so a catalog's entry and a rule's effect never differ in
//! what they can say.

use crate::canonical::CANONICAL_SAFE_INTEGER_MAX;
use crate::fact::is_list_reading_key;
use serde::{Deserialize, Deserializer, Serialize};

/// SS-2b (register §1.225): the explicit marker a resource side writes where a value is not
/// known yet (the owner's ruling 116): a pool's `valuation` and an effect's
/// `amount` may each be this string instead of their value (`actingcommand.scheduling.v2`
/// only). It changes nothing but the net benefit of a judged candidate, which it makes unknown.
pub const PENDING_MARKER: &str = "pending";

/// #386 CF-4: the `type` of a `fact` predicate's `value` that names another fact; SCHED-C1f1: the
/// `type` of a consumption read from a fact too.
pub const FACT_REFERENCE_TYPE: &str = "fact";

/// The most entries of one task's `consumes`, and of its `produces` (CV-1: and of one business
/// part's side in `pack-catalog.json` of version 2).
pub const MAX_EFFECTS_PER_TASK: usize = 128;

/// The longest scheduling identifier, in bytes (a task, pool or fact key id; CV-1: a rule id of a
/// per-rule business part in `pack-catalog.json`).
pub const MAX_SCHEDULING_IDENTIFIER_BYTES: usize = 128;

/// The highest `confidence_milli` of an effect.
pub const MAX_EFFECT_CONFIDENCE_MILLI: u16 = 1000;

/// The largest whole quantity an effect writes (an `amount`, an `expected_amount_milli`, a
/// consumption's `fallback`): the largest integer every JSON reader keeps exactly.
pub const MAX_EFFECT_QUANTITY: u64 = CANONICAL_SAFE_INTEGER_MAX.unsigned_abs();

/// SCHED-C1f1: the members of [`ConsumptionFact`]'s wire form, in the order the format writes
/// them.
pub const CONSUMPTION_FACT_MEMBERS: [&str; 3] = ["type", "fact_key", "fallback"];

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ResourceEffectSpec {
    pub pool_id: String,
    pub direction: EffectDirection,
    /// A positive integer in real inventory units, or (SS-2b, V2 only) the marker
    /// [`PENDING_MARKER`]: the quantity is not known yet; or (SCHED-C1f1, register §1.303, a V2
    /// `consumes` entry only) one run's amount read from a fact ([`ConsumptionFact`]). Read it
    /// through [`ResourceEffectSpec::units`], [`ResourceEffectSpec::is_pending`] and
    /// [`ResourceEffectSpec::fact_amount`].
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "effect_amount"
    )]
    pub amount: Option<EffectAmount>,
    /// Expected quantity per completed run, in thousandths of a real inventory unit: a V2
    /// production's or (SS-2g, register §1.232) a V2 consumption's.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "effect_quantity"
    )]
    pub expected_amount_milli: Option<u64>,
    pub observation_source: ObservationSource,
    pub confidence_milli: u16,
}

fn effect_quantity<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Option<u64>, D::Error> {
    u64::deserialize(deserializer).map(Some)
}

/// An effect's written `amount` (SS-2b, register §1.225; SCHED-C1f1, register §1.303).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EffectAmount {
    /// A non-negative integer as written; the compiler requires it to be positive.
    Units(u64),
    /// The marker [`PENDING_MARKER`]: the quantity is not known yet.
    Pending,
    /// SCHED-C1f1 (register §1.303 SC1F-1; `contracts/scheduling/README.md`, "A run's
    /// consumption read from a fact"): `{"type": "fact", "fact_key": B, "fallback": F}`, written
    /// with exactly these members, `fact_key` a string and `fallback` an unsigned integer. The
    /// compiler checks the rest (a V2 `consumes` entry only, B an identifier, F in
    /// `1..=2^53−1` and at most a constant capacity).
    Fact(ConsumptionFact),
    /// SCHED-C1f1: an object `amount` that is not [`EffectAmount::Fact`] as the format writes it
    /// (another `type`, a member missing, of another kind, or one the form does not have), kept as
    /// written only so that the compiler refuses it at its member; a compiled catalog holds none.
    Malformed(serde_json::Map<String, serde_json::Value>),
}

/// SCHED-C1f1 (register §1.303 SC1F-1): one run's amount of a pool read from the instance fact
/// `fact_key` (B) in the scope of the instance an evaluation decides for, and `fallback` (F), the
/// amount while B gives no usable reading. It names no scope and no offset: the fact holds the
/// amount itself, which the evaluation resolves once per instance (the evaluator's
/// `consumption` module).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConsumptionFact {
    pub fact_key: String,
    pub fallback: u64,
}

/// The wire form of a [`ConsumptionFact`].
#[derive(Serialize)]
struct ConsumptionFactWire<'a> {
    #[serde(rename = "type")]
    kind: &'static str,
    fact_key: &'a str,
    fallback: u64,
}

impl ConsumptionFact {
    /// The amount an object `amount` writes: [`EffectAmount::Fact`] when it is the form exactly
    /// (the three members, `type` the fact reference's, a string key and an unsigned fallback),
    /// else [`EffectAmount::Malformed`] as written.
    fn read(members: serde_json::Map<String, serde_json::Value>) -> EffectAmount {
        let [kind, fact_key, fallback] = CONSUMPTION_FACT_MEMBERS;
        let form = members.len() == CONSUMPTION_FACT_MEMBERS.len()
            && members.get(kind).and_then(serde_json::Value::as_str) == Some(FACT_REFERENCE_TYPE);
        match (
            form,
            members.get(fact_key).and_then(serde_json::Value::as_str),
            members.get(fallback).and_then(serde_json::Value::as_u64),
        ) {
            (true, Some(key), Some(amount)) => EffectAmount::Fact(Self {
                fact_key: key.to_owned(),
                fallback: amount,
            }),
            _ => EffectAmount::Malformed(members),
        }
    }
}

impl Serialize for EffectAmount {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match self {
            Self::Units(amount) => serializer.serialize_u64(*amount),
            Self::Pending => serializer.serialize_str(PENDING_MARKER),
            Self::Fact(form) => ConsumptionFactWire {
                kind: FACT_REFERENCE_TYPE,
                fact_key: &form.fact_key,
                fallback: form.fallback,
            }
            .serialize(serializer),
            Self::Malformed(members) => members.serialize(serializer),
        }
    }
}

/// An `amount` member: an unsigned integer, the string [`PENDING_MARKER`], or (SCHED-C1f1) an
/// object, read by [`ConsumptionFact::read`] and checked by the compiler. Every other value,
/// `null` included, is refused (`type_mismatch`), as an integer-only `amount` refused it before.
fn effect_amount<'de, D: Deserializer<'de>>(
    deserializer: D,
) -> Result<Option<EffectAmount>, D::Error> {
    struct AmountVisitor;

    impl<'de> serde::de::Visitor<'de> for AmountVisitor {
        type Value = EffectAmount;

        fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            write!(
                formatter,
                "u64, the string \"{PENDING_MARKER}\" or a consumption read from a fact"
            )
        }

        fn visit_map<A: serde::de::MapAccess<'de>>(self, map: A) -> Result<EffectAmount, A::Error> {
            let members = serde_json::Map::<String, serde_json::Value>::deserialize(
                serde::de::value::MapAccessDeserializer::new(map),
            )?;
            Ok(ConsumptionFact::read(members))
        }

        fn visit_u64<E: serde::de::Error>(self, value: u64) -> Result<EffectAmount, E> {
            Ok(EffectAmount::Units(value))
        }

        fn visit_i64<E: serde::de::Error>(self, value: i64) -> Result<EffectAmount, E> {
            u64::try_from(value)
                .map(EffectAmount::Units)
                .map_err(|_| E::invalid_value(serde::de::Unexpected::Signed(value), &"u64"))
        }

        fn visit_str<E: serde::de::Error>(self, value: &str) -> Result<EffectAmount, E> {
            if value == PENDING_MARKER {
                Ok(EffectAmount::Pending)
            } else {
                Err(E::invalid_value(serde::de::Unexpected::Str(value), &self))
            }
        }
    }

    deserializer.deserialize_any(AmountVisitor).map(Some)
}

impl ResourceEffectSpec {
    /// The written integer `amount`; `None` for an `expected_amount_milli` entry (a production or,
    /// SS-2g, a consumption), for a pending quantity and (SCHED-C1f1) for an amount read from a
    /// fact, which only the evaluator's resolution reads.
    pub fn units(&self) -> Option<u64> {
        match &self.amount {
            Some(EffectAmount::Units(amount)) => Some(*amount),
            _ => None,
        }
    }

    /// SS-2b: whether the effect's `amount` is the marker [`PENDING_MARKER`].
    pub fn is_pending(&self) -> bool {
        matches!(self.amount, Some(EffectAmount::Pending))
    }

    /// SCHED-C1f1 (register §1.303): the effect's `amount` read from a fact, when it writes one.
    pub fn fact_amount(&self) -> Option<&ConsumptionFact> {
        match &self.amount {
            Some(EffectAmount::Fact(form)) => Some(form),
            _ => None,
        }
    }

    /// CV-1: the first of the effect's own members a V2 effect on the side of `direction` may
    /// not hold, read as the catalog compiler reads an effect before it judges the pools: its
    /// `direction` the side's; exactly one quantity, `amount` or `expected_amount_milli`; a whole
    /// `amount` of 1 to [`MAX_EFFECT_QUANTITY`], an `expected_amount_milli` of at most it, or the
    /// pending marker; a consumption read from a fact on a `consumes` entry only, its `fact_key`
    /// a scheduling identifier that holds no list reading and its `fallback` of 1 to
    /// [`MAX_EFFECT_QUANTITY`]; a `confidence_milli` of at most [`MAX_EFFECT_CONFIDENCE_MILLI`].
    /// What the pools decide (a capacity, a valuation) is not judged here. `None` when the effect
    /// holds every rule.
    pub fn form_fault(&self, direction: EffectDirection) -> Option<EffectFault> {
        let form = |member| {
            Some(EffectFault {
                member,
                bound: false,
            })
        };
        let bound = |member| {
            Some(EffectFault {
                member,
                bound: true,
            })
        };
        if self.direction != direction {
            return form("/direction");
        }
        if self.confidence_milli > MAX_EFFECT_CONFIDENCE_MILLI {
            return bound("/confidence_milli");
        }
        match (&self.amount, self.expected_amount_milli) {
            (Some(_), Some(_)) | (None, None) => form(""),
            (None, Some(milli)) => {
                if milli > MAX_EFFECT_QUANTITY {
                    bound("/expected_amount_milli")
                } else {
                    None
                }
            }
            (Some(EffectAmount::Units(units)), None) => {
                if (1..=MAX_EFFECT_QUANTITY).contains(units) {
                    None
                } else {
                    bound("/amount")
                }
            }
            (Some(EffectAmount::Pending), None) => None,
            (Some(EffectAmount::Fact(fact)), None) => {
                if direction != EffectDirection::Consume {
                    form("/amount")
                } else if !scheduling_identifier(&fact.fact_key)
                    || is_list_reading_key(&fact.fact_key)
                {
                    form("/amount/fact_key")
                } else if !(1..=MAX_EFFECT_QUANTITY).contains(&fact.fallback) {
                    bound("/amount/fallback")
                } else {
                    None
                }
            }
            (Some(EffectAmount::Malformed(_)), None) => form("/amount"),
        }
    }
}

/// CV-1: why an effect does not hold its own rules ([`ResourceEffectSpec::form_fault`]): the
/// member it lies in, as a JSON pointer below the effect (empty for the effect as a whole, such as
/// two quantities or none), and whether a bound is broken (`true`: a number outside its range)
/// or a form (`false`: a member another kind of effect has, or a word the effect cannot take).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EffectFault {
    pub member: &'static str,
    pub bound: bool,
}

/// Whether the byte `byte` at `index` of a scheduling identifier keeps its grammar,
/// `[a-z0-9][a-z0-9._:-]*` (`contracts/scheduling/v2/common.schema.json`, `identifier`).
pub fn scheduling_identifier_byte(index: usize, byte: u8) -> bool {
    byte.is_ascii_lowercase()
        || byte.is_ascii_digit()
        || (index > 0 && matches!(byte, b'.' | b'_' | b':' | b'-'))
}

/// Whether `value` is a scheduling identifier: 1 to [`MAX_SCHEDULING_IDENTIFIER_BYTES`] bytes of
/// its grammar ([`scheduling_identifier_byte`]).
pub fn scheduling_identifier(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= MAX_SCHEDULING_IDENTIFIER_BYTES
        && value
            .bytes()
            .enumerate()
            .all(|(index, byte)| scheduling_identifier_byte(index, byte))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EffectDirection {
    Consume,
    Produce,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ObservationSource {
    SelfReported,
    ScanVerified,
    Inferred,
}
