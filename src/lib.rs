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
use std::sync::{Arc, Mutex, PoisonError};

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
///
/// Linting is the slow part of a run, and nearly all of it is the engine.
/// [`Prose::lint`] does one document at a time; [`Prose::lint_many`] does a
/// corpus on several threads, each with an engine of its own -- see
/// `Shared` for why the engine is the one thing that cannot be shared.
pub struct Prose {
    shared: Shared,
    /// This thread's engine, or `None` when `[engine] enabled = false`.
    engine: Option<LintGroup>,
}

/// The part of a [`Prose`] that is plain data, and so can be read from every
/// thread at once.
///
/// The engine is the exception, for two reasons that each would be enough.
/// `organized_lints` takes `&mut self` -- it keeps a cache of what it found
/// in each chunk of text -- so one engine cannot serve two threads. And with
/// harper's `concurrent` feature off, which is what `default-features =
/// false` does along with dropping `thesaurus`, its pointers are `Rc` rather
/// than `Arc`, so an engine cannot leave the thread that built it either. A
/// thread that wants to lint therefore builds one of its own from
/// [`EngineRecipe`]. It is cheap: the expensive parts, the dictionary and the
/// models, are process-wide and built once, so a second engine is a few
/// milliseconds.
struct Shared {
    options: preprocess::Options,
    house: Vec<house::Compiled>,
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
    /// How to build another engine. `None` when the engine is switched off.
    recipe: Option<EngineRecipe>,
}

/// What it takes to build one more engine, once the first has shown that the
/// configuration is sound.
struct EngineRecipe {
    dialect: Dialect,
    /// The project's own corrections, and the path they came from for any
    /// message about them.
    pack: Option<(String, harper_core::weirpack::Weirpack)>,
    disable: Vec<String>,
    enable: Vec<String>,
}

impl EngineRecipe {
    fn build(&self, dictionary: &Arc<MergedDictionary>) -> Result<LintGroup, String> {
        // The project's own corrections first, then harper's: a corpus
        // that has decided "Allen key" is "hex key" should not also be
        // told something else about the same words.
        let mut group = match &self.pack {
            Some((rel, pack)) => {
                let mut group = pack.to_lint_group().map_err(|e| format!("{rel}: {e:?}"))?;
                group.merge_from(LintGroup::new_curated(dictionary.clone(), self.dialect));
                group
            }
            None => LintGroup::new_curated(dictionary.clone(), self.dialect),
        };
        // Mutate the group's own config rather than replacing it.
        // `with_lint_config` takes a whole `FlatConfig`, and a rule the
        // config does not mention counts as disabled -- so handing it a
        // fresh one switches off every rule in the group, including the
        // weirpack's, which `to_lint_group` had just enabled. The result
        // is a linter that loads a rule set, reports nothing, and exits
        // 0, which is the failure that looks most like success.
        for name in &self.disable {
            if !group.contains_key(name) {
                return Err(format!("[engine] disable: no rule named `{name}`"));
            }
            group.config.set_rule_enabled(name, false);
        }
        for name in &self.enable {
            if !group.contains_key(name) {
                return Err(format!("[engine] enable: no rule named `{name}`"));
            }
            group.config.set_rule_enabled(name, true);
        }
        Ok(group)
    }
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

