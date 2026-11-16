# markdoc-prose

Prose linting for a Markdoc corpus, on
[harper-core](https://writewithharper.com).

`markdoc-lint` checks what a document *is*. This checks what it *says*.

## Why not Vale

The reason to move is not that Vale does not work. It is that a Markdoc
corpus is not what it expects: Vale reads `{% table column_weights="3 1" %}`
as a sentence, and the configuration offered to fix that — substitute the
match with a filler character — *breaks the markdown it substitutes into*.
With `*` as the filler, a line holding a tag and an italic phrase has the
rest of its content pulled into an emphasis span, which the same config then
skips. That was hiding real findings in the corpus this was written for.

Once the document has to be pre-processed anyway, the engine behind it
becomes a choice rather than a given, and harper-core is a better one here:
it is a library rather than a subprocess, so there is no binary to download
and pin and no output to parse; it reports character spans into the text it
was handed, which a length-preserving pre-processor maps back for free; and
it carries its own rules, which is most of what a borrowed third-party style
pack was being used for.

## The pre-processor, and its one contract

The output has exactly as many **characters** as the input, and a newline
exactly where the input had one. A finding at character 1 234 of the
processed text is at character 1 234 of the real file, so nothing has to be
mapped back — which matters because a position that is nearly right is worse
than none.

What each thing becomes depends on what it *renders* as:

| in the source | what the linter sees | why |
|---|---|---|
| `{% $product %}` | `Mdocxxxxxxxxxx` | it renders as a word; blanking it leaves "The  is heavy." |
| `{% tagref "safety" /%}`, a `word_tags` tag | the same stand-in | it renders as the title of its target, so a sentence built around it still has a subject |
| `{% table %}` alone on its line | spaces | it renders as structure |
| `{% /notice %}` | spaces | a closing tag renders as nothing |
| a tag broken across lines | spaces | a word cannot contain a newline without moving every position after it |
| frontmatter | spaces | it is metadata |
| `` `code` ``, link destinations | spaces, delimiters kept | not prose |
| a ```` ``` ```` fenced block | spaces, fence lines included | not prose |
| `**bold**` | spaces, optionally | in a software manual it is usually a UI label |
| `\{% table %}` | itself | an escaped brace is a literal, not a tag |

Three of those are less obvious than they look.

**Structural or not is decided by position, not by a list of tag names.** A
tag alone on its line is structure; a tag inside a sentence is read as part
of it. A list marker still counts as alone — `* {% partial file="a.md" /%}`
is a table cell, not a sentence. `$` and the configured `word_tags` are
words wherever they sit, because a cross-reference is one even on a line of
its own. Keeping a name list in step with a schema is work that a position
test does not need.

**The stand-in is padded only where it would otherwise merge.** Tags get
written hard against words — `Checklist{% tag "received" /%}` — and without
a gap the stand-in joins the word into a single 60-character non-word, which
a spell checker reports over a span that covers real text and so cannot
simply be dropped. Padding unconditionally is no better: it puts two spaces
where the author wrote one, which a spacing rule then reports 3 873 times
over one corpus. So a space goes in only next to a word character — and
*emitted* output is what counts, since `{% a %}{% b %}` has a `}` between
the two tags in the source but two stand-ins flush against each other in the
output.

**Delimiters survive.** Only what a pair of backticks or asterisks wraps is
blanked; the markers stay. Dropping a closing backtick would leave the
markdown parser with an unclosed span and change how the rest of the
document reads, which is the whole failure this replaces.

Frontmatter detection is deliberately tolerant of what real corpora hold: a
byte-order mark, trailing whitespace on a delimiter, CRLF endings. An
opening `---` with no closing one is not frontmatter. A rule anchored too
tightly here does not fail loudly — it just stops hiding one file's
metadata, and the rules start reporting fields nobody wrote as prose.

## Rules come from two places, and the split is not arbitrary

**Weir**, for anything that can say what to write instead. "Allen key"
becomes "hex key"; "log in" becomes "sign in". Weir requires a `becomes`,
which is the right requirement — a correction a tool cannot apply is one a
writer has to work out — and it can do what a regex cannot:

```
expr phrase [(log in), (log on)]
expr main <((@phrase) ![DET, NOUN, PROPN]), (@phrase)>

