//! Prose linting for a Markdoc corpus, on [harper-core].
//!
//! ## What this replaces, and why it is not a port
//!
//! The usual tool here is Vale: a Go binary, a `.vale.ini`, and style rules
//! as YAML. It works, and the reason to move off it is not that it does not.
//! It is that a Markdoc corpus is not what it expects. Vale sees `{% table
//! column_weights="3 1" %}` as a sentence, and the configuration offered to
//! fix that — substitute the match with a filler character — breaks the
//! markdown it is substituting into. See [`preprocess`].
//!
//! Once the document has to be pre-processed anyway, the engine behind it
//! becomes a choice rather than a given, and harper-core is a better one for
//! this: it is a library rather than a subprocess, so there is no binary to
//! download and pin and no output to parse; it reports character spans into
//! the text it was handed, which a length-preserving pre-processor can map
//! back for free; and it carries its own rules, which is most of what a
//! borrowed third-party style pack was being used for.
//!
//! ## Rules come from two places, and the split is not arbitrary
//!
//! **Weir**, in a `.weirpack`, for anything that can say what to write
//! instead. "Allen key" becomes "hex key"; "log in" becomes "sign in". Weir
//! requires a `becomes`, which is the right requirement: a correction a tool
//! cannot apply is a correction a writer has to work out.
//!
//! **House rules**, in the project's TOML, for the ones that cannot. "Avoid
//! `etc.` in lists, name the items" has no single replacement, and inventing
//! one so the rule fits the DSL would produce confidently wrong suggestions.
//! See [`house`].
//!
//! Neither is built in. A corpus states both, next to everything else it
//! states about itself.

pub mod house;
pub mod preprocess;

use harper_core::Dialect;
use harper_core::DictWordMetadata;
use harper_core::Document;
use harper_core::linting::LintGroup;
use harper_core::spell::{FstDictionary, MergedDictionary, MutableDictionary};
use markdoc_lint::{Diagnostic, Severity, Span as OutSpan};
use serde::Deserialize;
use std::path::Path;
use std::sync::Arc;

/// The file a project puts at its content root.
pub const CONFIG_FILENAME: &str = "markdoc-prose.toml";

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct Config {
    pub engine: EngineConfig,
    pub preprocess: PreprocessConfig,
    /// The patterns that cannot say what to write instead.
    #[serde(rename = "rule")]
    pub rules: Vec<house::HouseRule>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct EngineConfig {
    /// Whether harper's own rules run at all. A corpus migrating from
    /// another tool will want them off for the first pass, then on.
    pub enabled: bool,
    /// Rules of harper's to switch off, by name.
    pub disable: Vec<String>,
    /// Rules of harper's to switch on that are off by default.
    pub enable: Vec<String>,
    /// A `.weirpack` of this project's own corrections, relative to the
    /// config file.
    pub weirpack: Option<String>,
    /// What a finding from the engine counts as. House rules carry their own
    /// level; harper does not make that distinction, so it is set once here.
    pub level: house::Level,
    /// Which English. Affects the dictionary and several rules.
    pub dialect: String,
    /// Words this corpus uses that no general dictionary has: product
    /// names, protocols, units.
    ///
    /// A word here is exempt from *spelling* only. That is deliberate, and
    /// it is where the tool this replaces goes wrong: its vocabulary file
    /// exempts a term from every rule, so adding `Allen` so the spell
    /// checker would stop complaining also switched off the error-level rule
    /// that turns "Allen key" into "hex key". Nobody noticed for as long as
    /// the file existed.
    pub dictionary: Vec<String>,
    /// A file of the same, one word per line, `#` for a comment.
    ///
    /// A few hundred product names and part codes do not belong inline in a
    /// config file, and a plain list is reviewable in a way a TOML array is
    /// not: a diff of it reads as "these words were added".
    pub dictionary_file: Option<String>,
}

impl Default for EngineConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            disable: Vec::new(),
            enable: Vec::new(),
            weirpack: None,
            level: house::Level::Warning,
            dialect: "American".to_string(),
            dictionary: Vec::new(),
            dictionary_file: None,
        }
    }
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct PreprocessConfig {
    /// Tags that render as words wherever they appear.
    pub word_tags: Option<Vec<String>>,
    /// Whether `**bold**` is treated as prose. See
    /// [`preprocess::Options::skip_bold`].
    pub skip_bold: bool,
}

