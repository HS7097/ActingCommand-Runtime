// SPDX-License-Identifier: AGPL-3.0-only

use super::{CliError, CliOutcome};
use actingcommand_lab_ops::codes::ArgumentProblem;
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

    /// Workflow #434 LAB-5B4 (register §1.287, "Slice LAB-5B4"): `args` read for the own command
    /// whose read list is `list`, before the command does anything. A flag the list does not hold
    /// is refused (`unknown`). A switch of the list takes the next word as its value only when it
    /// is `true` or `false`, and leaves any other where it is, a positional or the next flag. An
    /// option takes the next word as [`parse`](Self::parse) does (none when it begins with `--`),
    /// and is refused at the end of the line (`value_missing`). A flag the list reads only to
    /// refuse it is read as `parse` reads it, for the command's own refusal. A positional word
    /// beyond the list's positionals, or other than the word a positional of the list is, is
    /// refused (`unknown`). Each refusal is the one the CLI module gives a row's short words
    /// (`cli_module::refused_word`), naming the word. In test builds the flags are held to the
    /// list ([`reading`](Self::reading)).
    pub(super) fn parse_reading(
        args: &[String],
        list: &'static reads::FlagReads,
    ) -> CliOutcome<Self> {
        let mut parsed = Self::default();
        let mut index = 0usize;
        while index < args.len() {
            let word = &args[index];
            if !word.starts_with("--") {
                if !list.takes_positional(parsed.positionals.len(), word) {
                    return Err(refused(list.row, word, ArgumentProblem::Unknown));
                }
                parsed.positionals.push(word.clone());
                index += 1;
                continue;
            }
            let next = args.get(index + 1);
            let value = match list.form(word) {
                Some(reads::Taken::Switch) => next.filter(|next| switch_value(next)),
                Some(reads::Taken::Option) if next.is_none() => {
                    return Err(refused(list.row, word, ArgumentProblem::ValueMissing));
                }
                Some(reads::Taken::Option | reads::Taken::Refused) => {
                    next.filter(|next| !next.starts_with("--"))
                }
                None => return Err(refused(list.row, word, ArgumentProblem::Unknown)),
            };
            parsed.push(word, value);
            index += 1 + usize::from(value.is_some());
        }
        #[cfg(test)]
        parsed.reading(list);
        Ok(parsed)
    }

    /// Workflow #434 LAB-5B4: [`parse`](Self::parse) for a strict command, which checks its flags
    /// against its own lists, but a flag of `switches`, the flags it takes without a value, takes
    /// the next word as its value only when it is `true` or `false`; any other next word stays
    /// where it is, a positional or the next flag.
    pub(super) fn parse_switches(args: &[String], switches: &[&str]) -> Self {
        let mut parsed = Self::default();
        let mut index = 0usize;
        while index < args.len() {
            let word = &args[index];
            if !word.starts_with("--") {
                parsed.positionals.push(word.clone());
                index += 1;
                continue;
            }
            let next = args.get(index + 1);
            let value = if switches.contains(&word.as_str()) {
                next.filter(|next| switch_value(next))
            } else {
                next.filter(|next| !next.starts_with("--"))
            };
            parsed.push(word, value);
            index += 1 + usize::from(value.is_some());
        }
        parsed
    }

    /// Workflow #434 LAB-5B4: refuses the first positional word the strict command `command` does
    /// not read, `positionals` being those it reads (as [`reads::FlagReads::positionals`] names
    /// them), in the form of [`parse_reading`](Self::parse_reading).
    pub(super) fn refuse_unread_positionals(
        &self,
        command: &str,
        positionals: &[&str],
    ) -> CliOutcome<()> {
        match self
            .positionals
            .iter()
            .enumerate()
            .find(|(index, word)| !reads::takes_positional(positionals, *index, word))
        {
            Some((_, word)) => Err(refused(command, word, ArgumentProblem::Unknown)),
            None => Ok(()),
        }
    }

    /// One more value of the flag `flag`: `value`, or `true` when it takes none.
    fn push(&mut self, flag: &str, value: Option<&String>) {
        self.flags
            .entry(flag.to_owned())
            .or_default()
            .push(value.cloned().unwrap_or_else(|| "true".to_owned()));
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
    /// every copy of them, to `list`, the read list of the own command they were parsed for (since
    /// LAB-5B4 [`parse_reading`](Self::parse_reading) declares it): a read of a name outside it,
    /// or in the form it does not give the name, fails the test.
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

/// Whether `word` is the value a switch takes from the next word: `true` or `false` (Workflow
/// #434 LAB-5B4).
fn switch_value(word: &str) -> bool {
    matches!(word, "true" | "false")
}

/// Workflow #434 LAB-5B4: the refusal of `word` on the command line of the own command `command`.
fn refused(command: &str, word: &str, problem: ArgumentProblem) -> CliError {
    crate::cli_module::refused_word(command, word, problem)
}

/// Workflow #434 LAB-5B3 and LAB-5B4 (register §1.287, "Slice LAB-5B3", "Slice LAB-5B4"): the
/// read lists of the own commands that read their flags by name, and, in test builds only, the
/// recording of the reads of flags a list holds.
///
/// Since LAB-5B4 such a command parses its command line by its list
/// ([`FlagArgs::parse_reading`]), which refuses a flag or a positional the list does not hold. In
/// test builds the parse holds the flags to the list ([`FlagArgs::reading`]); from then on every
/// read of them (`bool`, `optional`, `values`, `count`, `given` and the readers built on them) is
/// recorded on the reading thread ([`take`]), and a read the list does not give, by name or by
/// form, fails the test with the command's row and the name.
pub(super) mod reads {
    #[cfg(test)]
    use std::cell::RefCell;
    #[cfg(test)]
    use std::sync::{Arc, Mutex, PoisonError};

    /// The flags an own command reads, part by part of its code: each part one of its paths, or a
    /// helper several commands share. The command's list is the union of its parts.
    #[derive(Debug)]
    pub(crate) struct FlagReads {
        /// The command's row of the operation table, its name in a refusal; for an own command no
        /// row is (`run list`, `capture`, `lab receipt`, `lab evidence`), its own words.
        pub(crate) row: &'static str,
        pub(crate) parts: &'static [&'static ReadPart],
        /// Workflow #434 LAB-5B4: the positional words the command reads, in order, each of which
        /// may be absent: a name in angle brackets takes any word (`<target>`), another is that
        /// word itself (`capture`'s `diagnose`). A word beyond them is refused.
        pub(crate) positionals: &'static [&'static str],
    }

    /// Workflow #434 LAB-5B4: how a read list takes a flag from the command line.
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub(crate) enum Taken {
        /// An option: the next word is its value.
        Option,
        /// A switch: the next word only when it is `true` or `false`.
        Switch,
        /// Read only to be refused: as `FlagArgs::parse` reads it.
        Refused,
    }

    /// Workflow #434 LAB-5B4: whether a command whose positionals are `positionals` (as
    /// [`FlagReads::positionals`] names them) reads `word` as its positional number `index`.
    pub(crate) fn takes_positional(positionals: &[&str], index: usize, word: &str) -> bool {
        positionals
            .get(index)
            .is_some_and(|name| name.starts_with('<') || *name == word)
    }

    /// One part of an own command's reads.
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
        /// Workflow #434 LAB-5B4: how the list takes the flag `name`: an option of some part, else
        /// a switch of some part, else a flag some part reads only to refuse it; `None` for a flag
        /// the list does not hold.
        pub(crate) fn form(&self, name: &str) -> Option<Taken> {
            let mut form = None;
            for part in self.parts {
                if part.options.contains(&name) {
                    return Some(Taken::Option);
                }
                if part.switches.contains(&name) {
                    form = Some(Taken::Switch);
                } else if form.is_none() && part.refused.contains(&name) {
                    form = Some(Taken::Refused);
                }
            }
            form
        }

        /// Workflow #434 LAB-5B4: whether the command reads `word` as its positional number
        /// `index`.
        pub(crate) fn takes_positional(&self, index: usize, word: &str) -> bool {
            takes_positional(self.positionals, index, word)
        }

        /// The options of the list, each once, in the order of its parts.
        #[cfg(test)]
        pub(crate) fn options(&self) -> Vec<&'static str> {
            self.union(|part| part.options)
        }

        /// The switches of the list, each once.
        #[cfg(test)]
        pub(crate) fn switches(&self) -> Vec<&'static str> {
            self.union(|part| part.switches)
        }

        /// The refused flags of the list, each once.
        #[cfg(test)]
        pub(crate) fn refused(&self) -> Vec<&'static str> {
            self.union(|part| part.refused)
        }

        #[cfg(test)]
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
        #[cfg(test)]
        fn lets(&self, name: &str, form: Form) -> Result<(), String> {
            let has = |flags: &[&str]| flags.contains(&name);
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

    /// How a reader reads a flag (test builds only).
    #[cfg(test)]
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub(crate) enum Form {
        /// For its value: `optional`, `values`, `required` and their path forms.
        Value,
        /// As a switch: `bool`.
        Switch,
        /// Whether it was given: `count`, `given`.
        Presence,
    }

    /// What a held FlagArgs is held to (test builds only).
    #[cfg(test)]
    #[derive(Debug)]
    pub(super) enum Rule {
        /// An own command's read list.
        List(&'static FlagReads),
        /// The form the register gives each flag a strict row's command accepts.
        Forms {
            row: &'static str,
            options: Vec<&'static str>,
            switches: Vec<&'static str>,
        },
    }

    #[cfg(test)]
    impl Rule {
        fn row(&self) -> &'static str {
            match self {
                Self::List(list) => list.row,
                Self::Forms { row, .. } => row,
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
                    let has = |flags: &[&str]| flags.contains(&name);
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

    /// The rule a FlagArgs and its copies are held to, `None` until its command declares one
    /// (test builds only).
    #[cfg(test)]
    #[derive(Debug, Clone, Default)]
    pub(super) struct Held(Arc<Mutex<Option<Rule>>>);

    #[cfg(test)]
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

    /// One recorded read: the row of the rule held, the flag and how it was read (test builds
    /// only).
    #[cfg(test)]
    #[derive(Debug, Clone, PartialEq, Eq)]
    pub(crate) struct Read {
        pub(crate) row: &'static str,
        pub(crate) name: String,
        pub(crate) form: Form,
    }

    #[cfg(test)]
    thread_local! {
        static RECORDED: RefCell<Vec<Read>> = const { RefCell::new(Vec::new()) };
    }

    /// Every read the held flags recorded on this thread since the last call, in order (test
    /// builds only).
    #[cfg(test)]
    pub(crate) fn take() -> Vec<Read> {
        RECORDED.with(|recorded| std::mem::take(&mut *recorded.borrow_mut()))
    }
}
