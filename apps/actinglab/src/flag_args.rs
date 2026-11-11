// SPDX-License-Identifier: AGPL-3.0-only

use super::{CliError, CliOutcome};
use std::collections::BTreeMap;
use std::path::PathBuf;

#[derive(Debug, Clone, Default)]
pub(super) struct FlagArgs {
    pub(super) flags: BTreeMap<String, Vec<String>>,
    pub(super) positionals: Vec<String>,
    /// Workflow #434 LAB-5B3, test builds only: the read list the command these flags were parsed
    /// for holds them to, shared by every copy of them ([`reads`]).
    #[cfg(test)]
    reads: reads::Held,
}

impl FlagArgs {
    /// Flags the code builds rather than parses: `flags` by name, and `positionals`.
    pub(super) fn from_parts(
        flags: BTreeMap<String, Vec<String>>,
        positionals: Vec<String>,
    ) -> Self {
        Self {
            flags,
            positionals,
            #[cfg(test)]
            reads: reads::Held::default(),
        }
    }

    pub(super) fn parse(args: &[String]) -> CliOutcome<Self> {
        Self::parse_with_required_values(args, false)
    }

    pub(super) fn parse_values(args: &[String]) -> CliOutcome<Self> {
        Self::parse_with_required_values(args, true)
    }

    fn parse_with_required_values(args: &[String], require_values: bool) -> CliOutcome<Self> {
        let mut parsed = Self::default();
        let mut index = 0usize;
        while index < args.len() {
            let arg = &args[index];
            if arg.starts_with("--") {
                if index + 1 < args.len() && !args[index + 1].starts_with("--") {
                    parsed
                        .flags
                        .entry(arg.clone())
                        .or_default()
                        .push(args[index + 1].clone());
                    index += 2;
                } else {
                    if require_values {
                        return Err(CliError::usage(format!("missing {arg} <value>")));
                    }
                    parsed
                        .flags
                        .entry(arg.clone())
                        .or_default()
                        .push("true".to_string());
                    index += 1;
                }
            } else {
                parsed.positionals.push(arg.clone());
                index += 1;
            }
        }
        Ok(parsed)
    }

    pub(super) fn bool(&self, name: &str) -> bool {
        #[cfg(test)]
        self.reads.read(name, reads::Form::Switch);
        self.flags
            .get(name)
            .and_then(|values| values.last())
            .is_some_and(|value| value == "true")
    }

    pub(super) fn optional(&self, name: &str) -> Option<String> {
        #[cfg(test)]
        self.reads.read(name, reads::Form::Value);
        self.flags
            .get(name)
            .and_then(|values| values.last())
            .cloned()
    }

    pub(super) fn values(&self, name: &str) -> Vec<String> {
        #[cfg(test)]
        self.reads.read(name, reads::Form::Value);
        self.flags.get(name).cloned().unwrap_or_default()
    }

    /// #411 Lab B-2: how many times `name` was given.
    pub(super) fn count(&self, name: &str) -> usize {
        #[cfg(test)]
        self.reads.read(name, reads::Form::Presence);
        self.flags.get(name).map_or(0, Vec::len)
    }

    /// Workflow #434 LAB-5B3: whether `name` was given, with a value or without one (the reads
    /// that asked the map itself before, so that a held read list sees them).
    pub(super) fn given(&self, name: &str) -> bool {
        #[cfg(test)]
        self.reads.read(name, reads::Form::Presence);
        self.flags.contains_key(name)
    }

    /// Workflow #434 LAB-5B3, test builds only: holds every later read of these flags, and of
    /// every copy of them, to `list`, the read list of the lenient command they were parsed for: a
    /// read of a name outside it, or in the form it does not give the name, fails the test.
    #[cfg(test)]
    pub(super) fn reading(&self, list: &'static reads::FlagReads) {
        self.reads.hold(reads::Rule::List(list));
    }

    /// Workflow #434 LAB-5B3 (work order point 4), test builds only: holds every later read of a
    /// flag the strict row `row`'s command accepts to the form the register gives it
    /// (`cli_module::own_flags::accepted`): one it lists as taking a value read with `bool`, or a
    /// switch read for its value, fails the test. Another name is not held: the command refuses
    /// such a flag by its list before it reads.
    #[cfg(test)]
    pub(super) fn reading_forms(&self, row: &'static str) {
        let accepted = crate::cli_module::own_flags::accepted(row)
            .unwrap_or_else(|| panic!("{row} is a strict row of the register"));
        self.reads.hold(reads::Rule::Forms {
            row,
            options: accepted.options,
            switches: accepted.switches,
        });
    }

