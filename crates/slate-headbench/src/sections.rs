//! Which sections an example was asked for, refusing a name it does not have.
//!
//! Every example here takes bare section names as arguments — `head_report
//! stream lease` — and each grew the same three lines:
//!
//! ```ignore
//! let requested: Vec<String> = std::env::args().skip(1).collect();
//! let wanted = |name: &str| requested.is_empty() || requested.iter().any(|s| s == name);
//! ```
//!
//! `ledger/2026-09-21-the-benchmarks-run-now-and-a-fourth-was-broken.md`
//! recorded what that costs:
//!
//! > **A nonsense section argument runs nothing and exits 0.** `head_report
//! > --typo` matches no section, prints its header and stops.
//!
//! An exit code of 0 over an empty report is the shape this repository keeps
//! meeting: a skip that reads as a pass. It is worse here than in a test
//! runner, because the thing a reader takes away from a benchmark is a table,
//! and a *missing* table looks like a section that had nothing to say.
//!
//! # Two refusals, not one
//!
//! [`Selection::new`] refuses a name the example does not have, which catches
//! the typo. [`Selection::confirm`] refuses a **roster** that has stopped
//! matching the sections the example actually offers, which catches the other
//! direction: a section renamed in the body and not in the list would make
//! `new` reject the name a reader correctly typed, and a section deleted from
//! the body would leave a name in the list that selects nothing.
//!
//! The second is the never-fires guard every check in this repository carries,
//! moved inside the program because there is nowhere else it can live: the
//! roster is a constant in an example, and no static check knows which strings
//! in a `.rs` file are section names.
//!
//! `confirm` panics rather than returning, deliberately. It is a statement
//! about the program's own consistency, not about its input, and the audience
//! for it is whoever just edited the example — the same reason
//! `debug_assert!`-shaped invariants are not `Result`.

use std::collections::BTreeSet;

/// The sections an example was asked to run.
#[derive(Debug)]
pub struct Selection {
    known: Vec<String>,
    requested: Vec<String>,
    asked: BTreeSet<String>,
}

impl Selection {
    /// Read `args` as section names, refusing any that `known` does not list.
    ///
    /// An empty `args` means every section, which is what running the example
    /// with no arguments has always meant.
    ///
    /// # Errors
    ///
    /// Returns the message to print when an argument names no section. It
    /// lists the names that *are* available, because a reader who mistyped one
    /// wants the list and not a usage line.
    pub fn new<S: AsRef<str>>(args: &[S], known: &[&str]) -> Result<Self, String> {
        let known: Vec<String> = known.iter().map(|name| (*name).to_owned()).collect();
        let requested: Vec<String> = args.iter().map(|arg| arg.as_ref().to_owned()).collect();
        // Every unknown name, not the first: a reader who typed two wants both,
        // and stopping at the first turns one fix into two runs.
        let unknown: Vec<&String> = requested.iter().filter(|a| !known.contains(a)).collect();
        if !unknown.is_empty() {
            let bad: Vec<&str> = unknown.iter().map(|a| a.as_str()).collect();
            return Err(format!(
                "no section named {}. This example has: {}.\n\
                 Run it with no arguments for all of them.",
                bad.join(", "),
                known.join(", "),
            ));
        }
        Ok(Self {
            known,
            requested,
            asked: BTreeSet::new(),
        })
    }

    /// Should this section run? Records the name for [`Selection::confirm`].
    pub fn wants(&mut self, name: &str) -> bool {
        self.asked.insert(name.to_owned());
        self.requested.is_empty() || self.requested.iter().any(|one| one == name)
    }

