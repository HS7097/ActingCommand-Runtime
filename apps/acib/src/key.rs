// SPDX-License-Identifier: AGPL-3.0-only

//! The key `acib` names a phone by (board #479, ACIB-2): a salted digest of the identifier
//! usbmuxd lists for it, made inside this program, so that a phone can be named again after it
//! is plugged in again (its usbmuxd session number belongs to one connection) without the
//! identifier ever leaving the program.
//!
//! The key is the Runtime's own fingerprint of a handset's serial, byte for byte: the SHA-256 of
//! the salt, a zero byte, the field name `device_serial`, a zero byte and the identifier, written
//! `sha256:` and 64 lowercase hexadecimal digits (the ledger's `Sha256SecretFingerprinter`, its
//! `SecretField::DeviceSerial` arm). One test vector, the same literal salt, identifier and key,
//! is asserted here and in the ledger's tests, so the two cannot drift apart. This crate depends
//! on no crate of the Runtime: the SHA-256 is the `sha2` the library already brings in.
//!
//! A salt comes only from the environment variable a flag names, never from the command line,
//! and is never printed, logged or written: [`Salt`] has no way to show it. A key is printed only
//! when its salt's flag was given.

use serde::{Serialize, Serializer};
use sha2::{Digest, Sha256};
use std::ffi::OsString;
use std::fmt;

/// The shortest salt, in bytes: the bound of the Lab's `--key-salt-env` and of the
/// installation's own secret.
pub const MIN_SALT_BYTES: usize = 16;
/// The longest salt, in bytes, the same bound's other end.
pub const MAX_SALT_BYTES: usize = 1024;
/// The field the digest names, the ledger's for a handset's serial.
const FIELD: &[u8] = b"device_serial";
/// What a key's text starts with.
const PREFIX: &str = "sha256:";
/// How many lowercase hexadecimal digits follow [`PREFIX`].
const HEX_DIGITS: usize = 64;

/// A salt the keys are made with: 16 to 1024 bytes of UTF-8 text, read from an environment
/// variable. It cannot be shown: it has no `Serialize`, its `Debug` shows none of it, and no
/// answer type has a member of it.
pub struct Salt(Vec<u8>);

impl Salt {
    /// The salt of the environment variable `name`, as `read` reads the variables of this
    /// process; `None` when the variable is unset, is not UTF-8, or holds fewer than
    /// [`MIN_SALT_BYTES`] bytes (an empty one included) or more than [`MAX_SALT_BYTES`].
    pub fn from_environment(name: &str, read: &dyn Fn(&str) -> Option<OsString>) -> Option<Self> {
        let text = read(name)?.into_string().ok()?;
        (MIN_SALT_BYTES..=MAX_SALT_BYTES)
            .contains(&text.len())
            .then(|| Self(text.into_bytes()))
    }
}

impl fmt::Debug for Salt {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("Salt(<redacted>)")
    }
}

/// A phone's key under one salt: `sha256:` and 64 lowercase hexadecimal digits. Its one
/// constructor is the key function, so it holds a digest and never the identifier or the salt.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeviceKey(String);

impl DeviceKey {
    /// The key of the phone usbmuxd lists with `identifier`, under `salt`.
    pub fn of(salt: &Salt, identifier: &str) -> Self {
        let mut digest = Sha256::new();
        digest.update(&salt.0);
        digest.update([0]);
        digest.update(FIELD);
        digest.update([0]);
        digest.update(identifier.as_bytes());
        Self(format!("{PREFIX}{:x}", digest.finalize()))
    }

    /// Whether this is the key `text` names.
    pub fn matches(&self, text: &str) -> bool {
        self.0 == text
    }
}

impl Serialize for DeviceKey {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.0)
    }
}