impl Config {
    pub fn load(dir: &Path) -> Result<Self, String> {
        let path = dir.join(CONFIG_FILENAME);
        match std::fs::read_to_string(&path) {
            Ok(text) => toml::from_str(&text).map_err(|e| format!("{}: {e}", path.display())),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Self::default()),
            Err(e) => Err(format!("{}: {e}", path.display())),
        }
    }

    pub fn preprocess_options(&self) -> preprocess::Options {
        let mut o = preprocess::Options::default();
        if let Some(tags) = &self.preprocess.word_tags {
            o.word_tags = tags.clone();
        }
        o.skip_bold = self.preprocess.skip_bold;
        o
    }
}

/// Everything needed to lint, built once and reused across files.
pub struct Prose {
    options: preprocess::Options,
    house: Vec<house::Compiled>,
    engine: Option<LintGroup>,
    engine_level: house::Level,
    /// The same dictionary the rules use.
    ///
    /// Held because the *document* has to be built with it too, not only the
    /// linter. The tokenizer attaches metadata to each word from whatever
    /// dictionary it was given, and the spell checker skips a word only when
    /// that metadata is present *and* the dictionary contains it. Build the
    /// document with the curated dictionary and the project's own words have
    /// no metadata, so every one of them is reported however many times it
    /// appears in the dictionary the linter was handed. `Modbus` passed and
    /// `M8` did not, which is a confusing way to find this out.
    dictionary: Arc<MergedDictionary>,
}

impl Prose {
    pub fn new(config: &Config, root: &Path) -> Result<Self, String> {
        let mut house = Vec::with_capacity(config.rules.len());
        for rule in &config.rules {
            house.push(rule.compile()?);
        }

        let mut words = config.engine.dictionary.clone();
        {
            if let Some(rel) = &config.engine.dictionary_file {
                let path = root.join(rel);
                let text = std::fs::read_to_string(&path)
                    .map_err(|e| format!("{}: {e}", path.display()))?;
                words.extend(
                    text.lines()
                        .map(str::trim)
                        .filter(|l| !l.is_empty() && !l.starts_with('#'))
                        .map(str::to_string),
                );
            }
        }
        let dictionary = project_dictionary(&words);

        let engine = if config.engine.enabled {
            let dialect = dialect_of(&config.engine.dialect)?;
            // The project's own corrections first, then harper's: a corpus
            // that has decided "Allen key" is "hex key" should not also be
            // told something else about the same words.
            let group = match &config.engine.weirpack {
                Some(rel) => {
                    let pack = load_pack(&root.join(rel))?;
                    // Compile each rule alone first. `to_lint_group` reports
                    // the first failure without saying which rule it came
                    // from, so a pack of two hundred produced
                    // `Weir(UnsupportedToken("12"))` and nothing to act on.
                    // (It was `12:00 noon`: a Weir expression holds words,
                    // punctuation and tags, and a numeral is none of those.)
                    let mut names: Vec<&String> = pack.rules.keys().collect();
                    names.sort();
                    for name in names {
                        let mut probe = harper_core::weirpack::Weirpack::new(
                            harper_core::weirpack::WeirpackManifest::new(),
                        );
                        probe.add_rule(name.clone(), pack.rules[name].clone());
                        if let Err(e) = probe.to_lint_group() {
                            return Err(format!("{rel}/{name}.weir: {e:?}"));
                        }
                    }
                    let mut group = pack.to_lint_group().map_err(|e| format!("{rel}: {e:?}"))?;
                    group.merge_from(LintGroup::new_curated(dictionary.clone(), dialect));
                    group
                }
                None => LintGroup::new_curated(dictionary.clone(), dialect),
            };
            // Mutate the group's own config rather than replacing it.
            // `with_lint_config` takes a whole `FlatConfig`, and a rule the
            // config does not mention counts as disabled -- so handing it a
            // fresh one switches off every rule in the group, including the
            // weirpack's, which `to_lint_group` had just enabled. The result
            // is a linter that loads a rule set, reports nothing, and exits
            // 0, which is the failure that looks most like success.
            let mut group = group;
            for name in &config.engine.disable {
                if !group.contains_key(name) {
                    return Err(format!("[engine] disable: no rule named `{name}`"));
                }
                group.config.set_rule_enabled(name, false);
            }
            for name in &config.engine.enable {
                if !group.contains_key(name) {
                    return Err(format!("[engine] enable: no rule named `{name}`"));
                }
                group.config.set_rule_enabled(name, true);
            }
            Some(group)
        } else {
            None
        };

        Ok(Self {
            options: config.preprocess_options(),
            house,
            engine,
            engine_level: config.engine.level,
            dictionary,
        })
    }

