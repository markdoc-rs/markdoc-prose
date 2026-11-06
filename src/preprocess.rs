//! Hide what is not prose, without moving anything.
//!
//! ## The contract
//!
//! The output has exactly as many **characters** as the input, and a newline
//! exactly where the input had one. A finding at character 1 234 of the
//! processed text is at character 1 234 of the real file, so nothing has to
//! be mapped back — which matters because the engine reports character
//! spans, and a position that is nearly right is worse than none.
//!
//! ## Why substitute at all, rather than configure the linter
//!
//! Because the usual configuration does not work. The approach every
//! prose linter offers is "ignore text matching this pattern", implemented
//! as an equal-length run of some filler character. If the filler is `*` it
//! is markdown emphasis, so a line holding a tag *and* an italic phrase
//! stops parsing where the tag was — and anything the linter then treats as
//! emphasis is usually configured to be skipped, so the rest of the line
//! goes unchecked. That is not hypothetical: it was hiding a real finding in
//! the corpus this was written for.
//!
//! ## What a tag becomes depends on what it renders as
//!
//! * `{% $product %}` renders as a word, so it leaves a word behind.
//!   Blanking it turns "The {% $product %} is heavy." into "The  is heavy.",
//!   and every rule downstream is then reading a sentence the document does
//!   not contain.
//! * `{% table %}` renders as structure, so it leaves spaces.
//!
//! The test is position rather than a list of tag names to keep in step with
//! a schema: a tag alone on its line is structural, a tag inside a sentence
//! is read as part of it. `$` and a configured reference tag are always
//! words, wherever they sit.

/// The stem of the stand-in word. Nonsense on purpose: it is not in any
/// dictionary and never appears in real prose, so a rule firing on it is a
/// rule that has gone wrong rather than a finding.
const STEM: &str = "Mdoc";

#[derive(Debug, Clone)]
pub struct Options {
    /// Tags whose content is read as part of the sentence wherever they
    /// appear — a cross-reference renders as the title of its target.
    pub word_tags: Vec<String>,
    /// Whether `**bold**` is prose.
    ///
    /// In a software manual it very often is not: bold is how a UI label is
    /// quoted, and a label is the product's words, not the writer's.
    /// "**Log on as a service**" is a Windows permission; a terminology rule
    /// that turns it into "Sign in as a service" is telling the reader to
    /// look for something that is not on their screen.
    pub skip_bold: bool,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            word_tags: vec!["tagref".to_string()],
            skip_bold: false,
        }
    }
}

/// The result of pre-processing: the text, and where it is not the author's.
pub struct Processed {
    pub text: String,
    /// Character ranges that were substituted. A finding inside one is about
    /// the stand-in, not about the document -- the placeholder word is
    /// nobody's spelling mistake -- so the caller drops it.
    pub substituted: Vec<std::ops::Range<usize>>,
}

impl Processed {
    /// Is this span entirely inside something that was substituted?
    ///
    /// Entirely, not partly: a rule that matched across the join between a
    /// stand-in and real prose has found something about the real prose.
    pub fn is_substituted(&self, start: usize, end: usize) -> bool {
        self.substituted
            .iter()
            .any(|r| start >= r.start && end <= r.end)
    }
}

/// Replace the frontmatter and every tag, preserving every position.
pub fn preprocess(source: &str, options: &Options) -> Processed {
    let (text, substituted) = run(source, options);
    debug_assert_eq!(
        text.chars().count(),
        source.chars().count(),
        "preprocess must preserve the character count"
    );
    Processed { text, substituted }
}

fn run(source: &str, options: &Options) -> (String, Vec<std::ops::Range<usize>>) {
    let (front, front_span) = blank_frontmatter_spanned(source);
    let (tagged, mut spans) = blank_tags_spanned(&front, options);
    let (text, code_spans) = blank_code_spanned(&tagged, options.skip_bold);
    spans.extend(code_spans);
    if let Some(span) = front_span {
        spans.push(span);
    }
    spans.sort_by_key(|r| r.start);
    (text, spans)
}