    pub(super) fn without_first_positional(&self) -> Self {
        let mut next = self.clone();
        if !next.positionals.is_empty() {
            next.positionals.remove(0);
        }
        next
    }

    pub(super) fn required(&self, name: &str) -> CliOutcome<String> {
        self.optional(name)
            .filter(|value| value != "true")
            .ok_or_else(|| CliError::usage(format!("missing {name} <value>")))
    }

    pub(super) fn optional_path(&self, name: &str) -> Option<PathBuf> {
        self.optional(name)
            .filter(|value| value != "true")
            .map(PathBuf::from)
    }

    pub(super) fn required_path(&self, name: &str) -> CliOutcome<PathBuf> {
        self.required(name).map(PathBuf::from)
    }

    /// Refuses every flag outside `accepted`, the command's flag list (Workflow #434 LAB-5B2: the
    /// one constant its row is held equal to). Every caller's list is empty: it takes positional
    /// arguments only.
    pub(super) fn reject_flags(&self, command: &str, accepted: &[&str]) -> CliOutcome<()> {
        let names = self
            .flags
            .keys()
            .filter(|name| !accepted.contains(&name.as_str()))
            .cloned()
            .collect::<Vec<_>>();
        if names.is_empty() {
            return Ok(());
        }
        Err(CliError::usage(format!(
            "{command} takes positional arguments only; unexpected flags: {}",
            names.join(", ")
        )))
    }

    pub(super) fn expect_positionals(&self, command: &str, expected: usize) -> CliOutcome<()> {
        if self.positionals.len() == expected {
            return Ok(());
        }
        Err(CliError::usage(format!(
            "{command} expects {expected} positional argument(s), got {}",
            self.positionals.len()
        )))
    }

    pub(super) fn required_positional(&self, index: usize, name: &str) -> CliOutcome<&str> {
        self.positionals
            .get(index)
            .map(String::as_str)
            .ok_or_else(|| CliError::usage(format!("missing {name}")))
    }

    pub(super) fn required_i32(&self, index: usize, name: &str) -> CliOutcome<i32> {
        let value = self.required_positional(index, name)?;
        value
            .parse::<i32>()
            .map_err(|err| CliError::usage(format!("failed to parse {name} '{value}': {err}")))
    }

    pub(super) fn required_u64(&self, index: usize, name: &str) -> CliOutcome<u64> {
        let value = self.required_positional(index, name)?;
        value
            .parse::<u64>()
            .map_err(|err| CliError::usage(format!("failed to parse {name} '{value}': {err}")))
    }
}

/// Workflow #434 LAB-5B3 (register §1.287, "Slice LAB-5B3"), test builds only: the read lists of
/// the lenient commands, which take any flag and read the ones they know by name, and the
/// recording of the reads of flags a list holds. Nothing of it is in the product build.
///
/// A lenient command declares its list where it starts ([`FlagArgs::reading`]); from then on
/// every read of its flags (`bool`, `optional`, `values`, `count`, `given` and the readers built on
/// them) is recorded on the reading thread ([`take`]), and a read the list does not give, by name
/// or by form, fails the test with the command's row and the name.
#[cfg(test)]
pub(super) mod reads {
    use std::cell::RefCell;
    use std::sync::{Arc, Mutex, PoisonError};