    /// Panic unless the sections asked about are exactly the roster.
    ///
    /// Call once, after the last section. See the module docs for why this is
    /// a panic and why it cannot be a static check.
    ///
    /// # Panics
    ///
    /// When a name is in the roster and never asked about, or asked about and
    /// not in the roster.
    pub fn confirm(&self) {
        let known: BTreeSet<&str> = self.known.iter().map(String::as_str).collect();
        let asked: BTreeSet<&str> = self.asked.iter().map(String::as_str).collect();
        assert!(
            known == asked,
            "this example's section roster has drifted from its body.\n  \
             in the roster and never run: {:?}\n  \
             run and not in the roster: {:?}\n  \
             The roster is what refuses a mistyped argument, so a stale one \
             refuses a correct name or accepts a dead one.",
            known.difference(&asked).collect::<Vec<_>>(),
            asked.difference(&known).collect::<Vec<_>>(),
        );
    }
}

/// Read `std::env::args()`, or print the refusal and exit 2.
///
/// The examples all want the same four lines around [`Selection::new`], and
/// four lines copied five times is the shape that drifts.
#[must_use]
pub fn from_args(known: &[&str]) -> Selection {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match Selection::new(&args, known) {
        Ok(selection) => selection,
        Err(why) => {
            eprintln!("{why}");
            // 2, not 1: `run_examples.sh` reads a non-zero exit as "the
            // benchmark failed", and this is "you asked for the wrong thing".
            // Nothing distinguishes them today; the number is here so that
            // something can.
            std::process::exit(2);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::Selection;

    const KNOWN: [&str; 3] = ["rpc", "stream", "lease"];

    #[test]
    fn no_arguments_means_every_section() {
        let mut chosen = Selection::new::<String>(&[], &KNOWN).expect("no arguments");
        assert!(chosen.wants("rpc"));
        assert!(chosen.wants("stream"));
        assert!(chosen.wants("lease"));
    }

    #[test]
    fn a_named_section_runs_and_the_others_do_not() {
        let mut chosen = Selection::new(&["stream"], &KNOWN).expect("a known name");
        assert!(!chosen.wants("rpc"));
        assert!(chosen.wants("stream"));
        assert!(!chosen.wants("lease"));
    }

    #[test]
    fn a_mistyped_section_is_refused_rather_than_running_nothing() {
        // The defect this module exists for: `head_report --typo` matched no
        // section, printed its header and exited 0.
        let why = Selection::new(&["--typo"], &KNOWN).expect_err("a name with no section");
        assert!(why.contains("no section named --typo"), "{why}");
    }

    #[test]
    fn the_refusal_lists_the_sections_there_are() {
        let why = Selection::new(&["strem"], &KNOWN).expect_err("a near miss");
        assert!(why.contains("rpc, stream, lease"), "{why}");
    }

    #[test]
    fn every_unknown_name_is_reported_not_only_the_first() {
        let why = Selection::new(&["one", "stream", "two"], &KNOWN).expect_err("two bad names");
        assert!(why.contains("one, two"), "{why}");
    }

    #[test]
    fn a_roster_matching_the_body_confirms() {
        let mut chosen = Selection::new(&["stream"], &KNOWN).expect("a known name");
        for name in KNOWN {
            chosen.wants(name);
        }
        chosen.confirm();
    }

    #[test]
    #[should_panic(expected = "in the roster and never run: [\"lease\"]")]
    fn a_section_in_the_roster_that_never_runs_is_refused() {
        let mut chosen = Selection::new::<String>(&[], &KNOWN).expect("no arguments");
        chosen.wants("rpc");
        chosen.wants("stream");
        chosen.confirm();
    }

    #[test]
    #[should_panic(expected = "run and not in the roster: [\"views\"]")]
    fn a_section_run_that_is_not_in_the_roster_is_refused() {
        let mut chosen = Selection::new::<String>(&[], &KNOWN).expect("no arguments");
        for name in KNOWN {
            chosen.wants(name);
        }
        chosen.wants("views");
        chosen.confirm();
    }

    #[test]
    fn asking_twice_about_one_section_is_not_drift() {
        // `wants` is called once per section today, and a loop that called it
        // twice would be ordinary. The roster is a set for that reason.
        let mut chosen = Selection::new::<String>(&[], &KNOWN).expect("no arguments");
        for name in KNOWN {
            chosen.wants(name);
            chosen.wants(name);
        }
        chosen.confirm();
    }
}