let description "Prefer 'sign in' to 'log in' as a verb."
let message "Use 'sign in' instead of 'log in'."
let becomes "sign in"

test  "Please log in to the interface." "Please sign in to the interface."
allows "Open the error log in Foxglove Studio."
```

"log in" is a verb in the first sentence and a noun followed by a
preposition in the second. A part of speech separates them; a regex cannot,
and the one this replaced reported four pages of correct English as
terminology errors.

**House rules**, in `markdoc-prose.toml`, for the ones that cannot say what
to write instead. "Avoid `etc.` in lists, name the items" has no single
replacement, and inventing one so the rule fits the DSL would produce
confidently wrong suggestions. A few rules that *can* name a correction
still land here, because the thing they match is a shape rather than a
phrase: `(?<!safety )laser scanner` is "a laser scanner nobody has called a
safety one yet", which no sequence of words expresses. Those keep their
suggestion via `becomes` rather than losing it to the file they are in.

Neither set is built in. A corpus states both, next to everything else it
states about itself.

### A rule directory beats a `.weirpack`

`weirpack` may name either. A `.weirpack` is a zip, and a zip in version
control is a file nobody can review — the diff of a changed rule is "binary
files differ". So a repository keeps a directory of `.weir` files plus a
`manifest.json`; the packed form is for *distributing* a rule set, which is
a different job from keeping one.

The manifest is harper's: `author`, `version`, `description` and `license`,
all strings, all required. A directory without one, or without a single
`.weir` file, is an error rather than an empty rule set.

Each file's stem is the rule name, which is also what `[engine] disable`
refers to. Files load in sorted order, so a rule set loads the same way
twice and a diff of output is a diff of behaviour.

Rules are compiled one at a time before the pack is built, which costs a
little and buys the only thing that matters when a pack is wrong: the
filename. `to_lint_group` reports the first failure without saying where it
came from, so a pack of two hundred produced `Weir(UnsupportedToken("12"))`
and nothing to act on. (It was `12:00 noon`. A Weir expression holds words,
punctuation and tags, and a numeral is none of those.) A file missing one of
the four things Weir requires of every rule — `main`, `description`,
`message`, `becomes` — fails the same way, against its name:
`rules/sign-in.weir: Weir(ExpectedVariableUndefined)`.

### Phrase or regex, decided by the pattern

A house-rule pattern with no regular-expression metacharacters is a
**phrase**, and gets word boundaries put around it. Without them `sis` — a
real entry in one terminology list — matches inside "analysis", and a rule
meant to catch a handful of uses reports six hundred. Anything that looks
like a regex is used as written, because adding boundaries would change what
it means.

A bare `.` does not make a pattern a regex. In a terminology list a dot is
an abbreviation — `pcs.`, `etc.` — and treating it as "any character" is how
such an entry quietly stops working: as a regex, `pcs.` matches "pcs" plus
anything, and a word boundary after an any-character match lands between two
non-word characters and fails. The entry then matches nothing at all, which
looks exactly like a corpus with no violations. `pcs.` is in one real list
and had never once fired. A backslash is the tell: anything genuinely
written as a regex has one.

Boundaries go where they mean something, judged at each end separately,
because `\b` only says anything next to a word character:

| pattern | compiled as |
|---|---|
| `sis` | `\bsis\b` |
| `action list` | `\baction list\b` |
| `pcs.` | `\bpcs\.` — a boundary after the dot could never match |
| `(?<!Order )log in` | `\b(?<!Order )log in\b` — past the lookbehind |
| `\betc\.` | unchanged; it carries its own |
| `\b\d+(?:kg\|mm)\b` | unchanged; already bounded |

The engine is `fancy-regex`, not `regex`, for one reason: lookaround. Three
of the patterns a real style guide needs are of the form "this, but not when
it is already that" — `(?<!safety )laser scanner`, or
`(?<!\d{4}-)\b\d{1,3}-\d{1,3}\b(?!-\d{2})` for a numeric range that is not
part of a date. Rust's `regex` refuses lookaround by design, to guarantee
linear time. `fancy-regex` backtracks for exactly these constructs, which is
the trade every other prose linter has already made; Vale carries a
.NET-compatible engine for the same reason. A pattern that exceeds the
backtracking step limit on pathological input is reported as nothing rather
than taken as a crash — one bad pattern should not stop the whole gate.

## Configuration

`markdoc-prose.toml` at the content root. There is no mandatory key; with no
file at all the engine's own rules run and nothing else. An unknown key at
the top level, in `[engine]` or in `[preprocess]` is an error rather than a
silent no-op, because a typo in a config that still parses is a rule that
stops firing without anyone noticing. A `[[rule]]` table is the exception:
it does not check its keys, so `ignorecase = true` in one is ignored and the
rule matches case-sensitively. Its `name`, `message` and `patterns` are
required.

```toml
[engine]
enabled = true                 # false for the first pass of a migration
dialect = "American"           # or British, Canadian, Australian, Indian
level = "warning"              # what a finding from the engine counts as
weirpack = ".tooling/prose/rules"   # a directory, or a .weirpack file
# Rule names as `--rules` prints them; an unknown one is an error.
disable = ["SpellCheck"]
enable  = []
dictionary = ["Modbus", "M8", "STO"]
dictionary_file = ".tooling/prose/vocabulary.txt"

