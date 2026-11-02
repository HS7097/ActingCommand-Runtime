// SPDX-License-Identifier: AGPL-3.0-only

//! The message a channel receives (Workflow #432 AGT-1a; `contracts/agent-wake.md`, "The message"):
//!
//! ```text
//! ActingCommand to <channel>
//! <kind word> <instance alias> 正文 <absolute body path>
//! ```
//!
//! The limit counts the whole message in Unicode scalars, the header included. Only the alias is
//! ever cut, ending in `…`; it can be read back from the body. The kind word and the path are never
//! cut: a message that cannot hold them is refused, and nothing is sent.

use actingcommand_runtime_config::agent_wake::{
    KIND_DAILY_SUMMARY, KIND_DELIVERY_TEST, KIND_INSTANCE_FENCED, KIND_LADDER_EXHAUSTED,
    KIND_RUN_FAILED,
};

/// Line 1 apart from the channel. It never varies.
pub(crate) const HEADER_PREFIX: &str = "ActingCommand to ";
/// The word before the body path.
const BODY_WORD: &str = "正文";
/// What marks a cut alias.
const CUT_MARK: char = '…';

/// The four characters line 2 starts with, for a kind of the section's closed list.
pub(crate) fn kind_word(kind: &str) -> Option<&'static str> {
    match kind {
        KIND_LADDER_EXHAUSTED => Some("阶梯用尽"),
        KIND_INSTANCE_FENCED => Some("实例隔离"),
        KIND_RUN_FAILED => Some("运行失败"),
        KIND_DAILY_SUMMARY => Some("今日汇总"),
        KIND_DELIVERY_TEST => Some("发送测试"),
        _ => None,
    }
}

/// A message ready to send.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct WakeMessage {
    pub(crate) text: String,
    /// Its length in Unicode scalars, which the row records.
    pub(crate) chars: usize,
    pub(crate) alias_cut: bool,
}

/// A message that cannot hold its header, kind word and path within the limit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct MessageTooLong {
    /// The length with no alias at all.
    pub(crate) needed: usize,
    pub(crate) max_chars: usize,
}

/// The message of a wake to `channel`.
pub(crate) fn render_message(
    channel: &str,
    word: &str,
    alias: Option<&str>,
    body_path: &str,
    max_chars: usize,
) -> Result<WakeMessage, MessageTooLong> {
    let compose = |alias: Option<&str>| match alias {
        Some(alias) => format!("{HEADER_PREFIX}{channel}\n{word} {alias} {BODY_WORD} {body_path}"),
        None => format!("{HEADER_PREFIX}{channel}\n{word} {BODY_WORD} {body_path}"),
    };
    let finished = |text: String, alias_cut: bool| WakeMessage {
        chars: text.chars().count(),
        text,
        alias_cut,
    };
    let bare = compose(None);
    let needed = bare.chars().count();
    if needed > max_chars {
        return Err(MessageTooLong { needed, max_chars });
    }
    let Some(alias) = alias.filter(|alias| !alias.is_empty()) else {
        return Ok(finished(bare, false));
    };
    // The room for a space and the alias.
    let room = max_chars - needed;
    let alias_chars = alias.chars().count();
    if alias_chars < room {
        return Ok(finished(compose(Some(alias)), false));
    }
    // A space and the mark are the least of a cut alias; with less room it goes altogether.
    if room < 2 {
        return Ok(finished(bare, true));
    }
    let cut = alias
        .chars()
        .take(room - 2)
        .chain(std::iter::once(CUT_MARK))
        .collect::<String>();
    Ok(finished(compose(Some(cut.as_str())), true))
}

#[cfg(test)]
mod tests {
    use super::*;
    use actingcommand_runtime_config::ConfigCode;
    use actingcommand_runtime_config::agent_wake::{
        KINDS, MESSAGE_FIXED_CHARS, body_folder_text, message_floor_chars,
        refuse_channels_that_cannot_fit,
    };
    use std::path::Path;

    const PATH: &str = r"F:\AC\state\agent-wakes\w1793000000000.json";

    #[test]
    fn message_counts_unicode_scalars_including_the_header() {
        let message = render_message("总览", "阶梯用尽", Some("node-1"), PATH, 100)
            .expect("the typical message fits");
        // Chinese characters count one each, whatever their bytes.
        assert_eq!(message.chars, 78);
        assert_eq!(message.text.chars().count(), 78);
        assert!(message.text.len() > 78);
        assert!(!message.alias_cut);
        // At exactly its own length it still fits whole; one less cuts the alias.
        assert_eq!(
            render_message("总览", "阶梯用尽", Some("node-1"), PATH, 78),
            Ok(message.clone())
        );
        let cut = render_message("总览", "阶梯用尽", Some("node-1"), PATH, 77).expect("it fits");
        assert!(cut.alias_cut);
        assert_eq!(cut.chars, 77);
    }