        let (engine, recipe) = if config.engine.enabled {
            let dialect = dialect_of(&config.engine.dialect)?;
            let pack = match &config.engine.weirpack {
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
                    Some((rel.clone(), pack))
                }
                None => None,
            };
            let recipe = EngineRecipe {
                dialect,
                pack,
                disable: config.engine.disable.clone(),
                enable: config.engine.enable.clone(),
            };
            // Built here, and not left to the first thread that wants one, so
            // a bad config is an error before anything is linted.
            (Some(recipe.build(&dictionary)?), Some(recipe))
        } else {
            (None, None)
        };

        Ok(Self {
            shared: Shared {
                options: config.preprocess_options(),
                house,
                engine_level: config.engine.level,
                dictionary,
                recipe,
            },
            engine,
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
        self.shared.lint(self.engine.as_mut(), rel, source)
    }

    /// Lint many documents, on at most `jobs` threads.
    ///
    /// `items` pairs each document's corpus-relative path with whatever
    /// `read` needs to fetch its text. `read` runs on the thread that lints
    /// the document, so a corpus is streamed through rather than loaded
    /// whole, and it returns `None` for one that cannot be read -- which
    /// comes back as `None` in that position, because a page that has gone
    /// missing is not a page with nothing wrong.
    ///
    /// The results are in the order of `items`, whichever thread got to
    /// which and whenever it finished: a diff of the output is a diff of
    /// behaviour, and one that depends on timing is of neither.
    ///
    /// "At most" because a thread has to be worth having. Each one carries
    /// an engine, a copy of the chunker's model and of the spell checker's
    /// tables: 235 MB apiece by the end of a corpus-sized run, and a short
    /// list split between two of them has each re-deriving the chunks the
    /// other just cached. Fewer than `PAGES_PER_THREAD` pages to read does not
    /// repay that, so a short list gets fewer threads than it was offered.
    pub fn lint_many<T: Sync>(
        &mut self,
        items: &[(String, T)],
        jobs: usize,
        read: impl Fn(&T) -> Option<String> + Sync,
    ) -> Vec<Option<Vec<Diagnostic>>> {
        let Self { shared, engine } = self;
        let shared: &Shared = shared;
        let jobs = jobs
            .min(MAX_JOBS)
            .clamp(1, items.len().div_ceil(PAGES_PER_THREAD).max(1));

        // Handed out in runs, to whichever thread asks. A run and not one
        // document, because the engine remembers what it found in the last
        // thousand chunks of text it read, and neighbouring pages -- the same
        // directory, the same boilerplate -- share most of them: a thread
        // given every fourth page by turns re-derives what its neighbour just
        // did, and pays a quarter more CPU for it. Not a fixed share each
        // either, because pages differ in cost by a factor of fifty, and the
        // last thread to finish would set the time of the whole run. So the
        // runs shrink as the list runs out: long ones while there is plenty
        // left, down to single pages at the end. The position goes with the
        // result, which is what keeps the output in order.
        let next = Mutex::new(0);
        let claim = || {
            // A claim is a few dozen over a whole run and nothing inside the
            // lock can panic, so this is not a place to be clever.
            let mut next = next.lock().unwrap_or_else(PoisonError::into_inner);
            let from = *next;
            let left = items.len() - from;
            if left == 0 {
                return None;
            }
            let run = (left / (jobs * 4)).clamp(1, 64).min(left);
            *next += run;
            Some(from..from + run)
        };
        let work = |engine: &mut Option<LintGroup>| {
            let mut done = Vec::new();
            while let Some(run) = claim() {
                for (i, (rel, item)) in run.clone().zip(&items[run]) {
                    let found = read(item).map(|source| shared.lint(engine.as_mut(), rel, &source));
                    done.push((i, found));
                }
            }
            done
        };

        let finished = std::thread::scope(|scope| {
            // The helpers are spawned first so that they are building their
            // engines while this thread is getting on with its own. If one
            // cannot be spawned the others, and this thread, take its share:
            // `next` hands out each document exactly once however many
            // threads there turn out to be.
            let helpers: Vec<_> = (1..jobs)
                .filter_map(|n| {
                    std::thread::Builder::new()
                        .name(format!("markdoc-prose-{n}"))
                        .stack_size(WORKER_STACK)
                        .spawn_scoped(scope, || {
                            let mut engine = shared.recipe.as_ref().map(|recipe| {
                                // It built once, on the main thread, from the
                                // same recipe. Running without the engine
                                // instead would be a different answer, quietly.
                                recipe
                                    .build(&shared.dictionary)
                                    .expect("an engine that built once builds again")
                            });
                            work(&mut engine)
                        })
                        .ok()
                })
                .collect();
            let mut finished = work(engine);
            for helper in helpers {
                match helper.join() {
                    Ok(part) => finished.extend(part),
                    Err(panic) => std::panic::resume_unwind(panic),
                }
            }
            finished
        });

        let mut out: Vec<Option<Vec<Diagnostic>>> = Vec::new();
        out.resize_with(items.len(), || None);
        for (i, found) in finished {
            out[i] = found;
        }
        out
    }
}

