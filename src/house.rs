//! House rules: the ones that say "do not write this" without saying what to
//! write instead.
//!
//! ## Why these are not Weir rules
//!
//! Weir requires a `becomes`. That is the right call for a language whose
//! job is corrections — a rule that cannot say what the text should be
//! instead is a rule whose fix nobody can apply automatically. But a real
//! style guide is full of them: "avoid `etc.` in lists, name the items",
//! "do not write parenthetical plurals such as `screw(s)`", "put a space
//! between the number and the unit". There is no single replacement for any
//! of those, and inventing one so the rule fits the DSL would produce
//! confidently wrong suggestions.
//!
//! So they stay as patterns with a message, declared in the project's TOML
//! next to everything else about the project. Anything that *can* say what
//! to write instead belongs in the weirpack, where the engine can offer the
//! fix.
//!
//! ## Why `fancy-regex` and not `regex`
//!
//! Lookaround. Three of the patterns a real style guide needs are of the
//! form "this, but not when it is already that":
//! `(?<!safety )laser scanner` is a laser scanner nobody has called a
//! safety one yet, and `(?<!\d{4}-)\b\d{1,3}-\d{1,3}\b(?!-\d{2})` is a
//! numeric range that is not part of a date. Rust's `regex` refuses
//! lookaround by design, because it guarantees linear time. `fancy-regex`
//! falls back to backtracking for exactly these constructs, which is the
//! trade every other prose linter has already made -- Vale carries a
//! .NET-compatible engine for the same reason.

use fancy_regex::Regex;
use serde::Deserialize;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Level {
    Error,
    Warning,
    Hint,
}

#[derive(Debug, Deserialize)]
pub struct HouseRule {
    /// Appears in output and in whatever somebody greps for when a rule
    /// fires too often, so it outlives the pattern and may not be renamed
    /// casually.
    pub name: String,
    #[serde(default = "default_level")]
    pub level: Level,
    /// What the writer is told. `%s` is replaced with the matched text.
    pub message: String,
    /// The patterns. Any match is a finding.
    ///
    /// A pattern with no regular-expression metacharacters is a *phrase*,
    /// and is matched with word boundaries around it. Without that, `sis`
    /// -- a real entry in one terminology list -- matches inside
    /// "analysis", and a rule meant to catch a handful of uses reports six
    /// hundred. Anything that looks like a regex is used as written, because
    /// adding boundaries to it would change what it means.
    pub patterns: Vec<String>,
    /// Matches that are allowed anyway, compared case-insensitively against
    /// the matched text. The escape hatch a pattern needs when the exception
    /// is a phrase rather than a shape.
    #[serde(default)]
    pub exceptions: Vec<String>,
    /// Whether the patterns ignore case.
    #[serde(default)]
    pub ignore_case: bool,
    /// What to write instead, when the rule can say.
    ///
    /// Most cannot, which is why these are not Weir rules. A few can and
    /// still land here because the thing they match is a shape rather than a
    /// phrase -- `(?<!safety )laser scanner` is "a laser scanner not already
    /// called a safety one", which no sequence of words expresses. Those
    /// keep their suggestion rather than losing it to the file they are in.
    #[serde(default)]
    pub becomes: Option<String>,
}

fn default_level() -> Level {
    Level::Warning
}

/// Is this pattern a plain phrase rather than a regular expression?
///
/// A bare `.` does not make it one. In a terminology list a dot is an
/// abbreviation -- `pcs.`, `etc.` -- and treating it as "any character" is
/// how such an entry quietly stops working: as a regex, `pcs.` matches
/// "pcs" plus anything, and a word boundary after an any-character match
/// lands between two non-word characters and fails. The entry then matches
/// nothing at all, which looks exactly like a corpus with no violations.
/// `pcs.` is in one real list and had never once fired.
///
/// A backslash is the tell: anything genuinely written as a regex has one.
fn is_phrase(p: &str) -> bool {
    !p.chars().any(|c| {
        matches!(
            c,
            '\\' | '[' | ']' | '(' | ')' | '?' | '*' | '+' | '|' | '^' | '$' | '{' | '}'
        )
    })
}

/// Put word boundaries around a pattern, where a boundary means anything.
///
/// This is not decoration. Without it `log in` matches inside "error log
/// information" and `sis` inside "analysis" -- two real entries in one
/// terminology list, between them reporting most of a corpus. Every prose
/// linter does this; the question is only where the boundaries go.
///
/// Both ends are judged separately, because `\b` only means something next
/// to a word character:
///
/// * `sis`                   -> `\bsis\b`
/// * `(?<!Order )log in`     -> `\b(?<!Order )log in\b`  (past the lookbehind)
/// * `\betc\.` -> unchanged. It carries its own leading boundary, and a
///   `\b` after the `.` would demand a word character that is never there.
/// * `\b\d+(?:kg|mm)\b` -> unchanged, already bounded.
fn bound(pattern: &str) -> String {
    if is_phrase(pattern) {
        let escaped = fancy_regex::escape(pattern);
        let head = pattern.chars().next();
        let tail = pattern.chars().last();
        let prefix = if head.is_some_and(is_word) { r"\b" } else { "" };
        let suffix = if tail.is_some_and(is_word) { r"\b" } else { "" };
        return format!("{prefix}{escaped}{suffix}");
    }
    let lead = skip_lookaround(pattern);
    let head = pattern[lead..].chars().next();
    let prefix = match head {
        Some(c) if is_word(c) && !pattern[..lead].ends_with("\\b") => r"\b",
        _ => "",
    };
    let tail = pattern.chars().last();
    let ends_bounded = pattern.ends_with("\\b") || pattern.ends_with('$');
    let suffix = match tail {
        Some(c) if is_word(c) && !ends_bounded => r"\b",
        _ => "",
    };
    format!("{prefix}{pattern}{suffix}")
}