[preprocess]
word_tags = ["tagref", "productname"]   # tags that render as a word anywhere
skip_bold = true                        # bold is a UI label, not prose

# A pattern that cannot say what to write instead.
[[rule]]
name = "Etc"
level = "warning"                       # error | warning | hint
message = "Avoid '%s' in lists. Name the specific items."
patterns = ['\betc\.']

# One that can, but matches a shape rather than a phrase.
[[rule]]
name = "SafetyLaserScanner"
message = "%s is a safety device. Write %r."
patterns = ['(?<!safety )laser scanner']
becomes = "safety laser scanner"
ignore_case = true

# An exception is a whole match: these two are product names.
[[rule]]
name = "UnitSpacing"
level = "hint"
message = "Put a space between the number and the unit: '%s'."
patterns = ['\b\d+(?:kg|mm|V)\b']
exceptions = ["24V", "48V"]
```

Every key, and what it is when left out:

| key | type | default | |
|---|---|---|---|
| `engine.enabled` | bool | `true` | whether harper's rules, and the weirpack's, run at all |
| `engine.dialect` | string | `"American"` | `American`, `British`, `Canadian`, `Australian` or `Indian`, in any case |
| `engine.level` | `"error"`, `"warning"` or `"hint"` | `"warning"` | what every finding from the engine counts as |
| `engine.weirpack` | path | none | a directory of `.weir` files or a `.weirpack`, relative to the root |
| `engine.disable` | rule names | `[]` | rules to switch off |
| `engine.enable` | rule names | `[]` | rules to switch on that are off by default |
| `engine.dictionary` | words | `[]` | exempt from spelling, and only spelling |
| `engine.dictionary_file` | path | none | more of the same, one word per line, relative to the root |
| `preprocess.word_tags` | tag names | `["tagref"]` | tags that render as a word wherever they sit; a list here replaces the default rather than adding to it |
| `preprocess.skip_bold` | bool | `false` | blank what `**…**` wraps |
| `rule.name` | string | required | what findings are reported under |
| `rule.message` | string | required | what the writer is told |
| `rule.patterns` | strings | required | phrases or regular expressions; any match is a finding |
| `rule.level` | `"error"`, `"warning"` or `"hint"` | `"warning"` | what a finding counts as |
| `rule.ignore_case` | bool | `false` | match the patterns regardless of case |
| `rule.exceptions` | strings | `[]` | whole matches to let through, compared ignoring case |
| `rule.becomes` | string | none | what to write instead |

`rule.*` are the keys of each `[[rule]]` table. A level is written in lower
case; `"Error"` is an error.

`%s` in a message is the matched text and `%r` is `becomes`, or nothing
when there is none. An exception is compared with the whole of what a
pattern matched, ignoring case, and not with the text around it — so it can
only let through something its pattern matches on its own. `exceptions =
["Laser Scanner Pro"]` on the second rule above would never apply: what that
pattern matches in "Laser Scanner Pro" is `Laser Scanner`.

A name in `disable` or `enable` that no rule answers to is an error, not a
no-op — that is the other half of the same bargain. Those names are checked
when the engine is built, so while `enabled = false` a wrong name in
`disable` or `enable`, an unknown `dialect` or a missing `weirpack` is not
an error: none of the four is looked at until the engine is switched on.

`dictionary_file` is one word per line; blank lines and lines starting with
`#` are skipped. It is read whether the engine is on or not, and a file that
is not there is an error. A few hundred product names and part codes do not
belong inline in a config file, and a plain list is reviewable in a way a
TOML array is not: a diff of it reads as "these words were added".