/// The most threads this will use, whatever it is asked for.
///
/// Above four, the only thing that grows is memory. Measured over
/// a 5 251-file production corpus on a 4-core, 8-thread laptop: 52.5 s on one thread,
/// 31.4 s on two, 24.6 s on four, and 26.8 s on eight -- eight is *slower*
/// than four, for twice the resident memory, because each thread carries its
/// own engine and re-derives chunks its neighbours have already cached.
/// Four threads already burn 1.76x the CPU-seconds of a serial run to
/// deliver 2.13x the speed.
///
/// It is a ceiling and not just a default because the number a caller passes
/// is otherwise unbounded, and `lint_many`'s own clamp admits a great many:
/// its ceiling is pages / 16, which is 69 on this corpus, and `--jobs 64`
/// duly allocated 8.1 GB -- an out-of-memory kill on a 16 GB machine, to run
/// slower than four threads would have.
///
/// Enforced here, in the library, rather than in the binary that parses the
/// flag: it is a measured property of the engine, so every caller wants it,
/// and a ceiling that each caller has to remember to apply is one that some
/// caller will not.
pub const MAX_JOBS: usize = 4;

/// The fewest pages a thread is given before another is added.
///
/// Not the engine's build time, which is what this used to be reasoned from. A
/// second engine costs under 60 ms, not the second that reasoning assumed,
/// because the dictionary and the models are built once per process -- see
/// `Shared`; it is only when seven are built at once that they reach about
/// 300 ms each. A break-even derived from a whole second would put this
/// constant at 1, which is the wrong direction.
///
/// What a short list really pays is a cold cache on each new thread, and
/// 235 MB once that cache has filled. Measured over that corpus with this
/// constant lowered to 4, so that the short counts were granted the second
/// thread this value denies them, that trade loses: 5 pages 0.87x, 8 pages
/// 0.85x, 12 pages 1.05x, and 1.13x by the 17 pages the constant admits as it
/// stands. Sixteen is the conservative side of a break-even near twelve.
const PAGES_PER_THREAD: usize = 16;

/// The stack a worker thread gets. The main thread has 8 MiB on Linux and
/// the default for a spawned one is 2: a document the engine can lint on one
/// should not overflow on the other. Address space only; it is not committed
/// until it is used.
const WORKER_STACK: usize = 16 * 1024 * 1024;

