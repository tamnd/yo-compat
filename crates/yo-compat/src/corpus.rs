//! Corpus files, and the register of divergences we have already agreed to.
//!
//! A corpus file is a list of commands, one per line, written the way you would
//! type them into redis-cli. There are no expected answers in it, because the
//! expected answer is whatever Redis says, and writing them down by hand would
//! turn this into a test of our reading of the documentation.

use std::collections::BTreeMap;
use std::path::Path;

/// One command out of a corpus file, with the line it came from so a failure
/// report points at something you can open.
pub struct Step {
    pub line: usize,
    pub args: Vec<String>,
}

pub struct Corpus {
    pub steps: Vec<Step>,
}

impl Corpus {
    pub fn load(path: &Path) -> Result<Corpus, String> {
        let text = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
        let mut steps = Vec::new();
        for (at, line) in text.lines().enumerate() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let args = split(line).map_err(|e| format!("{}:{}: {e}", path.display(), at + 1))?;
            if !args.is_empty() {
                steps.push(Step { line: at + 1, args });
            }
        }
        Ok(Corpus { steps })
    }
}

/// Split a line into arguments, respecting double quotes.
///
/// Quotes matter more than they look like they should. Half the interesting
/// cases in a string corpus are about empty values, values with spaces in them
/// and values that are only whitespace, and none of those can be written down
/// without a way to quote them.
fn split(line: &str) -> Result<Vec<String>, String> {
    let mut args = Vec::new();
    let mut cur = String::new();
    let mut in_quotes = false;
    let mut started = false;
    let mut chars = line.chars();

    while let Some(c) = chars.next() {
        match c {
            '"' => {
                in_quotes = !in_quotes;
                started = true;
            }
            '\\' if in_quotes => match chars.next() {
                Some('n') => cur.push('\n'),
                Some('r') => cur.push('\r'),
                Some('t') => cur.push('\t'),
                Some('\\') => cur.push('\\'),
                Some('"') => cur.push('"'),
                Some(other) => cur.push(other),
                None => return Err("a line ended in a backslash".to_string()),
            },
            c if c.is_whitespace() && !in_quotes => {
                if started || !cur.is_empty() {
                    args.push(std::mem::take(&mut cur));
                    started = false;
                }
            }
            c => {
                cur.push(c);
                started = true;
            }
        }
    }
    if in_quotes {
        return Err("a quote was never closed".to_string());
    }
    if started || !cur.is_empty() {
        args.push(cur);
    }
    Ok(args)
}

/// The divergences we have decided to live with.
///
/// A divergence in here is a claim with a reason attached, not a test being
/// switched off. Everything else that differs fails the run. The point of the
/// file is that saying "we behave differently here and this is why" has to be a
/// diff somebody wrote and somebody else read.
#[derive(Default)]
pub struct Register {
    /// corpus file name to the command prefixes excused in it.
    excused: BTreeMap<String, Vec<String>>,
}

impl Register {
    pub fn load(root: &Path) -> Result<Register, String> {
        let path = root.join("divergences.toml");
        let Ok(text) = std::fs::read_to_string(&path) else {
            return Ok(Register::default());
        };
        Register::parse(&text).map_err(|e| format!("{}: {e}", path.display()))
    }

    /// A small hand written reader for a small hand written file. It wants
    /// `[[divergence]]` tables with a `corpus`, a `command` and a `reason`, and
    /// it insists on the reason, because a divergence with no reason on it is
    /// the thing this file exists to prevent.
    fn parse(text: &str) -> Result<Register, String> {
        let mut out = Register::default();
        let mut corpus: Option<String> = None;
        let mut command: Option<String> = None;
        let mut reason: Option<String> = None;

        let mut flush = |corpus: &mut Option<String>,
                         command: &mut Option<String>,
                         reason: &mut Option<String>|
         -> Result<(), String> {
            match (corpus.take(), command.take(), reason.take()) {
                (None, None, None) => Ok(()),
                (Some(c), Some(cmd), Some(r)) if !r.is_empty() => {
                    out.excused.entry(c).or_default().push(cmd);
                    Ok(())
                }
                _ => Err("every divergence needs a corpus, a command and a reason".to_string()),
            }
        };

        for line in text.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            if line == "[[divergence]]" {
                flush(&mut corpus, &mut command, &mut reason)?;
                continue;
            }
            let Some((key, value)) = line.split_once('=') else {
                return Err(format!("not a key and a value: {line}"));
            };
            let value = value.trim().trim_matches('"').to_string();
            match key.trim() {
                "corpus" => corpus = Some(value),
                "command" => command = Some(value),
                "reason" => reason = Some(value),
                other => return Err(format!("no such field: {other}")),
            }
        }
        flush(&mut corpus, &mut command, &mut reason)?;
        Ok(out)
    }

    /// Is this mismatch one we have already signed for?
    ///
    /// Matched on a prefix so a register entry can cover a command rather than
    /// one exact set of arguments. `SETRANGE` excuses every SETRANGE in that
    /// file, `SETRANGE key 5` excuses only the ones that start that way.
    pub fn excuses(&self, corpus: &str, command: &str) -> bool {
        let Some(prefixes) = self.excused.get(corpus) else {
            return false;
        };
        let lower = command.to_ascii_lowercase();
        prefixes
            .iter()
            .any(|p| lower.starts_with(&p.to_ascii_lowercase()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_quoted_empty_value_is_an_argument() {
        let args = split(r#"SET key """#).unwrap();
        assert_eq!(args, vec!["SET", "key", ""]);
    }

    #[test]
    fn a_value_with_spaces_survives() {
        let args = split(r#"SET key "two words""#).unwrap();
        assert_eq!(args, vec!["SET", "key", "two words"]);
    }

    #[test]
    fn an_unclosed_quote_is_an_error_and_not_a_guess() {
        assert!(split(r#"SET key "unfinished"#).is_err());
    }

    #[test]
    fn a_divergence_without_a_reason_does_not_load() {
        let text = "[[divergence]]\ncorpus = \"string.txt\"\ncommand = \"OBJECT\"\n";
        assert!(Register::parse(text).is_err());
    }

    #[test]
    fn the_register_excuses_by_prefix_and_nothing_else() {
        let text = "[[divergence]]\ncorpus = \"string.txt\"\n\
                    command = \"OBJECT ENCODING\"\nreason = \"we have one encoding\"\n";
        let r = Register::parse(text).unwrap();
        assert!(r.excuses("string.txt", "OBJECT ENCODING key"));
        assert!(!r.excuses("string.txt", "OBJECT REFCOUNT key"));
        assert!(!r.excuses("expire.txt", "OBJECT ENCODING key"));
    }
}
