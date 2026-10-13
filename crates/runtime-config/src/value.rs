// SPDX-License-Identifier: AGPL-3.0-only

//! A key's value between its three forms (Workflow #387 model v1.2 §5.5, §5.9): the JSON of the
//! configuration file and of an instruction, the fact scalar of `config.parameters`, and the
//! canonical JSON text of `host.config_overrides` and of list, object and decimal values.

use crate::codes::ConfigValueProblem;
use crate::effective::ConfigFailure;
use crate::table::{KeyRange, KeyRow, ValueType};
use actingcommand_contract::FactScalar;
use serde_json::{Map, Value};
use std::collections::BTreeMap;

/// The scalar `config.parameters` reports for `value`, checked against the row's type and range;
/// `config_value_invalid` for `config_key` otherwise, `wrong_type` or `out_of_range`.
pub fn parse_value(
    row: &KeyRow,
    config_key: &str,
    value: &Value,
) -> Result<FactScalar, ConfigFailure> {
    let invalid = |problem| ConfigFailure::invalid_value(config_key, problem).with_range(row.range);
    let wrong_type = || invalid(ConfigValueProblem::WrongType);
    let in_range = |fits: bool| {
        if fits {
            Ok(())
        } else {
            Err(invalid(ConfigValueProblem::OutOfRange))
        }
    };
    match row.value_type {
        ValueType::Boolean => value
            .as_bool()
            .map(FactScalar::Boolean)
            .ok_or_else(wrong_type),
        ValueType::Integer => {
            if !value.is_i64() && !value.is_u64() {
                return Err(wrong_type());
            }
            let number = value
                .as_i64()
                .ok_or_else(|| invalid(ConfigValueProblem::OutOfRange))?;
            in_range(within(row.range, number))?;
            Ok(FactScalar::Integer(number))
        }
        ValueType::DurationMs => {
            if !value.is_i64() && !value.is_u64() {
                return Err(wrong_type());
            }
            let ms = value
                .as_u64()
                .ok_or_else(|| invalid(ConfigValueProblem::OutOfRange))?;
            in_range(i64::try_from(ms).is_ok_and(|signed| within(row.range, signed)))?;
            Ok(FactScalar::DurationMs(ms))
        }
        ValueType::String => {
            let text = value.as_str().ok_or_else(wrong_type)?;
            in_range(match row.range {
                KeyRange::Tokens(tokens) => tokens.contains(&text),
                KeyRange::Any | KeyRange::Integer { .. } => true,
            })?;
            Ok(FactScalar::String(text.to_owned()))
        }
        ValueType::Decimal if value.is_number() => Ok(FactScalar::String(canonical_json(value))),
        ValueType::List if value.is_array() => Ok(FactScalar::String(canonical_json(value))),
        ValueType::Object if value.is_object() => Ok(FactScalar::String(canonical_json(value))),
        ValueType::Decimal | ValueType::List | ValueType::Object => Err(wrong_type()),
    }
}

/// The JSON of a reported scalar of type `value_type`; `None` when the scalar is not of that
/// type.
pub fn scalar_json(value_type: ValueType, scalar: &FactScalar) -> Option<Value> {
    match (value_type, scalar) {
        (ValueType::Boolean, FactScalar::Boolean(flag)) => Some(Value::Bool(*flag)),
        (ValueType::Integer, FactScalar::Integer(number)) => Some(Value::from(*number)),
        (ValueType::DurationMs, FactScalar::DurationMs(ms)) => Some(Value::from(*ms)),
        (ValueType::String, FactScalar::String(text)) => Some(Value::String(text.clone())),
        (ValueType::Decimal | ValueType::List | ValueType::Object, FactScalar::String(text)) => {
            serde_json::from_str(text).ok()
        }
        _ => None,
    }
}

/// The canonical JSON text of `value`: object members sorted by name at every depth, no
/// whitespace.
pub fn canonical_json(value: &Value) -> String {
    sorted(value).to_string()
}

fn sorted(value: &Value) -> Value {
    match value {
        Value::Object(members) => Value::Object(
            members
                .iter()
                .collect::<BTreeMap<_, _>>()
                .into_iter()
                .map(|(name, member)| (name.clone(), sorted(member)))
                .collect::<Map<_, _>>(),
        ),
        Value::Array(items) => Value::Array(items.iter().map(sorted).collect()),
        other => other.clone(),
    }
}

fn within(range: KeyRange, value: i64) -> bool {
    match range {
        KeyRange::Integer { min, max } => (min..=max).contains(&value),
        KeyRange::Any | KeyRange::Tokens(_) => true,
    }
}