### The dictionary exempts spelling, and only spelling

That is deliberate, and it is where the tool this replaces goes wrong. Its
vocabulary file exempts a term from *every* rule, so adding `Allen` to stop
the spell checker complaining also switched off the error-level rule that
turns "Allen key" into "hex key". Nobody noticed for as long as the file
existed.

The matching subtlety is worth knowing if you touch this code: the document
has to be built with the project's dictionary, not just the linter. The
tokenizer attaches metadata to each word from whatever dictionary it was
given, and the spell checker skips a word only when that metadata is present
*and* the dictionary holds it. Build the document with the curated
dictionary and the project's own words arrive without metadata, so every one
of them is reported however many times it appears in the dictionary the
linter was handed. `Modbus` passed and `M8` did not, which is a confusing
way to find this out.

## What it deliberately does not report

| dropped | why |
|---|---|
| an engine finding — harper's or a Weir rule's — that begins and ends inside text the pre-processor replaced (a stand-in, a blanked tag, frontmatter, code), with nothing between but more of it and whitespace | the placeholder word is nobody's spelling mistake, and a blanked tag is nobody's prose. "Nothing between but" because two adjacent tags of the same length mask to the same word, and `RepeatedWords` would otherwise report the pair as a repeat |
| an engine finding that straddles the join, or starts or ends in whitespace beside a stand-in | it has found something about the real text, so it is kept. Two spaces after a tag are the author's two spaces |
| `STO` → `STO` | a Weir expression matches case-insensitively, which is right for catching `sto` and `Sto` and means it also matches the form the rule steers towards. `becomes` can be a list for exactly that reason, so a match that already is one of the options is not a finding. 50 of one corpus's warnings were that |
| the same text, same rule, twice | two patterns of one rule can both match — a terminology list holding a term and one of its variants. One fault, one line of output |

House rules are not filtered this way. Their patterns run over the
pre-processed text as it stands, so one that can match a run of spaces, or
`Mdoc` followed by `x`s, matches what the pre-processor put there too.

Findings are keyed by the engine's rule name rather than by category.
Harper's flat `lint` call drops the name and leaves only the category, so
every terminology correction would be reported as "Usage" — useless to grep
for and useless to switch off.

A message gets "Use `x` instead of `y`." appended only when it does not
already say so. A rule written for a corpus carries its own wording, and
appending the same thing again produces a diagnostic that says it twice in
two different styles. "Did you mean `preferred`?" gets `Found \`x\`.`
instead: it names the fix and not the fault, which is the half a reader
cannot supply.

## Using it

```
markdoc-prose [--json] [--root <dir>] [--jobs <n>] [--rules] [--version] [path…]
```

| flag | |
|---|---|
| `--root <dir>` | the content root: where both config files are read, and what reported paths are relative to. Defaults to the working directory |
| `--json` | one JSON object on stdout instead of text |
| `--jobs <n>`, `-j <n>` | threads to lint on, at most; `n` is 1 or more. Defaults to the number of CPUs, and 4 is the ceiling either way |
| `--rules` | list every rule the engine has and whether it is on, then exit |
| `--version` | print `markdoc-prose <version>` and exit |
| `--help`, `-h` | print the usage and exit |

Any other argument that starts with `-` is an error; the rest are paths.

With no paths, every `.md` under the root, leaving out anything whose name
starts with `.` and the `node_modules` and `target` directories. With paths,
only those; a directory among them expands the same way, and a file named
outright is linted whatever its extension. A path is resolved against the
working directory as a shell does, then against the root, and anything that
still lands outside the root is skipped with a warning rather than linted
under a relative path no skip list recognises and reported against a
filename nobody can find. A path that resolves to nothing at all is dropped
without one, so the file count in the summary is where a typo shows.