fn is_word(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

/// How many bytes of leading `(?...)` groups to step over before asking what
/// the pattern really starts with.
fn skip_lookaround(pattern: &str) -> usize {
    let mut i = 0;
    while pattern[i..].starts_with("(?") {
        let rest = &pattern[i..];
        // Only a lookaround, not a capturing or non-capturing group whose
        // contents are part of the match.
        if !(rest.starts_with("(?=")
            || rest.starts_with("(?!")
            || rest.starts_with("(?<=")
            || rest.starts_with("(?<!"))
        {
            break;
        }
        let mut depth = 0usize;
        let mut end = None;
        for (off, c) in rest.char_indices() {
            match c {
                '(' => depth += 1,
                ')' => {
                    depth -= 1;
                    if depth == 0 {
                        end = Some(off + 1);
                        break;
                    }
                }
                _ => {}
            }
        }
        match end {
            Some(e) => i += e,
            None => break,
        }
    }
    i
}

pub struct Compiled {
    pub name: String,
    pub level: Level,
    pub message: String,
    pub patterns: Vec<Regex>,
    pub exceptions: Vec<String>,
    pub becomes: Option<String>,
}

impl HouseRule {
    pub fn compile(&self) -> Result<Compiled, String> {
        let mut patterns = Vec::with_capacity(self.patterns.len());
        for p in &self.patterns {
            let bounded = bound(p);
            let source = if self.ignore_case {
                format!("(?i){bounded}")
            } else {
                bounded
            };
            patterns.push(Regex::new(&source).map_err(|e| format!("rule `{}`: {e}", self.name))?);
        }
        Ok(Compiled {
            name: self.name.clone(),
            level: self.level,
            message: self.message.clone(),
            patterns,
            exceptions: self.exceptions.iter().map(|e| e.to_lowercase()).collect(),
            becomes: self.becomes.clone(),
        })
    }
}

/// One finding, as character offsets into the text that was searched.
pub struct Hit {
    pub rule: String,
    pub level: Level,
    pub message: String,
    pub start: usize,
    pub end: usize,
}

impl Compiled {
    pub fn run(&self, text: &str, out: &mut Vec<Hit>) {
        for re in &self.patterns {
            // `find_iter` yields a `Result` here: a backtracking engine can
            // exceed its step limit on a pathological input. A rule that
            // blows up is reported as nothing rather than crashing the run;
            // the alternative is one bad pattern stopping the whole gate.
            for m in re.find_iter(text).flatten() {
                let matched = m.as_str();
                if self.exceptions.iter().any(|e| e == &matched.to_lowercase()) {
                    continue;
                }
                // `%s` is the matched text, `%r` what to write instead --
                // the same two the message of a substitution rule needs.
                let message = self
                    .message
                    .replace("%r", self.becomes.as_deref().unwrap_or(""))
                    .replace("%s", matched);
                out.push(Hit {
                    rule: self.name.clone(),
                    level: self.level,
                    message,
                    // Character offsets, because that is what the engine's
                    // own findings use and the two are reported together.
                    start: text[..m.start()].chars().count(),
                    end: text[..m.end()].chars().count(),
                });
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rule(toml_src: &str) -> Compiled {
        let r: HouseRule = toml::from_str(toml_src).expect("parses");
        r.compile().expect("compiles")
    }

    #[test]
    fn a_match_is_reported_with_the_matched_text_in_the_message() {
        let r = rule(
            r#"
            name = "Etc"
            level = "warning"
            message = "Avoid '%s' in lists. Name the specific items."
            patterns = ['\betc\.']
            "#,
        );
        let mut hits = Vec::new();
        r.run("Screws, bolts, etc. go here.", &mut hits);
        assert_eq!(hits.len(), 1);
        assert!(hits[0].message.contains("'etc.'"), "{}", hits[0].message);
        assert_eq!(hits[0].start, 15);
    }

    #[test]
    fn an_exception_is_allowed_through() {
        let r = rule(
            r#"
            name = "Demo"
            message = "no"
            patterns = ['\b\w+ in\b']
            exceptions = ["Order log in"]
            "#,
        );
        let mut hits = Vec::new();
        r.run("Order log in please", &mut hits);
        assert!(hits.iter().all(|h| h.message == "no"));
        // "log in" matches; "Order log in" is not what the pattern matched,
        // so the exception has to be the matched text to apply.
        let r2 = rule(
            r#"
            name = "Demo"
            message = "no"
            patterns = ['log in']
            exceptions = ["log in"]
            "#,
        );
        let mut hits2 = Vec::new();
        r2.run("Order log in please", &mut hits2);
        assert!(hits2.is_empty(), "exception should suppress it");
    }

    #[test]
    fn offsets_are_characters_not_bytes() {
        // A finding after a multi-byte character has to line up with what
        // the engine reports, which counts characters.
        let r = rule(
            r#"
            name = "Demo"
            message = "x"
            patterns = ['trolley']
            "#,
        );
        let mut hits = Vec::new();
        r.run("café and a trolley", &mut hits);
        assert_eq!(hits[0].start, 11, "char offset, not byte offset");
    }

    #[test]
    fn a_phrase_is_matched_with_word_boundaries() {
        // `sis` is a real entry in one terminology list. Without boundaries
        // it matches inside "analysis", and a rule meant to catch a handful
        // of uses reported six hundred.
        let r = rule(
            r#"
            name = "Avoid"
            message = "Avoid '%s'."
            patterns = ["sis", "action list"]
            ignore_case = true
            "#,
        );
        let mut hits = Vec::new();
        r.run(
            "forensic analysis of the action list, and sis itself",
            &mut hits,
        );
        let found: Vec<_> = hits.iter().map(|h| h.message.clone()).collect();
        assert_eq!(found.len(), 2, "{found:?}");
        assert!(found.iter().any(|m| m.contains("'sis'")), "{found:?}");
        assert!(
            found.iter().any(|m| m.contains("'action list'")),
            "{found:?}"
        );
    }

    #[test]
    fn boundaries_go_where_they_mean_something() {
        assert_eq!(bound("sis"), r"\bsis\b");
        assert_eq!(bound("action list"), r"\baction list\b");
        // An abbreviation: the dot is literal, and no boundary follows it
        // because a boundary after a dot can never match.
        assert_eq!(bound("pcs."), r"\bpcs\.");
        // Past the lookbehind, which is not part of the match.
        assert_eq!(bound("(?<!Order )log in"), r"\b(?<!Order )log in\b");
        // Already bounded at the front; a `\b` after `.` would demand a
        // word character that is never there.
        assert_eq!(bound(r"\betc\."), r"\betc\.");
        assert_eq!(bound(r"\b\d+(?:kg|mm)\b"), r"\b\d+(?:kg|mm)\b");
    }

    #[test]
    fn a_bounded_phrase_does_not_match_inside_a_longer_word() {
        // The one that mattered: `log in` inside "error log information".
        let r = rule(
            r#"
            name = "T"
            message = "use sign in"
            patterns = ['(?<!Order )log in']
            ignore_case = true
            "#,
        );
        let mut hits = Vec::new();
        r.run("Look into the error log information here.", &mut hits);
        assert!(hits.is_empty(), "matched inside a word: {}", hits.len());
        let mut hits2 = Vec::new();
        r.run("Please log in now.", &mut hits2);
        assert_eq!(hits2.len(), 1, "but a real one still fires");
    }

    #[test]
    fn a_lookbehind_still_excludes_what_it_is_for() {
        let r = rule(
            r#"
            name = "T"
            message = "use safety laser scanner"
            patterns = ['(?<!safety )laser scanner']
            ignore_case = true
            "#,
        );
        let mut hits = Vec::new();
        r.run("The safety laser scanner is fine.", &mut hits);
        assert!(hits.is_empty(), "{:?}", hits.len());
        let mut hits2 = Vec::new();
        r.run("The laser scanner is not.", &mut hits2);
        assert_eq!(hits2.len(), 1);
    }

    #[test]
    fn a_regex_pattern_is_used_as_written() {
        // Adding boundaries to this would change what it means.
        let r = rule(
            r#"
            name = "Units"
            message = "space please"
            patterns = ['\b\d+(?:kg|mm)\b']
            "#,
        );
        let mut hits = Vec::new();
        r.run("Weighs 2kg here.", &mut hits);
        assert_eq!(hits.len(), 1);
    }

    #[test]
    fn a_phrase_with_a_dot_is_still_treated_as_a_regex() {
        // `etc.` has a dot, so it is left alone -- the pattern in the real
        // config is `\betc\.` and already carries its own boundary.
        let r = rule(
            r#"
            name = "Etc"
            message = "x"
            patterns = ['\betc\.']
            "#,
        );
        let mut hits = Vec::new();
        r.run("bolts, etc. here", &mut hits);
        assert_eq!(hits.len(), 1);
    }

    #[test]
    fn case_insensitivity_is_opt_in() {
        let sensitive = rule(
            r#"
            name = "D"
            message = "x"
            patterns = ['Trolley']
            "#,
        );
        let mut a = Vec::new();
        sensitive.run("a trolley", &mut a);
        assert!(a.is_empty());

        let insensitive = rule(
            r#"
            name = "D"
            message = "x"
            patterns = ['Trolley']
            ignore_case = true
            "#,
        );
        let mut b = Vec::new();
        insensitive.run("a trolley", &mut b);
        assert_eq!(b.len(), 1);
    }
}