/// Blank fenced blocks, inline code and link destinations.
///
/// None of it is prose. A JSON example contains `"type": "48V"`, which a
/// units rule reads as a missing space; a URL contains words a spell checker
/// does not know. Every prose linter has a list of markup it skips for
/// exactly this reason, and doing it here rather than in the linter means
/// the project's own patterns get the same treatment as the engine's rules —
/// a pattern that only skipped code when the engine happened to is a pattern
/// that reports different things to different people.
pub fn blank_code_spanned(source: &str, skip_bold: bool) -> (String, Vec<std::ops::Range<usize>>) {
    let mut out = String::with_capacity(source.len());
    let mut spans: Vec<std::ops::Range<usize>> = Vec::new();
    let mut chars_out = 0usize;
    let mut in_fence: Option<String> = None;

    for line in source.split_inclusive('\n') {
        let trimmed = line.trim_start();
        let fence = trimmed
            .starts_with("```")
            .then(|| {
                trimmed
                    .chars()
                    .take_while(|c| *c == '`')
                    .collect::<String>()
            })
            .filter(|f| f.len() >= 3);

        match (&in_fence, fence) {
            (None, Some(f)) => {
                in_fence = Some(f);
                push_blank(line, &mut out, &mut spans, &mut chars_out);
                continue;
            }
            (Some(open), Some(f)) if f.len() >= open.len() => {
                in_fence = None;
                push_blank(line, &mut out, &mut spans, &mut chars_out);
                continue;
            }
            (Some(_), _) => {
                push_blank(line, &mut out, &mut spans, &mut chars_out);
                continue;
            }
            (None, None) => {}
        }

        // Inline: `code`, the destination of [text](dest), and optionally
        // **bold**.
        let mut rest = line;
        while !rest.is_empty() {
            let tick = rest.find('`');
            let link = rest.find("](");
            let bold = skip_bold.then(|| rest.find("**")).flatten();
            let next = [tick, link, bold].into_iter().flatten().min();
            let Some(at) = next else { break };
            let (open_len, close) = if tick == Some(at) {
                (1, rest[at + 1..].find('`').map(|p| at + 1 + p + 1))
            } else if bold == Some(at) {
                (2, rest[at + 2..].find("**").map(|p| at + 2 + p + 2))
            } else {
                (2, rest[at + 2..].find(')').map(|p| at + 2 + p + 1))
            };
            let Some(end) = close else { break };
            // Bold closes with two characters, the others with one.
            let close_len = if bold == Some(at) { 2 } else { 1 };
            // The text before, verbatim.
            push_plain(&rest[..at], &mut out, &mut chars_out);
            // Both delimiters stay and only what they wrap is blanked.
            // Dropping a closing backtick would leave the markdown parser
            // with an unclosed span and change how the rest of the document
            // is read -- which is the whole failure this replaces.
            let open_end = at + open_len;
            push_plain(&rest[at..open_end], &mut out, &mut chars_out);
            let close_start = end - close_len;
            push_blank(
                &rest[open_end..close_start],
                &mut out,
                &mut spans,
                &mut chars_out,
            );
            push_plain(&rest[close_start..end], &mut out, &mut chars_out);
            rest = &rest[end..];
        }
        push_plain(rest, &mut out, &mut chars_out);
    }
    (out, spans)
}

fn push_plain(text: &str, out: &mut String, chars_out: &mut usize) {
    out.push_str(text);
    *chars_out += text.chars().count();
}

fn push_blank(
    text: &str,
    out: &mut String,
    spans: &mut Vec<std::ops::Range<usize>>,
    chars_out: &mut usize,
) {
    let blanked = blank(text);
    let n = blanked.chars().count();
    out.push_str(&blanked);
    spans.push(*chars_out..*chars_out + n);
    *chars_out += n;
}

/// Same length, no new lines, no markdown meaning.
fn blank(text: &str) -> String {
    text.chars()
        .map(|c| if c == '\n' { '\n' } else { ' ' })
        .collect()
}

