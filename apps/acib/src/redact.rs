// SPDX-License-Identifier: AGPL-3.0-only

//! What `acib` shows of a text it did not write itself (board #479, ACIB-0): a library error's
//! text, the word a device refused a value with, a remote service's name and entitlement, the
//! name of a member of the image mounter's listing. Such a text is shown only when it cannot
//! carry a device's serial number, UDID, ECID, name fragment with digits or address; otherwise
//! it is cut to its kind (a library error), to a fixed word (a refusal) or withheld (a name).
//! The check is deliberately coarse: it cuts some harmless texts rather than let one identifier
//! through. A bundle identifier names an application, not the device, and is shown when it is
//! written in the characters a bundle identifier is made of.

/// The longest run of hexadecimal digits a shown text may hold. A UDID holds 40 (older
/// devices) or 8 and 16 around a dash (newer ones, 24 without it), an ECID up to 20 decimal
/// digits, a serial number letters and digits in one word of 10 to 12.
const LONGEST_HEX_RUN: usize = 7;
/// The longest word (letters and digits) with a digit in it that a shown text may hold.
const LONGEST_WORD_WITH_DIGIT: usize = 7;
/// The longest text shown at all.
const LONGEST_TEXT: usize = 240;
/// The longest word a device's refusal may be shown with.
const LONGEST_REFUSAL_WORD: usize = 64;
/// The kind of an error whose debug form does not start with a name.
const UNNAMED_KIND: &str = "Unnamed";
/// The word shown for a device's refusal whose own word cannot be shown.
const UNKNOWN_REFUSAL: &str = "UnknownErrorType";
/// What a service name that cannot be shown is replaced with.
const WITHHELD_NAME: &str = "withheld";

/// The kind of an error: the name its debug form starts with (an enum variant's name).
pub fn kind_of(debug: &str) -> String {
    let kind: String = debug
        .chars()
        .take_while(|c| c.is_ascii_alphanumeric() || *c == '_')
        .collect();
    if kind.is_empty() {
        UNNAMED_KIND.to_owned()
    } else {
        kind
    }
}

/// A library error as it is shown: its text when the text is shareable, else its kind alone.
pub fn shown_error(kind: &str, text: &str) -> String {
    if shareable(text) {
        text.to_owned()
    } else {
        kind.to_owned()
    }
}

/// The word a device refused a value with, from the device's error text (`<word>` or
/// `<word> (<description>)`): its first word when that is shareable; the description is
/// never shown.
pub fn refusal_word(text: &str) -> String {
    let word: String = text
        .chars()
        .take_while(|c| c.is_ascii_alphanumeric())
        .collect();
    if word.is_empty() || word.len() > LONGEST_REFUSAL_WORD || !shareable(&word) {
        UNKNOWN_REFUSAL.to_owned()
    } else {
        word
    }
}

/// A remote service's name as it is shown: the name when it is shareable, else a fixed word.
pub fn shown_name(name: &str) -> String {
    if shareable(name) {
        name.to_owned()
    } else {
        WITHHELD_NAME.to_owned()
    }
}

/// A bundle identifier as it is shown: as it is when it is made only of the characters Apple
/// allows in one (ASCII letters and digits, `-` and `.`) and is not too long, else a fixed word.
pub fn shown_bundle_identifier(identifier: &str) -> String {
    let own_characters = identifier
        .bytes()
        .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'.');
    if !identifier.is_empty() && identifier.len() <= LONGEST_TEXT && own_characters {
        identifier.to_owned()
    } else {
        WITHHELD_NAME.to_owned()
    }
}

/// Whether `text` can be shown: printable ASCII on one line, not too long, and holding nothing
/// shaped like a device identifier or an address.
pub fn shareable(text: &str) -> bool {
    text.len() <= LONGEST_TEXT
        && text.chars().all(|c| c.is_ascii_graphic() || c == ' ')
        && !text.contains("::")
        && longest_hex_run(text) <= LONGEST_HEX_RUN
        && !holds_long_word_with_digit(text)
        && !holds_dotted_quad(text)
        && !holds_hex_colon_hex(text)
}

fn longest_hex_run(text: &str) -> usize {
    let mut longest = 0;
    let mut run = 0;
    for c in text.chars() {
        if c.is_ascii_hexdigit() {
            run += 1;
            longest = longest.max(run);
        } else {
            run = 0;
        }
    }
    longest
}

fn holds_long_word_with_digit(text: &str) -> bool {
    text.split(|c: char| !c.is_ascii_alphanumeric())
        .any(|word| {
            word.len() > LONGEST_WORD_WITH_DIGIT && word.bytes().any(|b| b.is_ascii_digit())
        })
}