    /// The flags a lenient command reads, part by part of its code: each part one of its paths,
    /// or a helper several commands share. The command's list is the union of its parts.
    #[derive(Debug)]
    pub(crate) struct FlagReads {
        /// The command's row of the operation table.
        pub(crate) row: &'static str,
        pub(crate) parts: &'static [&'static ReadPart],
    }

    /// One part of a lenient command's reads.
    #[derive(Debug)]
    pub(crate) struct ReadPart {
        /// The flags it reads for their value (`optional`, `values`, `required` and their path
        /// forms): options, which take a value.
        pub(crate) options: &'static [&'static str],
        /// The flags it reads with `bool`: switches, which take none.
        pub(crate) switches: &'static [&'static str],
        /// The flags it reads only to refuse them: the command never takes them.
        pub(crate) refused: &'static [&'static str],
    }

    impl FlagReads {
        /// The options of the list, each once, in the order of its parts.
        pub(crate) fn options(&self) -> Vec<&'static str> {
            self.union(|part| part.options)
        }

        /// The switches of the list, each once.
        pub(crate) fn switches(&self) -> Vec<&'static str> {
            self.union(|part| part.switches)
        }

        /// The refused flags of the list, each once.
        pub(crate) fn refused(&self) -> Vec<&'static str> {
            self.union(|part| part.refused)
        }

        fn union(&self, of: impl Fn(&ReadPart) -> &'static [&'static str]) -> Vec<&'static str> {
            let mut flags = Vec::new();
            for part in self.parts {
                for &flag in of(part) {
                    if !flags.contains(&flag) {
                        flags.push(flag);
                    }
                }
            }
            flags
        }

        /// Whether the list lets its command read `name` in `form`.
        fn lets(&self, name: &str, form: Form) -> Result<(), String> {
            let has = |flags: &[&str]| flags.iter().any(|flag| *flag == name);
            let (option, switch, refused) = (
                has(&self.options()),
                has(&self.switches()),
                has(&self.refused()),
            );
            let row = self.row;
            match form {
                _ if refused => Ok(()),
                _ if !option && !switch => Err(format!(
                    "Workflow #434 LAB-5B3: {row} reads {name}, which its read list does not hold"
                )),
                Form::Presence => Ok(()),
                Form::Value if option => Ok(()),
                Form::Switch if switch => Ok(()),
                Form::Value => Err(format!(
                    "Workflow #434 LAB-5B3: {row} reads {name} for its value, its read list holds \
                     it as a switch"
                )),
                Form::Switch => Err(format!(
                    "Workflow #434 LAB-5B3: {row} reads {name} as a switch, its read list holds \
                     it as an option"
                )),
            }
        }
    }

    /// How a reader reads a flag.
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub(crate) enum Form {
        /// For its value: `optional`, `values`, `required` and their path forms.
        Value,
        /// As a switch: `bool`.
        Switch,
        /// Whether it was given: `count`, `given`.
        Presence,
    }

    /// What a held FlagArgs is held to.
    #[derive(Debug)]
    pub(super) enum Rule {
        /// A lenient command's read list.
        List(&'static FlagReads),
        /// The form the register gives each flag a strict row's command accepts.
        Forms {
            row: &'static str,
            options: Vec<&'static str>,
            switches: Vec<&'static str>,
        },
    }

    impl Rule {
        fn row(&self) -> &'static str {
            match self {
                Self::List(list) => list.row,
                Self::Forms { row, .. } => *row,
            }
        }

        fn lets(&self, name: &str, form: Form) -> Result<(), String> {
            match self {
                Self::List(list) => list.lets(name, form),
                Self::Forms {
                    row,
                    options,
                    switches,
                } => {
                    let has = |flags: &[&str]| flags.iter().any(|flag| *flag == name);
                    match form {
                        Form::Switch if has(options) => Err(format!(
                            "Workflow #434 LAB-5B3: {row} reads {name} as a switch, the register \
                             gives it as an option"
                        )),
                        Form::Value if has(switches) => Err(format!(
                            "Workflow #434 LAB-5B3: {row} reads {name} for its value, the \
                             register gives it as a switch"
                        )),
                        _ => Ok(()),
                    }
                }
            }
        }
    }

    /// The rule a FlagArgs and its copies are held to, `None` until its command declares one.
    #[derive(Debug, Clone, Default)]
    pub(super) struct Held(Arc<Mutex<Option<Rule>>>);

    impl Held {
        pub(super) fn hold(&self, rule: Rule) {
            *self.0.lock().unwrap_or_else(PoisonError::into_inner) = Some(rule);
        }

        /// Records the read of `name` in `form` when a rule is held, and fails the test when the
        /// rule does not let the command read it so.
        pub(super) fn read(&self, name: &str, form: Form) {
            let verdict = {
                let held = self.0.lock().unwrap_or_else(PoisonError::into_inner);
                let Some(rule) = held.as_ref() else {
                    return;
                };
                RECORDED.with(|recorded| {
                    recorded.borrow_mut().push(Read {
                        row: rule.row(),
                        name: name.to_owned(),
                        form,
                    });
                });
                rule.lets(name, form)
            };
            if let Err(failure) = verdict {
                panic!("{failure}");
            }
        }
    }

    /// One recorded read: the row of the rule held, the flag and how it was read.
    #[derive(Debug, Clone, PartialEq, Eq)]
    pub(crate) struct Read {
        pub(crate) row: &'static str,
        pub(crate) name: String,
        pub(crate) form: Form,
    }

    thread_local! {
        static RECORDED: RefCell<Vec<Read>> = const { RefCell::new(Vec::new()) };
    }

    /// Every read the held flags recorded on this thread since the last call, in order.
    pub(crate) fn take() -> Vec<Read> {
        RECORDED.with(|recorded| std::mem::take(&mut *recorded.borrow_mut()))
    }
}