/// Whether `text` is written as a key is: `sha256:` and 64 lowercase hexadecimal digits.
pub fn is_key_text(text: &str) -> bool {
    text.strip_prefix(PREFIX).is_some_and(|digits| {
        digits.len() == HEX_DIGITS
            && digits
                .bytes()
                .all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
    })
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// The test vector this crate and the ledger's fingerprinter both assert (the ledger's test
    /// `acib2_the_device_key_test_vector_is_the_device_serial_fingerprint`): the same literal
    /// salt, identifier and key.
    pub(crate) const VECTOR_SALT: &str = "acib-and-ledger-shared-test-salt";
    pub(crate) const VECTOR_IDENTIFIER: &str = "TESTSERIAL0001";
    pub(crate) const VECTOR_KEY: &str =
        "sha256:b74e1be265fea5ee2fd1b1b2243df7dcb16530f24a6b1bef41b050f8b2f427bf";

    /// A reader of one variable, `name`, holding `value`; every other variable is unset.
    fn holding(name: &'static str, value: OsString) -> impl Fn(&str) -> Option<OsString> {
        move |asked: &str| (asked == name).then(|| value.clone())
    }

    /// The salt `text`, read from a variable that holds it.
    pub(crate) fn salt(text: &str) -> Salt {
        Salt::from_environment("ACIB_TEST_SALT", &holding("ACIB_TEST_SALT", text.into()))
            .expect("a salt within the bounds")
    }

    #[cfg(windows)]
    fn not_utf8() -> OsString {
        use std::os::windows::ffi::OsStringExt;
        OsString::from_wide(
            &[0x0041; 20]
                .into_iter()
                .chain([0xD800])
                .collect::<Vec<u16>>(),
        )
    }

    #[cfg(unix)]
    fn not_utf8() -> OsString {
        use std::os::unix::ffi::OsStringExt;
        OsString::from_vec([0x41; 20].into_iter().chain([0xFF]).collect())
    }

    /// The key function gives the test vector, the ledger fingerprinter's fingerprint of a
    /// handset's serial for the same salt and identifier.
    #[test]
    fn the_key_function_gives_the_test_vector_the_ledger_gives() {
        let key = DeviceKey::of(&salt(VECTOR_SALT), VECTOR_IDENTIFIER);
        assert!(key.matches(VECTOR_KEY));
        assert_eq!(
            serde_json::to_value(&key).expect("a key serializes"),
            serde_json::Value::String(VECTOR_KEY.to_owned())
        );
        assert!(is_key_text(VECTOR_KEY));
    }

    /// A key holds neither its salt nor its identifier: the same salt and identifier give the
    /// same key, another salt or another identifier another key, and its text is `sha256:` and
    /// 64 lowercase hexadecimal digits; a salt's `Debug` shows none of it.
    #[test]
    fn a_key_holds_neither_its_salt_nor_its_identifier() {
        const SALT: &str = "TESTSALTTESTSALT-one";
        const IDENTIFIER: &str = "TESTSERIAL0001";
        let key = DeviceKey::of(&salt(SALT), IDENTIFIER);
        let text = serde_json::to_string(&key).expect("a key serializes");
        assert!(!text.contains(SALT), "{text}");
        assert!(!text.contains("TESTSALT"), "{text}");
        assert!(!text.contains(IDENTIFIER), "{text}");
        assert!(!text.contains("TESTSERIAL"), "{text}");
        assert_eq!(key, DeviceKey::of(&salt(SALT), IDENTIFIER));
        assert_ne!(
            key,
            DeviceKey::of(&salt("TESTSALTTESTSALT-two"), IDENTIFIER)
        );
        assert_ne!(key, DeviceKey::of(&salt(SALT), "TESTSERIAL0002"));
        let shown: String = serde_json::from_str(&text).expect("a key is a string");
        assert_eq!(shown.len(), PREFIX.len() + HEX_DIGITS);
        assert!(is_key_text(&shown), "{shown}");
        let debug = format!("{:?}", salt(SALT));
        assert!(!debug.contains("TESTSALT"), "{debug}");
    }

    /// A salt comes from the variable named, and only when it is set, UTF-8 and of 16 to 1024
    /// bytes (bytes, not characters).
    #[test]
    fn a_salt_comes_only_from_its_variable_within_its_bounds() {
        let read =
            |value: OsString| Salt::from_environment("ACIB_SALT", &holding("ACIB_SALT", value));
        for (value, usable, case) in [
            (
                OsString::from("x".repeat(MIN_SALT_BYTES)),
                true,
                "the fewest bytes",
            ),
            (
                OsString::from("x".repeat(MAX_SALT_BYTES)),
                true,
                "the most bytes",
            ),
            (
                OsString::from("ä".repeat(8)),
                true,
                "16 bytes in 8 characters",
            ),
            (
                OsString::from("x".repeat(MIN_SALT_BYTES - 1)),
                false,
                "one byte too few",
            ),
            (
                OsString::from("x".repeat(MAX_SALT_BYTES + 1)),
                false,
                "one byte too many",
            ),
            (OsString::from("ä".repeat(7)), false, "14 bytes"),
            (OsString::new(), false, "empty"),
            (not_utf8(), false, "not UTF-8"),
        ] {
            assert_eq!(read(value).is_some(), usable, "{case}");
        }
        let elsewhere = holding("ACIB_OTHER", OsString::from("x".repeat(MIN_SALT_BYTES)));
        assert!(
            Salt::from_environment("ACIB_SALT", &elsewhere).is_none(),
            "unset"
        );
        let other_salt =
            Salt::from_environment("ACIB_OTHER", &elsewhere).expect("the variable named");
        assert_eq!(
            DeviceKey::of(&other_salt, VECTOR_IDENTIFIER),
            DeviceKey::of(&salt(&"x".repeat(MIN_SALT_BYTES)), VECTOR_IDENTIFIER)
        );
    }

    /// A key's text is `sha256:` and exactly 64 lowercase hexadecimal digits.
    #[test]
    fn a_key_is_written_as_sha256_and_64_lowercase_hex_digits() {
        let digits = &VECTOR_KEY[PREFIX.len()..];
        assert!(is_key_text(&format!("sha256:{}", "0".repeat(HEX_DIGITS))));
        for text in [
            String::new(),
            digits.to_owned(),
            format!("sha256:{}", &digits[1..]),
            format!("sha256:{digits}0"),
            format!("SHA256:{digits}"),
            format!("sha256:{}", digits.to_uppercase()),
            format!("sha256:{}", "g".repeat(HEX_DIGITS)),
            format!("sha256: {}", &digits[1..]),
            format!("{VECTOR_KEY}\n"),
        ] {
            assert!(!is_key_text(&text), "{text:?}");
        }
    }
}