    /// Every rule the engine has, and whether it is switched on.
    ///
    /// Worth having as a command rather than as documentation: the set comes
    /// from the engine's version, and a list in a README is a list that goes
    /// stale without anybody finding out. It is also the only way to answer
    /// "is this already covered?" before writing a rule.
    pub fn rules(&self) -> Vec<(String, bool)> {
        let Some(engine) = &self.engine else {
            return Vec::new();
        };
        let mut out: Vec<(String, bool)> = engine
            .iter_keys()
            .map(|k| (k.to_string(), engine.config.is_rule_enabled(k)))
            .collect();
        out.sort();
        out
    }

    /// Lint one document.
    ///
    /// `rel` is the corpus-relative path, which appears in every diagnostic.
    pub fn lint(&mut self, rel: &str, source: &str) -> Vec<Diagnostic> {
        let processed = preprocess::preprocess(source, &self.options);
        let text = &processed.text;
        let index = CharIndex::new(text);
        let mut out = Vec::new();

        for rule in &self.house {
            let mut hits = Vec::new();
            rule.run(text, &mut hits);
            for hit in hits {
                out.push(
                    Diagnostic::new(rel, severity(hit.level), leak(&hit.rule), hit.message)
                        .at(Some(index.span(hit.start, hit.end))),
                );
            }
        }

        if let Some(engine) = &mut self.engine {
            // Built with the project's dictionary, not the curated one --
            // see the note on `Prose::dictionary`.
            let document = Document::new_markdown_default(text, self.dictionary.as_ref());
            // `organized_lints` rather than `lint`, because it is keyed by
            // the rule's name. `lint` flattens that away and leaves only the
            // category, so every terminology correction would be reported as
            // "Usage" -- useless to grep for and useless to switch off.
            for (name, lints) in engine.organized_lints(&document) {
                for lint in lints {
                    // A finding inside a stand-in is about the stand-in. The
                    // placeholder word is nobody's spelling mistake, and a
                    // blanked tag is nobody's prose.
                    if processed.is_substituted(lint.span.start, lint.span.end) {
                        continue;
                    }
                    // Do not tell somebody to change `STO` into `STO`.
                    //
                    // A Weir expression matches words case-insensitively,
                    // which is right for catching `sto` and `Sto` -- and
                    // means it also matches `STO`, the form the rule is
                    // steering towards. `becomes` can be a list for exactly
                    // that reason ("Safe Torque Off" or "STO"), so a match
                    // that already is one of the options is not a finding.
                    // 50 of this corpus's warnings were that.
                    if already_correct(&lint, &document) {
                        continue;
                    }
                    let message = with_suggestion(&lint, &document);
                    out.push(
                        Diagnostic::new(rel, severity(self.engine_level), leak(&name), message)
                            .at(Some(index.span(lint.span.start, lint.span.end))),
                    );
                }
            }
        }

        out.sort_by_key(|d| {
            let s = d
                .span
                .map(|s| (s.start_line, s.start_char))
                .unwrap_or((0, 0));
            (s.0, s.1, d.rule)
        });
        // Two patterns of one rule can match the same text -- a terminology
        // list with both a term and one of its variants, say. One fault,
        // one line of output.
        out.dedup_by(|a, b| a.rule == b.rule && a.span == b.span && a.message == b.message);
        out
    }
}

/// A rule name has to outlive the loop that produced it, and there are a few
/// dozen of them for the life of the process.
fn leak(name: &str) -> &'static str {
    Box::leak(name.to_string().into_boxed_str())
}

fn severity(level: house::Level) -> Severity {
    match level {
        house::Level::Error => Severity::Error,
        house::Level::Warning => Severity::Warning,
        house::Level::Hint => Severity::Hint,
    }
}