    #[test]
    fn message_first_line_is_exactly_the_fixed_header_and_channel() {
        let message = render_message("总览", "阶梯用尽", Some("node-1"), PATH, 100).expect("fits");
        assert_eq!(
            message.text,
            format!("ActingCommand to 总览\n阶梯用尽 node-1 正文 {PATH}")
        );
        let mut lines = message.text.split('\n');
        assert_eq!(lines.next(), Some("ActingCommand to 总览"));
        assert!(lines.next().is_some());
        assert_eq!(lines.next(), None, "two lines and no blank line");
        // Every kind of the closed list has its four-character word.
        for kind in KINDS {
            let word = kind_word(kind).expect("a word of the closed list");
            assert_eq!(word.chars().count(), 4, "{kind}");
        }
        assert_eq!(kind_word("neutral_kind"), None);
    }

    #[test]
    fn message_cuts_only_the_alias_and_marks_it() {
        let alias = "neutral-alias-with-a-long-name";
        let whole = format!("ActingCommand to 总览\n运行失败 {alias} 正文 {PATH}");
        let limit = whole.chars().count() - 10;
        let message = render_message("总览", "运行失败", Some(alias), PATH, limit).expect("fits");
        assert!(message.alias_cut);
        assert_eq!(message.chars, limit);
        // Line 1, the kind word and the path stand as they were; the alias keeps its start.
        let (first, second) = message.text.split_once('\n').expect("two lines");
        assert_eq!(first, "ActingCommand to 总览");
        let cut = second
            .strip_prefix("运行失败 ")
            .and_then(|rest| rest.strip_suffix(&format!(" 正文 {PATH}")))
            .expect("the kind word and the path stand");
        assert!(cut.ends_with('…'));
        assert!(alias.starts_with(cut.trim_end_matches('…')));
        // A wide alias is cut by scalar, never inside a character.
        let wide = "实例别名".repeat(8);
        let bare = render_message("总览", "运行失败", None, PATH, 100)
            .expect("fits")
            .chars;
        let message =
            render_message("总览", "运行失败", Some(&wide), PATH, bare + 6).expect("fits");
        assert!(message.alias_cut);
        assert_eq!(message.chars, bare + 6);
        assert!(message.text.contains(" 实例别名…"));
    }

    #[test]
    fn message_never_cuts_the_kind_or_the_path() {
        // With no room for a space and a mark the alias goes altogether.
        let bare = render_message("总览", "实例隔离", None, PATH, 100).expect("fits");
        for room in [0, 1] {
            let message =
                render_message("总览", "实例隔离", Some("node-1"), PATH, bare.chars + room)
                    .expect("the header, kind word and path fit");
            assert_eq!(message.text, bare.text);
            assert!(message.alias_cut);
        }
        // One less than they need is refused whole, and no message comes of it.
        assert_eq!(
            render_message("总览", "实例隔离", Some("node-1"), PATH, bare.chars - 1),
            Err(MessageTooLong {
                needed: bare.chars,
                max_chars: bare.chars - 1,
            })
        );
    }

    #[test]
    fn message_with_no_instance_omits_the_alias() {
        let message = render_message("总览", "发送测试", None, PATH, 100).expect("fits");
        assert_eq!(
            message.text,
            format!("ActingCommand to 总览\n发送测试 正文 {PATH}")
        );
        assert!(!message.alias_cut);
        assert_eq!(
            render_message("总览", "发送测试", Some(""), PATH, 100),
            Ok(message)
        );
    }

    #[test]
    fn message_floor_is_refused_for_a_channel_that_cannot_fit() {
        let root = Path::new(r"F:\AC\state");
        let longest = "频".repeat(16);
        // The floor is a real message: the longest file name, with no alias.
        let folder = body_folder_text(root, "agent-wakes");
        assert_eq!(folder, r"F:\AC\state\agent-wakes\");
        let path = format!("{folder}w1793000000000-99.json");
        let floor = message_floor_chars(&longest, &folder);
        assert_eq!(floor, 88);
        assert_eq!(floor, MESSAGE_FIXED_CHARS + 16 + folder.chars().count());
        assert_eq!(
            render_message(&longest, "发送测试", None, &path, floor).map(|message| message.chars),
            Ok(floor)
        );
        assert!(render_message(&longest, "发送测试", None, &path, floor - 1).is_err());
        // `check-config` passes at the floor and refuses below it, naming the key.
        let channels = ["总览", longest.as_str()];
        refuse_channels_that_cannot_fit(88, root, "agent-wakes", channels)
            .expect("every channel fits at the floor");
        let refused = refuse_channels_that_cannot_fit(87, root, "agent-wakes", channels)
            .expect_err("the longest channel does not fit");
        assert_eq!(refused.failure_code(), ConfigCode::AgentWakeConfigInvalid);
        assert_eq!(refused.config_key(), "agent_wake.summary.max_chars");
    }
}