Which files are skipped comes from **`markdoc-lint.toml`**, read from the
same place `markdoc-lint` reads it: a file the structural rules do not apply
to is not a file the prose rules apply to either, and two lists would drift.
That holds for a file named on the command line too. A `markdoc-lint.toml`
that does not parse is not an error here: it is read for the skip list
alone, and in its place comes markdoc-lint's default list — `node_modules/**`,
`.*`, `.*/**` — so nothing the corpus listed is skipped. `markdoc-lint` is
what reports the file.

```
$ markdoc-prose --root ../docs
manuals/safety.md:42:16: warning[Etc] Avoid 'etc.' in lists. Name the specific items.
manuals/setup.md:7:5: warning[sign-in] Use 'sign in' instead of 'log in'.

1103 file(s) checked; 0 error(s), 2 warning(s).
```

Findings go to stdout, the summary to stderr, so a pipe gets only the
findings. `--json` prints one object — `files`, `configured`, `errors`,
`warnings`, `diagnostics` — for CI. `files` counts what was linted: a file
that cannot be read as UTF-8 is passed over without a message and not
counted. With no `markdoc-prose.toml` at the root the text summary says
that only the engine's own rules ran, and `configured` is `false`: a config
in the wrong directory otherwise looks like a corpus that breaks none of
its house rules.

Exit status is 1 if anything was reported at error level, 0 otherwise. A
warning does not fail a build, because a writer who cannot merge until every
style hint is gone stops reading them. A hint is printed and counted in
neither total.

`--rules` lists every rule the engine has and whether it is on, with
`disable`, `enable` and the weirpack applied:

```
$ markdoc-prose --rules
on  ACoupleMore
on  ALittleOfPractice
...
off AvoidContractions
...
on  SpellCheck
...
```

Worth having as a command rather than as documentation. The set comes from
the engine's version, and a list in a README is a list that goes stale
without anybody finding out. It is also the only way to answer "is this
already covered?" before writing a rule. House rules are not in it — they
are in the config already — and with `[engine] enabled = false` it is empty.

A malformed config is fatal rather than ignored. Falling back to the
defaults would turn a typo into "the linter stopped finding things".

### Threads

Nearly all of a run is the engine, and one page does not depend on the
next, so files are linted on several threads. `--jobs` says how many, at
most.

Each thread builds an engine of its own, because one cannot be shared: it
writes to a cache as it reads, and with harper's `concurrent` feature off it
cannot leave the thread that built it. So every thread costs memory — its
caches grow to a couple of hundred megabytes over a large corpus — and on
the machine the limit was measured on, eight threads ran slower than four.
That is why the default stops at four, and why four is a ceiling and not
only a default: asking for more uses four and says so on stderr. A thread is
also only added for every sixteen files, so a short list stays on one thread
whatever it was offered.

None of it shows in the output. Files are handed out in runs and their
findings put back in the order the files were listed, so the output is the
same, byte for byte, for any `n`, and `--jobs 1` lints on the main thread
alone.

### As a library

```rust
use markdoc_prose::{Config, Prose};
use std::path::{Path, PathBuf};

let root = Path::new("../docs");
let config = Config::load(root)?;          // defaults if there is no file
let mut prose = Prose::new(&config, root)?; // compiled once, reused

for diagnostic in prose.lint("manuals/safety.md", &source) {
    println!("{}", diagnostic.to_text());
}

// A list, on up to four threads. `read` runs on the thread that lints
// the file, and a `None` from it comes back as `None` in that place.
let files: Vec<(String, PathBuf)> = vec![
    ("manuals/safety.md".into(), root.join("manuals/safety.md")),
    ("manuals/setup.md".into(), root.join("manuals/setup.md")),
];
let found = prose.lint_many(&files, 4, |path| std::fs::read_to_string(path).ok());
```

`Prose` holds the compiled patterns, the rule group and the merged
dictionary, so build it once and lint every file with it. `Prose::new`
builds an engine straight away, so a bad configuration is an error before
anything is linted.

