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
| `{% tagref "safety" /%}` | the same stand-in | it renders as the title of its target, so a sentence built around it still has a subject |
| `{% table %}` alone on its line | spaces | it renders as structure |
| `{% /notice %}` | spaces | a closing tag renders as nothing |
| a tag broken across lines | spaces | a word cannot contain a newline without moving every position after it |
| frontmatter | spaces | it is metadata |
| `` `code` ``, fenced blocks, link destinations | spaces, delimiters kept | not prose |
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

Each file's stem is the rule name, which is also what `[engine] disable`
refers to. Files load in sorted order, so a rule set loads the same way
twice and a diff of output is a diff of behaviour.

Rules are compiled one at a time before the pack is built, which costs a
little and buys the only thing that matters when a pack is wrong: the
filename. `to_lint_group` reports the first failure without saying where it
came from, so a pack of two hundred produced `Weir(UnsupportedToken("12"))`
and nothing to act on. (It was `12:00 noon`. A Weir expression holds words,
punctuation and tags, and a numeral is none of those.)

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
file at all the engine's own rules run and nothing else. An unknown key is
an error rather than a silent no-op, because a typo in a config that still
parses is a rule that stops firing without anyone noticing.

```toml
[engine]
enabled = true                 # false for the first pass of a migration
dialect = "American"           # or British, Canadian, Australian, Indian
level = "warning"              # what a finding from the engine counts as
weirpack = "_tooling/prose/rules"   # a directory, or a .weirpack file
# Rule names as `--rules` prints them; an unknown one is an error.
disable = ["SpellCheck"]
enable  = []
dictionary = ["Modbus", "M8", "STO"]
dictionary_file = "_tooling/prose/vocabulary.txt"

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
exceptions = ["Laser Scanner Pro"]
```

`%s` in a message is the matched text and `%r` is `becomes`. A name in
`disable` or `enable` that no rule answers to is an error, not a no-op —
that is the other half of the same bargain.

`dictionary_file` is one word per line, `#` for a comment. A few hundred
product names and part codes do not belong inline in a config file, and a
plain list is reviewable in a way a TOML array is not: a diff of it reads as
"these words were added".

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
| a finding entirely inside a stand-in | the placeholder word is nobody's spelling mistake, and a blanked tag is nobody's prose |
| a finding that straddles the join | it has found something about the real text, so it is kept |
| `STO` → `STO` | a Weir expression matches case-insensitively, which is right for catching `sto` and `Sto` and means it also matches the form the rule steers towards. `becomes` can be a list for exactly that reason, so a match that already is one of the options is not a finding. 50 of one corpus's warnings were that |
| the same text, same rule, twice | two patterns of one rule can both match — a terminology list holding a term and one of its variants. One fault, one line of output |

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
markdoc-prose [--json] [--root <dir>] [--rules] [--version] [path…]
```

With no paths, every `.md` under the root. With paths, only those; a
directory among them expands. A path is resolved against the working
directory as a shell does, then against the root, and anything that still
lands outside the root is skipped with a warning rather than linted under a
relative path no skip list recognises and reported against a filename nobody
can find.

Which files are skipped comes from **`markdoc-lint.toml`**, read from the
same place `markdoc-lint` reads it: a file the structural rules do not apply
to is not a file the prose rules apply to either, and two lists would drift.

```
$ markdoc-prose --root ../mir-docs-content
manuals/safety.md:42:16: warning[Etc] Avoid 'etc.' in lists. Name the specific items.
manuals/setup.md:7:5: warning[sign-in] Use 'sign in' instead of 'log in'.

1103 file(s) checked; 0 error(s), 2 warning(s).
```

Findings go to stdout, the summary to stderr, so a pipe gets only the
findings. `--json` prints one object — `files`, `configured`, `errors`,
`warnings`, `diagnostics` — for CI.

Exit status is 1 if anything was reported at error level, 0 otherwise. A
warning does not fail a build, because a writer who cannot merge until every
style hint is gone stops reading them.

`--rules` lists every rule the engine has and whether it is on:

```
$ markdoc-prose --rules
on  SpellCheck
off ThatWhich
...
```

Worth having as a command rather than as documentation. The set comes from
the engine's version, and a list in a README is a list that goes stale
without anybody finding out. It is also the only way to answer "is this
already covered?" before writing a rule.

A malformed config is fatal rather than ignored. Falling back to the
defaults would turn a typo into "the linter stopped finding things".

### As a library

```rust
use markdoc_prose::{Config, Prose};
use std::path::Path;

let root = Path::new("../mir-docs-content");
let config = Config::load(root)?;          // defaults if there is no file
let mut prose = Prose::new(&config, root)?; // compiled once, reused

for diagnostic in prose.lint("manuals/safety.md", &source) {
    println!("{}", diagnostic.to_text());
}
```

`Prose` holds the compiled patterns, the rule group and the merged
dictionary, so build it once and lint every file with it. Diagnostics are
`markdoc_lint::Diagnostic` — the same type the structural rules produce,
carrying a `Span` in both line/column and character offsets, so an editor
and a build cannot disagree about where a finding is.

`preprocess::preprocess` is public on its own, for anything else that wants
to read a Markdoc document as prose without reimplementing the contract.

## Building

`harper-core` is taken with `default-features = false`: that drops
`thesaurus`, a large data table this never consults and which costs build
time and binary size on a tool writers install.

`markdoc-lint` is a path dependency on a sibling checkout, for the
`Diagnostic`, `Severity`, `Span` and `corpus` it shares — so a
`markdoc-lint` directory has to sit next to this one.

## Status

- [x] Length-preserving pre-processing, with positions asserted against the
      real file
- [x] Weir rules from a directory or a `.weirpack`, with the failing rule's
      filename in the error
- [x] House rules with lookaround, phrase boundaries, exceptions and levels
- [x] Project dictionary that exempts spelling and nothing else
- [x] Findings keyed by rule name, de-duplicated, suppressed inside stand-ins
- [ ] No language server. `markdoc-ls` serves the structural rules; prose
      findings reach a writer through the CLI only
- [ ] `[engine] level` is one setting for every harper rule. A corpus that
      wants spelling at error level and style at hint level cannot say so
- [ ] A rule directory is re-read on every run and never cached, which is
      most of the startup cost on a large pack

## Licence

MIT. See [LICENSE](LICENSE).