/// Same length, reads as one word, and cannot join the words beside it.
///
/// The spaces at each end are not padding. A tag is often written hard
/// against a word -- `Checklist{% tag "x" /%}` -- and without them the
/// stand-in merges with it into one token, which a spell checker then
/// reports as a 60-character non-word. The report is about the stand-in, but
/// the span covers real text too, so it cannot simply be dropped.
fn word(text: &str, pad_left: bool, pad_right: bool) -> String {
    if text.contains('\n') {
        // A tag broken across lines cannot be one word, and a word with a
        // newline in it would move every following position.
        return blank(text);
    }
    let n = text.chars().count();
    let pads = usize::from(pad_left) + usize::from(pad_right);
    // A gap is only inserted where one is needed, and only if there is room
    // for a letter between the gaps. Padding unconditionally would put two
    // spaces where the author wrote one, which a spacing rule then reports
    // -- 3 873 times over one corpus.
    if n <= pads {
        return " ".repeat(n);
    }
    let inner = n - pads;
    let mut out = String::with_capacity(n);
    if pad_left {
        out.push(' ');
    }
    if inner <= STEM.chars().count() {
        out.extend(std::iter::repeat_n('x', inner));
    } else {
        out.push_str(STEM);
        out.extend(std::iter::repeat_n('x', inner - STEM.chars().count()));
    }
    if pad_right {
        out.push(' ');
    }
    out
}

/// Replace the YAML frontmatter with spaces, keeping every newline.
///
/// Tolerant of what real corpora contain: a byte-order mark, trailing
/// whitespace on a delimiter, CRLF endings. A frontmatter rule that is
/// anchored too tightly does not fail loudly — it just stops hiding one
/// file's metadata, and the rules start reporting findings against fields
/// nobody wrote as prose.
pub fn blank_frontmatter(source: &str) -> String {
    blank_frontmatter_spanned(source).0
}

fn blank_frontmatter_spanned(source: &str) -> (String, Option<std::ops::Range<usize>>) {
    let body = source.strip_prefix('\u{FEFF}').unwrap_or(source);
    let bom = source.len() - body.len();
    let mut lines = body.split_inclusive('\n');
    let Some(first) = lines.next() else {
        return (source.to_string(), None);
    };
    if first.trim_end_matches(['\r', '\n']).trim_end() != "---" {
        return (source.to_string(), None);
    }
    let mut consumed = first.len();
    for line in lines {
        consumed += line.len();
        if line.trim_end_matches(['\r', '\n']).trim_end() == "---" {
            let end = bom + consumed;
            let text = format!(
                "{}{}{}",
                &source[..bom],
                blank(&source[bom..end]),
                &source[end..]
            );
            let chars_before = source[..bom].chars().count();
            let chars_in = source[bom..end].chars().count();
            return (text, Some(chars_before..chars_before + chars_in));
        }
    }
    // An opening delimiter with no closing one is not frontmatter.
    (source.to_string(), None)
}

/// Replace every Markdoc tag according to what it renders as.
pub fn blank_tags(source: &str, options: &Options) -> String {
    blank_tags_spanned(source, options).0
}

fn blank_tags_spanned(source: &str, options: &Options) -> (String, Vec<std::ops::Range<usize>>) {
    let bytes = source.as_bytes();
    let mut out = String::with_capacity(source.len());
    let mut spans = Vec::new();
    let mut chars_out = 0usize;
    let mut i = 0usize;
    while i < bytes.len() {
        if bytes[i] == b'{' && bytes.get(i + 1) == Some(&b'%') {
            // `\{%` is an escaped literal brace in Markdoc, not a tag.
            let escaped = i > 0 && bytes[i - 1] == b'\\';
            if let Some(end) = find_close(source, i)
                && !escaped
            {
                let tag = &source[i..end];
                let replacement = if renders_as_words(tag, source, i, end, options) {
                    // Only separate the stand-in from a neighbour that would
                    // otherwise merge with it into one token.
                    // What was emitted, not what was written: two tags
                    // written back to back -- `{% a %}{% b %}` -- have `}`
                    // between them in the source, which is not a word
                    // character, but two stand-ins beside each other in the
                    // output and those do merge.
                    let wordish = |c: Option<char>| {
                        c.is_some_and(|c| c.is_alphanumeric() || c == '_' || c == '\'')
                    };
                    let before = out.chars().next_back();
                    let after = source[end..].chars().next();
                    word(tag, wordish(before), wordish(after))
                } else {
                    blank(tag)
                };
                let n = replacement.chars().count();
                out.push_str(&replacement);
                spans.push(chars_out..chars_out + n);
                chars_out += n;
                i = end;
                continue;
            }
        }
        let ch = source[i..].chars().next().unwrap();
        out.push(ch);
        chars_out += 1;
        i += ch.len_utf8();
    }
    (out, spans)
}