`lint_many` returns one entry per item, in the order of the items, whichever
thread linted which. It applies the ceiling itself — `jobs` is capped at
`MAX_JOBS`, which is 4, inside the library rather than in the binary that
parses `--jobs`, so no caller can ask past it — and gives a short list fewer
threads, one per sixteen items. `Prose` is neither `Send` nor `Sync`, for
the reason under *Threads*, which is why `lint_many` builds an engine on
each thread it starts instead of sharing this one.

Diagnostics are `markdoc_lint::Diagnostic` — the same type the structural
rules produce, carrying a `Span` in both line/column and character offsets,
so an editor and a build cannot disagree about where a finding is.

`preprocess::preprocess` is public on its own, for anything else that wants
to read a Markdoc document as prose without reimplementing the contract.

## Building

```
cargo build --release
```

`harper-core` is taken with `default-features = false`: that drops
`thesaurus`, a large data table this never consults and which costs build
time and binary size on a tool writers install. It drops `concurrent` as
well, which is what keeps an engine on the thread that built it.

`markdoc-lint` comes from crates.io, for the `Diagnostic`, `Severity`,
`Span`, `Config` and `corpus` it shares, and brings `markdoc`, the parser,
with it, though nothing in this crate uses `markdoc` directly. It has to be a
release whose `Diagnostic.rule` is a `Cow<'static, str>`, as 0.1.0's is: the
rule names here — a house rule's, a Weir file's, harper's own — reach
`Diagnostic::new` as owned `String`s, which it takes only when `rule` is a
`Cow`. To build against unreleased work in a `markdoc-lint` checkout beside
this one, add `[patch.crates-io] markdoc-lint = { path = "../markdoc-lint" }`
to `Cargo.toml`, and take it out before committing: CI has no siblings.

`.github/scripts/check.sh` is the gate CI runs — formatting, clippy with
warnings as errors, the tests, a release build, and that build starting and
listing its rules — and it runs unchanged from a plain checkout.

A release publishes `markdoc-prose-<platform>` for Linux (x86-64 and ARM64,
static), Windows (x86-64 and ARM64) and macOS (Apple Silicon and Intel),
each with a
`.sha256` beside it and a GitHub build-provenance attestation, which ties
it to the commit and the workflow run that built it:

```
gh attestation verify markdoc-prose-linux-x86_64 --repo markdoc-rs/markdoc-prose
gh attestation verify markdoc-prose-linux-x86_64 --repo markdoc-rs/markdoc-prose \
    --bundle markdoc-prose-linux-x86_64.sigstore.json
```

The release carries each attestation too, as `<asset>.sigstore.json`
beside the asset; the second form checks against that copy instead of
asking GitHub.

Releasing is a version bump: push a new version in `Cargo.toml` to main,
and `.github/workflows/release.yml` publishes the crate to crates.io and
the binaries to a GitHub release, and tags the commit.

## Status

- [x] Length-preserving pre-processing, with positions asserted against the
      real file
- [x] Weir rules from a directory or a `.weirpack`, with the failing rule's
      filename in the error
- [x] House rules with lookaround, phrase boundaries, exceptions and levels
- [x] Project dictionary that exempts spelling and nothing else
- [x] Findings keyed by rule name and de-duplicated; the engine's suppressed
      inside stand-ins
- [x] Linting on up to four threads, an engine each, with output that does
      not depend on how many
- [ ] No language server. `markdoc-ls` serves the structural rules; prose
      findings reach a writer through the CLI only
- [ ] `[engine] level` is one setting for every harper rule. A corpus that
      wants spelling at error level and style at hint level cannot say so
- [ ] A rule directory is re-read on every run and never cached, which is
      most of the startup cost on a large pack
- [ ] The only code block the pre-processor blanks is a backtick fence, so a
      house rule still matches inside a `~~~` fence or an indented block. The
      engine's rules — harper's and a Weir rule's — do not, because harper
      reads the text as markdown and skips every code block
- [ ] A `[[rule]]` table does not reject a key it does not know, as every
      other table does
- [ ] A `markdoc-lint.toml` that does not parse is replaced by
      markdoc-lint's default skip list without a word from this tool

## Licence

MIT. See [LICENSE](LICENSE).