/// Is the matched text already one of the things the rule suggests?
fn already_correct(lint: &harper_core::linting::Lint, document: &Document) -> bool {
    use harper_core::linting::Suggestion;
    let found: String = document.get_span_content(&lint.span).iter().collect();
    let found = found.trim();
    if found.is_empty() {
        return false;
    }
    lint.suggestions.iter().any(|s| match s {
        // Case-sensitive: `sto` and `STO` are different answers to this
        // question, which is the whole point.
        Suggestion::ReplaceWith(chars) => chars.iter().collect::<String>() == found,
        _ => false,
    })
}

/// The engine's message, with what it suggests writing instead.
///
/// Appended only when the message does not already say it. A rule written
/// for this corpus carries its own wording -- "Use hex key instead of 'Allen
/// key'." -- and appending the same thing again produces a diagnostic that
/// says it twice in two different styles.
fn with_suggestion(lint: &harper_core::linting::Lint, document: &Document) -> String {
    use harper_core::linting::Suggestion;
    let found: String = document.get_span_content(&lint.span).iter().collect();
    let found = found.trim();
    let replacement = lint.suggestions.iter().find_map(|s| match s {
        Suggestion::ReplaceWith(chars) => Some(chars.iter().collect::<String>()),
        _ => None,
    });
    if found.is_empty() {
        return lint.message.clone();
    }
    let says_found = lint.message.to_lowercase().contains(&found.to_lowercase());
    let says_fix = replacement
        .as_ref()
        .is_some_and(|r| lint.message.to_lowercase().contains(&r.to_lowercase()));

    match (&replacement, says_found, says_fix) {
        // The rule wrote its own message and named the text it matched.
        (_, true, _) => lint.message.clone(),
        // "Did you mean `preferred`?" names the fix and not the fault, which
        // is the half a reader cannot supply. A diagnostic has to say what
        // it is about.
        (_, false, true) => format!("{} Found `{found}`.", lint.message),
        (Some(r), false, false) => format!("{} Use `{r}` instead of `{found}`.", lint.message),
        (None, false, false) => format!("{} Found `{found}`.", lint.message),
    }
}

/// Character offset -> line and column, built once per document.
struct CharIndex {
    /// Character offset of the start of each line.
    starts: Vec<usize>,
}

impl CharIndex {
    fn new(text: &str) -> Self {
        let mut starts = vec![0usize];
        for (i, c) in text.chars().enumerate() {
            if c == '\n' {
                starts.push(i + 1);
            }
        }
        Self { starts }
    }

    fn line_col(&self, offset: usize) -> (usize, usize) {
        let line = match self.starts.binary_search(&offset) {
            Ok(i) => i,
            Err(i) => i - 1,
        };
        (line + 1, offset - self.starts[line])
    }

    fn span(&self, start: usize, end: usize) -> OutSpan {
        let (sl, sc) = self.line_col(start);
        let (el, ec) = self.line_col(end);
        OutSpan {
            start_line: sl,
            start_char: sc,
            end_line: el,
            end_char: ec,
            start_offset: start,
            end_offset: end,
        }
    }
}