/// Four groups of digits joined by dots, as an IPv4 address is written.
fn holds_dotted_quad(text: &str) -> bool {
    text.split(|c: char| !(c.is_ascii_digit() || c == '.'))
        .any(|piece| piece.split('.').filter(|group| !group.is_empty()).count() >= 4)
}

/// A colon between two hexadecimal digits, as an IPv6 or a hardware address is written.
fn holds_hex_colon_hex(text: &str) -> bool {
    let bytes = text.as_bytes();
    bytes.windows(3).any(|window| {
        window[1] == b':' && window[0].is_ascii_hexdigit() && window[2].is_ascii_hexdigit()
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Obvious placeholders of the two identifier lengths the owner's rule names: a run of 40
    /// hexadecimal digits and one of 24.
    const RUN_OF_40: &str = "0123456789abcdef0123456789abcdef01234567";
    const RUN_OF_24: &str = "0123456789ABCDEF01234567";

    #[test]
    fn a_library_error_with_a_40_or_a_24_digit_hex_run_is_cut_to_its_kind() {
        let kind = kind_of(r#"UnexpectedResponse("no record")"#);
        assert_eq!(kind, "UnexpectedResponse");
        for run in [RUN_OF_40, RUN_OF_24] {
            for text in [
                format!("unexpected response from device: {run}"),
                format!("no pair record for {run}."),
                run.to_owned(),
                format!("device ({run}) not found"),
            ] {
                assert_eq!(shown_error(&kind, &text), "UnexpectedResponse", "{text}");
            }
        }
        // The dashed form of the 24-digit identifier: 8 and 16 around a dash.
        let dashed = format!("device {}-{} gone", &RUN_OF_24[..8], &RUN_OF_24[8..]);
        assert_eq!(shown_error(&kind, &dashed), "UnexpectedResponse");
    }

    #[test]
    fn a_plain_library_error_is_shown_as_it_is() {
        for text in [
            "device socket io failed",
            "unexpected response from device: missing PairRecordData in pair record response",
            "this request was prohibited",
            "connection refused (os error 10061)",
            "not enough bytes, expected 16, got 0",
        ] {
            assert_eq!(shown_error("Socket", text), text);
        }
    }

    #[test]
    fn a_text_shaped_like_an_address_or_a_serial_is_cut() {
        for text in [
            "connect to 192.0.2.1 failed",
            "peer fe80::1 refused",
            "peer fd00:0:0:0::2 refused",
            "interface 00:11:22:33:44:55 down",
            "serial TESTSERIAL0001 refused",
            "line one\nline two",
        ] {
            assert_eq!(shown_error("Socket", text), "Socket", "{text}");
        }
        assert_eq!(
            shown_error("Socket", &"x".repeat(LONGEST_TEXT + 1)),
            "Socket"
        );
    }

    #[test]
    fn kinds_refusal_words_and_service_names() {
        assert_eq!(kind_of("GetProhibited"), "GetProhibited");
        assert_eq!(kind_of("Socket(Os { code: 10061 })"), "Socket");
        assert_eq!(kind_of("(1, 2)"), UNNAMED_KIND);
        assert_eq!(refusal_word("PasswordProtected"), "PasswordProtected");
        assert_eq!(
            refusal_word("MissingValue (no value for this key)"),
            "MissingValue"
        );
        assert_eq!(refusal_word(RUN_OF_40), UNKNOWN_REFUSAL);
        assert_eq!(refusal_word(""), UNKNOWN_REFUSAL);
        assert_eq!(
            shown_name("com.apple.mobile.lockdown.remote.trusted"),
            "com.apple.mobile.lockdown.remote.trusted"
        );
        assert_eq!(
            shown_name(&format!("com.example.{RUN_OF_24}")),
            WITHHELD_NAME
        );
    }

    #[test]
    fn a_bundle_identifier_is_shown_only_in_its_own_characters() {
        for identifier in [
            "com.example.GameOne",
            "com.example.game-two",
            "com.example.game2026",
            "org.example.app.Release3",
        ] {
            assert_eq!(shown_bundle_identifier(identifier), identifier);
        }
        for identifier in [
            String::new(),
            "com.example.end_game".to_owned(),
            "com.example.app two".to_owned(),
            "com.example.app\nline".to_owned(),
            "com.example.äpp".to_owned(),
            format!("com.example.{}", "a".repeat(LONGEST_TEXT)),
        ] {
            assert_eq!(
                shown_bundle_identifier(&identifier),
                WITHHELD_NAME,
                "{identifier:?}"
            );
        }
    }
}