impl Shared {
    /// Lint one document, with `engine` if there is one.
    fn lint(&self, engine: Option<&mut LintGroup>, rel: &str, source: &str) -> Vec<Diagnostic> {
        let processed = preprocess::preprocess(source, &self.options);
        let text = &processed.text;
        let index = CharIndex::new(text);
        let mut out = Vec::new();

        for rule in &self.house {
            let mut hits = Vec::new();
            rule.run(text, &mut hits);
            for hit in hits {
                out.push(
                    Diagnostic::new(rel, severity(hit.level), hit.rule, hit.message)
                        .at(Some(index.span(hit.start, hit.end))),
                );
            }
        }

        if let Some(engine) = engine {
            // Built with the project's dictionary, not the curated one --
            // see the note on `Shared::dictionary`.
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
                        Diagnostic::new(
                            rel,
                            severity(self.engine_level),
                            name.to_string(),
                            message,
                        )
                        .at(Some(index.span(lint.span.start, lint.span.end))),
                    );
                }
            }
        }

        out.sort_by(|a, b| {
            let key = |d: &Diagnostic| {
                d.span
                    .map(|s| (s.start_line, s.start_char))
                    .unwrap_or((0, 0))
            };
            key(a).cmp(&key(b)).then_with(|| a.rule.cmp(&b.rule))
        });
        // Two patterns of one rule can match the same text -- a terminology
        // list with both a term and one of its variants, say. One fault,
        // one line of output.
        out.dedup_by(|a, b| a.rule == b.rule && a.span == b.span && a.message == b.message);
        out
    }
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

    /// A house-rules-only `Prose`: nothing to load, so it is quick.
    fn etc_only() -> Prose {
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
        Prose::new(&config, Path::new(".")).unwrap()
    }

    fn texts(found: Vec<Option<Vec<Diagnostic>>>) -> Vec<Option<Vec<String>>> {
        found
            .iter()
            .map(|f| {
                f.as_ref()
                    .map(|d| d.iter().map(Diagnostic::to_text).collect())
            })
            .collect()
    }

    #[test]
    fn many_documents_come_back_in_the_order_they_went_in() {
        // Enough of them that there really are several threads, whatever the
        // machine, and three kinds so that a result in the wrong place shows.
        let items: Vec<(String, Option<String>)> = (0..100)
            .map(|i| {
                let text = match i % 3 {
                    0 => None,
                    1 => Some(format!("{}etc. here\n", "word ".repeat(i))),
                    _ => Some("Nothing to see.\n".to_string()),
                };
                (format!("page-{i:03}.md"), text)
            })
            .collect();
        let mut prose = etc_only();
        let serial = texts(prose.lint_many(&items, 1, |text| text.clone()));
        for jobs in [2, 4, 7, 64] {
            let parallel = texts(prose.lint_many(&items, jobs, |text| text.clone()));
            assert_eq!(parallel, serial, "with {jobs} threads");
        }
        // And the serial answer is the right one, not merely a consistent one.
        assert!(serial[0].is_none(), "a page that cannot be read is None");
        let found = serial[1].as_ref().unwrap();
        assert_eq!(found.len(), 1);
        assert!(found[0].starts_with("page-001.md:1:"), "{}", found[0]);
        assert!(serial[2].as_ref().unwrap().is_empty());
        assert!(serial[97].as_ref().unwrap()[0].starts_with("page-097.md:1:"));
    }

    #[test]
    fn every_thread_builds_an_engine_that_gives_the_same_answer() {
        // The real engine, which is what `lint_many` exists for: each thread
        // builds its own, and none of them may disagree with the first.
        let config: Config = toml::from_str("").unwrap();
        let mut prose = Prose::new(&config, Path::new(".")).unwrap();
        let samples = [
            "this sentence does not start with a capital letter.\n",
            "Please attach the the cable to the robot.\n",
            "An robot arrives at a station and it can not move.\n",
            "Nothing is wrong with this one.\n",
        ];
        // More than two threads' minimum, so that there are three.
        let items: Vec<(String, String)> = (0..40)
            .map(|i| (format!("p{i}.md"), samples[i % samples.len()].to_string()))
            .collect();
        let read = |text: &String| Some(text.clone());
        let serial = texts(prose.lint_many(&items, 1, read));
        let parallel = texts(prose.lint_many(&items, 3, read));
        assert!(
            serial.iter().flatten().any(|found| !found.is_empty()),
            "the engine found nothing at all, so this proves nothing"
        );
        assert_eq!(parallel, serial);
    }

    #[test]
    fn a_list_too_short_to_repay_a_thread_never_leaves_the_calling_thread() {
        // The clamp is the one piece of `lint_many` with no other witness:
        // nothing in a result says which thread produced it, so lowering
        // `PAGES_PER_THREAD` or dropping the `div_ceil` that applies it passes
        // every other test here. It is worth pinning because the short-list
        // case is the one a second thread loses: 0.87x at five pages and 0.85x
        // at eight, measured over a 5 251-file production corpus.
        //
        // The counts are literal rather than written in terms of the constant.
        // Phrased as `PAGES_PER_THREAD - 1` the list shrinks with the constant
        // it is meant to be watching, and the test went on passing with the
        // constant at 1.
        let calling = std::thread::current().id();
        let mut prose = etc_only();
        for pages in [0, 1, 2, 8, 16] {
            let items: Vec<(String, ())> = (0..pages).map(|i| (format!("p{i}.md"), ())).collect();
            let strayed = Mutex::new(Vec::new());
            let found = prose.lint_many(&items, 64, |_| {
                let here = std::thread::current().id();
                if here != calling {
                    strayed.lock().unwrap().push(here);
                }
                // A millisecond so that a helper, if the clamp ever permits
                // one, has time to claim a page. `etc_only` has no engine to
                // build, so a helper starts in microseconds and still lost the
                // race to the main thread every time: with the constant at 8
                // and no pause here, 16 pages were linted on 2 permitted
                // threads and all of them by the caller, and the test passed a
                // constant it should have caught. The pause cannot cause a
                // false failure -- at 16 pages and the constant at 16 there is
                // no second thread to find.
                std::thread::sleep(std::time::Duration::from_millis(1));
                Some("Clean.\n".to_string())
            });
            assert_eq!(found.len(), pages);
            let strayed = strayed.into_inner().unwrap();
            assert!(
                strayed.is_empty(),
                "{pages} page(s) asked for 64 threads and reached {strayed:?}"
            );
        }
    }

    #[test]
    fn no_caller_gets_more_threads_than_the_ceiling() {
        // `MAX_JOBS` is enforced in here rather than in the binary that parses
        // `--jobs`, so this is where it has to be pinned. Before it, 64 was
        // accepted on this corpus -- `lint_many`'s own ceiling is
        // pages / PAGES_PER_THREAD, which is 69 there -- and allocated 8.1 GB
        // to run slower than four threads would have.
        //
        // Enough pages that the other clamp is not the one doing the work: at
        // 64 * PAGES_PER_THREAD the list alone would permit all 64.
        let pages = 64 * PAGES_PER_THREAD;
        let items: Vec<(String, ())> = (0..pages).map(|i| (format!("p{i}.md"), ())).collect();
        let mut prose = etc_only();
        let seen: Mutex<Vec<std::thread::ThreadId>> = Mutex::new(Vec::new());

        let found = prose.lint_many(&items, 64, |_| {
            let here = std::thread::current().id();
            let mut seen = seen.lock().unwrap();
            if !seen.contains(&here) {
                seen.push(here);
            }
            drop(seen);
            // Long enough that a permitted helper has time to claim a page --
            // see the clamp test above, where without a pause every page was
            // linted by the caller and a wrong constant went unnoticed.
            std::thread::sleep(std::time::Duration::from_micros(200));
            Some("Clean.\n".to_string())
        });

        assert_eq!(found.len(), pages);
        let seen = seen.into_inner().unwrap();
        assert!(
            seen.len() <= MAX_JOBS,
            "asked for 64 threads and {} ran; the ceiling is {MAX_JOBS}",
            seen.len()
        );
        // Not vacuous: threads were permitted and did run, so the assertion
        // above is about the ceiling rather than about nothing having started.
        assert!(
            seen.len() > 1,
            "no helper ran at all, so the ceiling was not what stopped them"
        );
    }

    #[test]
    fn lint_many_agrees_with_lint() {
        let mut prose = etc_only();
        let items = vec![("a.md".to_string(), "bolts, etc. here\n".to_string())];
        let many = texts(prose.lint_many(&items, 4, |text| Some(text.clone())));
        let one: Vec<String> = prose
            .lint("a.md", &items[0].1)
            .iter()
            .map(Diagnostic::to_text)
            .collect();
        assert_eq!(many, vec![Some(one)]);
    }

    #[test]
    fn nothing_to_lint_is_nothing() {
        let mut prose = etc_only();
        let none: Vec<(String, String)> = Vec::new();
        assert!(
            prose
                .lint_many(&none, 8, |text| Some(text.clone()))
                .is_empty()
        );
    }

    #[test]
    fn an_unknown_config_key_is_an_error_rather_than_a_silent_no_op() {
        let err = toml::from_str::<Config>("[engine]\nenabledd = true\n").unwrap_err();
        assert!(err.to_string().contains("enabledd"), "{err}");
    }
}