/// Load a rule set, from a `.weirpack` file or from a directory of `.weir`
/// files.
///
/// A directory is what a repository wants. `.weirpack` is a zip, and a zip
/// in version control is a file nobody can review: the diff of a changed
/// rule is "binary files differ". The packed form is for distributing a rule
/// set to other people, which is a different job from keeping one.
///
/// Each file's stem is the rule name, which is also what `[engine] disable`
/// refers to.
pub fn load_pack(path: &Path) -> Result<harper_core::weirpack::Weirpack, String> {
    use harper_core::weirpack::{Weirpack, WeirpackManifest};

    if path.is_file() {
        let bytes = std::fs::read(path).map_err(|e| format!("{}: {e}", path.display()))?;
        return Weirpack::from_bytes(&bytes).map_err(|e| format!("{}: {e:?}", path.display()));
    }
    if !path.is_dir() {
        return Err(format!("{}: no such file or directory", path.display()));
    }

    let manifest_path = path.join("manifest.json");
    let file = std::fs::File::open(&manifest_path).map_err(|e| {
        if e.kind() == std::io::ErrorKind::NotFound {
            format!("{}: a rule directory needs one", manifest_path.display())
        } else {
            format!("{}: {e}", manifest_path.display())
        }
    })?;
    let manifest = WeirpackManifest::from_reader(file)
        .map_err(|e| format!("{}: {e:?}", manifest_path.display()))?;

    let mut pack = Weirpack::new(manifest);
    let mut entries: Vec<_> = std::fs::read_dir(path)
        .map_err(|e| format!("{}: {e}", path.display()))?
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().and_then(|s| s.to_str()) == Some("weir"))
        .collect();
    // Sorted, so a rule set loads the same way twice and a diff of output is
    // a diff of behaviour.
    entries.sort();
    if entries.is_empty() {
        return Err(format!("{}: no .weir files", path.display()));
    }
    for file in entries {
        let name = file
            .file_stem()
            .and_then(|s| s.to_str())
            .ok_or_else(|| format!("{}: unusable file name", file.display()))?
            .to_string();
        let source =
            std::fs::read_to_string(&file).map_err(|e| format!("{}: {e}", file.display()))?;
        pack.add_rule(name, source);
    }
    Ok(pack)
}

/// The curated dictionary plus this corpus's own words.
///
/// Always merged, even with no extra words, so there is one type here and
/// no branch that only runs for some configurations.
fn project_dictionary(words: &[String]) -> Arc<MergedDictionary> {
    let mut merged = MergedDictionary::new();
    merged.add_dictionary(FstDictionary::curated());
    if !words.is_empty() {
        let mut extra = MutableDictionary::new();
        for word in words {
            extra.append_word_str(word, DictWordMetadata::default());
        }
        merged.add_dictionary(Arc::new(extra));
    }
    Arc::new(merged)
}

fn dialect_of(name: &str) -> Result<Dialect, String> {
    match name.to_ascii_lowercase().as_str() {
        "american" => Ok(Dialect::American),
        "british" => Ok(Dialect::British),
        "canadian" => Ok(Dialect::Canadian),
        "australian" => Ok(Dialect::Australian),
        "indian" => Ok(Dialect::Indian),
        other => Err(format!(
            "[engine] dialect: `{other}` is not one of American, British, Canadian, \
             Australian, Indian"
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_line_and_column_are_counted_in_characters() {
        let index = CharIndex::new("café x\nsecond line\n");
        assert_eq!(index.line_col(0), (1, 0));
        // After the accented character, which is two bytes and one char.
        assert_eq!(index.line_col(5), (1, 5));
        assert_eq!(index.line_col(7), (2, 0));
    }

    #[test]
    fn a_house_rule_reports_against_the_real_position() {
        let config: Config = toml::from_str(
            r#"
            [engine]
            enabled = false

            [[rule]]
            name = "Etc"
            level = "warning"
            message = "Avoid '%s' in lists."
            patterns = ['\betc\.']
            "#,
        )
        .unwrap();
        let mut prose = Prose::new(&config, Path::new(".")).unwrap();
        // The tag before it is blanked, so the column is the real one.
        let found = prose.lint("a.md", "---\nt: x\n---\n\n{% $p %} bolts, etc. here.\n");
        assert_eq!(found.len(), 1, "{found:?}");
        let span = found[0].span.unwrap();
        assert_eq!(span.start_line, 5);
        assert_eq!(span.start_char, 16, "the column in the real file");
    }

    #[test]
    fn frontmatter_is_not_prose() {
        let config: Config = toml::from_str(
            r#"
            [engine]
            enabled = false

            [[rule]]
            name = "Etc"
            message = "no"
            patterns = ['\betc\.']
            "#,
        )
        .unwrap();
        let mut prose = Prose::new(&config, Path::new(".")).unwrap();
        let found = prose.lint("a.md", "---\ntitle: \"a, b, etc.\"\n---\n\nClean.\n");
        assert!(found.is_empty(), "{found:?}");
    }

    #[test]
    fn an_unknown_config_key_is_an_error_rather_than_a_silent_no_op() {
        let err = toml::from_str::<Config>("[engine]\nenabledd = true\n").unwrap_err();
        assert!(err.to_string().contains("enabledd"), "{err}");
    }
}