fn find_close(source: &str, from: usize) -> Option<usize> {
    source[from..].find("%}").map(|rel| from + rel + 2)
}

fn renders_as_words(tag: &str, source: &str, start: usize, end: usize, o: &Options) -> bool {
    let inner = tag[2..tag.len() - 2].trim();
    if inner.starts_with('/') {
        return false;
    }
    if inner.starts_with('$') {
        return true;
    }
    let name = inner.split_whitespace().next().unwrap_or("");
    if o.word_tags.iter().any(|t| t == name) {
        return true;
    }
    let line_start = source[..start].rfind('\n').map_or(0, |p| p + 1);
    let line_end = source[end..].find('\n').map_or(source.len(), |p| end + p);
    let before = &source[line_start..start];
    let after = &source[end..line_end];
    // A list marker still counts as alone: `* {% x %}` is a table cell, not
    // a sentence.
    let alone_before = before
        .chars()
        .all(|c| c.is_whitespace() || matches!(c, '>' | '*' | '-' | '+'));
    let alone_after = after.chars().all(char::is_whitespace);
    !(alone_before && alone_after)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pre(s: &str) -> String {
        preprocess(s, &Options::default()).text
    }

    #[test]
    fn an_inline_variable_leaves_a_word() {
        assert_eq!(
            pre("The {% $product %} is heavy.\n"),
            "The Mdocxxxxxxxxxx is heavy.\n"
        );
    }

    #[test]
    fn a_tag_alone_on_its_line_leaves_spaces() {
        assert_eq!(
            pre("{% table column_weights=\"1 1\" %}\n* a\n{% /table %}\n"),
            format!("{}\n* a\n{}\n", " ".repeat(32), " ".repeat(12))
        );
    }

    #[test]
    fn a_tag_that_is_a_whole_cell_leaves_spaces() {
        assert_eq!(
            pre("* {% partial file=\"a.md\" /%}\n"),
            format!("* {}\n", " ".repeat(26))
        );
    }

    #[test]
    fn a_reference_is_a_word_even_alone_on_its_line() {
        // It renders as the title of what it points at, so a sentence built
        // around it still has a subject.
        let out = pre("{% tagref \"safety\" /%}\n");
        assert!(out.trim_start().starts_with("Mdocx"), "{out}");
    }

    #[test]
    fn frontmatter_is_hidden() {
        assert_eq!(
            pre("---\ntitle: \"Move it\"\n---\n\nBody.\n"),
            "   \n                \n   \n\nBody.\n"
        );
    }

    #[test]
    fn frontmatter_survives_a_trailing_space_a_bom_and_crlf() {
        for src in [
            "--- \nid: \"x\"\n---\n\nBody.\n",
            "\u{FEFF}---\nid: \"x\"\n---\n\nBody.\n",
            "---\r\nid: \"x\"\r\n---\r\n\r\nBody.\r\n",
        ] {
            let out = pre(src);
            assert!(
                !out.contains("id:"),
                "frontmatter not hidden in {src:?} -> {out:?}"
            );
            assert_eq!(out.chars().count(), src.chars().count(), "{src:?}");
        }
    }

    #[test]
    fn an_opening_delimiter_with_no_close_is_not_frontmatter() {
        let src = "---\nnot closed\n\nBody.\n";
        assert_eq!(pre(src), src);
    }

    #[test]
    fn a_stand_in_cannot_join_the_word_beside_it() {
        // Tags are written hard against words: `Checklist{% tag "x" /%}`.
        // Without a gap the stand-in merges into one token and a spell
        // checker reports a 60-character non-word -- over a span that
        // covers real text, so it cannot just be dropped.
        let out = pre("Checklist{% tag \"received\" /%} and more.\n");
        assert!(!out.contains("ChecklistMdoc"), "{out:?}");
        assert!(out.contains("Checklist "), "{out:?}");
    }

    #[test]
    fn two_stand_ins_do_not_merge_into_one() {
        // `{% a %}{% b %}` has `}` between them in the source, which is not
        // a word character -- but the stand-ins are beside each other in the
        // output, and those do merge.
        let out = pre("x{% $a %}{% tagref \"b\" /%}y\n");
        assert!(!out.contains("xxMdoc"), "{out:?}");
        for part in out.split_whitespace() {
            assert!(
                part.matches("Mdoc").count() <= 1,
                "two stand-ins in one token: {out:?}"
            );
        }
    }

    #[test]
    fn code_is_not_prose() {
        // A JSON example holds `"type": "48V"`, which a units rule reads as
        // a missing space; a fenced block holds whatever it holds.
        let out = pre("Set `--log-driver=journald` here.\n");
        assert_eq!(out, "Set `                     ` here.\n");
        let fenced = pre("Text.\n\n```json\n{\"type\": \"48V\"}\n```\n\nMore.\n");
        assert!(!fenced.contains("48V"), "{fenced:?}");
        assert!(fenced.contains("Text."), "{fenced:?}");
        assert!(fenced.contains("More."), "{fenced:?}");
    }

    #[test]
    fn a_links_destination_is_hidden_but_its_text_is_not() {
        // The words a reader sees are prose; the URL is not.
        let out = pre("See [the safety page](https://example.com/trolley) now.\n");
        assert!(out.contains("the safety page"), "{out:?}");
        assert!(!out.contains("example.com"), "{out:?}");
        assert!(!out.contains("trolley"), "{out:?}");
    }

    #[test]
    fn bold_is_skipped_only_when_asked() {
        // In a software manual bold is usually a UI label quoted from the
        // product -- the product's words, not the writer's.
        let src = "The user needs **Log on as a service** here.\n";
        let o = Options {
            skip_bold: true,
            ..Default::default()
        };
        let skipped = preprocess(src, &o).text;
        assert!(!skipped.contains("Log on"), "{skipped:?}");
        assert!(skipped.contains("**"), "the markers stay: {skipped:?}");
        assert!(skipped.contains("The user needs"), "{skipped:?}");
        assert_eq!(skipped.chars().count(), src.chars().count());

        let kept = pre(src);
        assert!(kept.contains("Log on as a service"), "{kept:?}");
    }

    #[test]
    fn an_escaped_brace_is_left_alone() {
        let src = "Write \\{% table %} to show the tag.\n";
        assert_eq!(pre(src), src);
    }

    #[test]
    fn positions_are_preserved_for_anything() {
        for src in [
            "The {% $a %} and {% tagref \"b\" /%} on one line with *emphasis*.\n",
            "---\nk: v\n---\n\n{% notice type=\"warning\" %}\nText.\n{% /notice %}\n",
            "No tags at all.\n",
            "A tag spanning\n{% table\n   column_weights=\"1 1\" %}\nlines.\n",
            "Unicode café and a {% $x %} tag.\n",
            "Code `x` and a [link](http://a.b) and ```\nfence\n``` here.\n",
            "An unclosed ` backtick and an unclosed [link](see.\n",
        ] {
            let out = pre(src);
            assert_eq!(out.chars().count(), src.chars().count(), "{src:?}");
            let lines = |s: &str| s.split('\n').map(|l| l.chars().count()).collect::<Vec<_>>();
            assert_eq!(lines(&out), lines(src), "{src:?}");
        }
    }
}
