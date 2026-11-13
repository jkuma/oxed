use imara_diff::{Algorithm, Diff, Hunk, InternedInput, sources};
use std::ops::Range;

/// How a changed region of the base was changed by each side.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ChunkKind {
    OursOnly,
    TheirsOnly,
    /// Both sides made the same change.
    BothSame,
    Conflict,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Strategy {
    /// The sides differ only in trailing whitespace or line endings.
    Whitespace,
    /// The sides changed different words of the conflicting lines.
    Words,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Resolution {
    pub strategy: Strategy,
    pub text: String,
}

/// A changed region, as line ranges in each revision.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Chunk {
    pub base: Range<u32>,
    pub ours: Range<u32>,
    pub theirs: Range<u32>,
    pub kind: ChunkKind,
    /// Set on conflicts that a strategy resolved.
    pub resolution: Option<Resolution>,
}

impl Chunk {
    pub fn is_unresolved_conflict(&self) -> bool {
        self.kind == ChunkKind::Conflict && self.resolution.is_none()
    }
}

pub struct MarkerLabels<'a> {
    pub ours: &'a str,
    /// Without a label, conflict markers leave out the base section.
    pub base: Option<&'a str>,
    pub theirs: &'a str,
}

/// A three-way merge of base, ours and theirs, with its changed chunks in order.
#[derive(Clone, Debug)]
pub struct MergeDocument {
    base: Text,
    ours: Text,
    theirs: Text,
    /// Without a common ancestor, the base only holds the lines both sides share.
    has_base: bool,
    chunks: Vec<Chunk>,
}

#[derive(Clone, Debug)]
struct Text {
    content: String,
    /// The byte offset of each line start, followed by the length of the content.
    line_starts: Vec<usize>,
}

impl Text {
    fn new(content: String) -> Self {
        let mut line_starts = vec![0];
        let mut offset = 0;
        for line in sources::lines(&content) {
            offset += line.len();
            line_starts.push(offset);
        }
        Self {
            content,
            line_starts,
        }
    }

    fn line_count(&self) -> u32 {
        (self.line_starts.len() - 1) as u32
    }

    fn lines(&self, range: Range<u32>) -> &str {
        let start = self.line_starts.get(range.start as usize);
        let end = self.line_starts.get(range.end as usize);
        match (start, end) {
            (Some(&start), Some(&end)) => self.content.get(start..end).unwrap_or_default(),
            _ => "",
        }
    }
}

/// Merges ours and theirs line by line, with the same conflicts as git: changes conflict when
/// their base ranges overlap or touch, unless both sides made the same change.
///
/// Without a base, as for a file both sides added, every difference between the sides is a
/// conflict.
pub fn merge_texts(base: Option<&str>, ours: &str, theirs: &str) -> MergeDocument {
    match base {
        Some(base) => {
            let chunks = merge_tokens(
                sources::lines(base),
                sources::lines(ours),
                sources::lines(theirs),
                Tokens::Lines,
            );
            MergeDocument {
                base: Text::new(base.to_string()),
                ours: Text::new(ours.to_string()),
                theirs: Text::new(theirs.to_string()),
                has_base: true,
                chunks,
            }
        }
        None => merge_without_base(ours, theirs),
    }
}

fn merge_without_base(ours: &str, theirs: &str) -> MergeDocument {
    let ours = Text::new(ours.to_string());
    let theirs = Text::new(theirs.to_string());
    let input = InternedInput::new(ours.content.as_str(), theirs.content.as_str());
    let mut base = String::new();
    let mut base_line = 0;
    let mut ours_line = 0;
    let mut chunks = Vec::new();
    for hunk in hunks(&input, Tokens::Lines) {
        base.push_str(ours.lines(ours_line..hunk.before.start));
        base_line += hunk.before.start - ours_line;
        ours_line = hunk.before.end;
        chunks.push(Chunk {
            base: base_line..base_line,
            ours: hunk.before,
            theirs: hunk.after,
            kind: ChunkKind::Conflict,
            resolution: None,
        });
    }
    base.push_str(ours.lines(ours_line..ours.line_count()));
    MergeDocument {
        base: Text::new(base),
        ours,
        theirs,
        has_base: false,
        chunks,
    }
}

impl MergeDocument {
    pub fn chunks(&self) -> &[Chunk] {
        &self.chunks
    }

    pub fn base_text(&self, chunk: &Chunk) -> &str {
        self.base.lines(chunk.base.clone())
    }

    pub fn ours_text(&self, chunk: &Chunk) -> &str {
        self.ours.lines(chunk.ours.clone())
    }

    pub fn theirs_text(&self, chunk: &Chunk) -> &str {
        self.theirs.lines(chunk.theirs.clone())
    }

    pub fn conflict_count(&self) -> usize {
        self.chunks
            .iter()
            .filter(|chunk| chunk.is_unresolved_conflict())
            .count()
    }

    /// Applies every change and resolution, and writes conflict markers for the conflicts left.
    pub fn render(&self, labels: &MarkerLabels) -> String {
        let newline = line_ending(&self.ours.content);
        let mut output = String::with_capacity(self.ours.content.len());
        let mut base_line = 0;
        for chunk in &self.chunks {
            output.push_str(self.base.lines(base_line..chunk.base.start));
            match chunk.kind {
                ChunkKind::OursOnly | ChunkKind::BothSame => output.push_str(self.ours_text(chunk)),
                ChunkKind::TheirsOnly => output.push_str(self.theirs_text(chunk)),
                ChunkKind::Conflict => match &chunk.resolution {
                    Some(resolution) => output.push_str(&resolution.text),
                    None => {
                        push_marker(&mut output, "<<<<<<<", labels.ours, newline);
                        push_section(&mut output, self.ours_text(chunk), newline);
                        if let Some(base_label) = labels.base {
                            push_marker(&mut output, "|||||||", base_label, newline);
                            push_section(&mut output, self.base_text(chunk), newline);
                        }
                        output.push_str("=======");
                        output.push_str(newline);
                        push_section(&mut output, self.theirs_text(chunk), newline);
                        push_marker(&mut output, ">>>>>>>", labels.theirs, newline);
                    }
                },
            }
            base_line = chunk.base.end;
        }
        output.push_str(self.base.lines(base_line..self.base.line_count()));
        output
    }
}

fn line_ending(text: &str) -> &'static str {
    match text.find('\n') {
        Some(index) if text[..index].ends_with('\r') => "\r\n",
        _ => "\n",
    }
}

fn push_marker(output: &mut String, marker: &str, label: &str, newline: &str) {
    output.push_str(marker);
    output.push(' ');
    output.push_str(label);
    output.push_str(newline);
}

fn push_section(output: &mut String, text: &str, newline: &str) {
    output.push_str(text);
    if !text.is_empty() && !text.ends_with('\n') {
        output.push_str(newline);
    }
}

/// Resolves conflicts whose sides differ only in trailing whitespace or line endings, keeping
/// the side that made the other change. Indentation is significant and is never ignored.
pub fn resolve_whitespace(document: &mut MergeDocument) {
    let MergeDocument {
        base,
        ours,
        theirs,
        has_base,
        chunks,
    } = document;
    for chunk in chunks.iter_mut() {
        if !chunk.is_unresolved_conflict() {
            continue;
        }
        let ours_text = ours.lines(chunk.ours.clone());
        let theirs_text = theirs.lines(chunk.theirs.clone());
        let normalized_base = without_trailing_whitespace(base.lines(chunk.base.clone()));
        let normalized_ours = without_trailing_whitespace(ours_text);
        let normalized_theirs = without_trailing_whitespace(theirs_text);
        let text = if normalized_ours == normalized_theirs {
            ours_text
        } else if *has_base && normalized_ours == normalized_base {
            theirs_text
        } else if *has_base && normalized_theirs == normalized_base {
            ours_text
        } else {
            continue;
        };
        chunk.resolution = Some(Resolution {
            strategy: Strategy::Whitespace,
            text: text.to_string(),
        });
    }
}

fn without_trailing_whitespace(text: &str) -> String {
    let mut normalized = String::with_capacity(text.len());
    for line in text.split_inclusive('\n') {
        normalized.push_str(line.trim_end_matches(['\n', '\r', ' ', '\t']));
        normalized.push('\n');
    }
    normalized
}

/// Resolves conflicts in which the sides changed different words: the conflicting lines are
/// merged again word by word, and the conflict is resolved if no words conflict.
pub fn resolve_words(document: &mut MergeDocument) {
    let MergeDocument {
        base,
        ours,
        theirs,
        chunks,
        ..
    } = document;
    for chunk in chunks.iter_mut() {
        if !chunk.is_unresolved_conflict() {
            continue;
        }
        let base_words = words(base.lines(chunk.base.clone()));
        let ours_words = words(ours.lines(chunk.ours.clone()));
        let theirs_words = words(theirs.lines(chunk.theirs.clone()));
        let word_chunks = merge_tokens(
            base_words.iter().copied(),
            ours_words.iter().copied(),
            theirs_words.iter().copied(),
            Tokens::Words,
        );
        if let Some(text) = assemble(&base_words, &ours_words, &theirs_words, &word_chunks) {
            chunk.resolution = Some(Resolution {
                strategy: Strategy::Words,
                text,
            });
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum WordClass {
    Word,
    Space,
    Newline,
    Punctuation,
}

impl WordClass {
    fn of(character: char) -> Self {
        if character == '\n' {
            Self::Newline
        } else if character.is_whitespace() {
            Self::Space
        } else if character.is_alphanumeric() || character == '_' {
            Self::Word
        } else {
            Self::Punctuation
        }
    }
}

/// Splits text into words, runs of spaces, and single newlines and punctuation characters.
fn words(text: &str) -> Vec<&str> {
    let mut words = Vec::new();
    let mut start = 0;
    let mut previous_class = None;
    for (index, character) in text.char_indices() {
        let class = WordClass::of(character);
        if let Some(previous_class) = previous_class
            && (class != previous_class
                || matches!(class, WordClass::Newline | WordClass::Punctuation))
        {
            words.push(&text[start..index]);
            start = index;
        }
        previous_class = Some(class);
    }
    if start < text.len() {
        words.push(&text[start..]);
    }
    words
}

fn assemble(base: &[&str], ours: &[&str], theirs: &[&str], chunks: &[Chunk]) -> Option<String> {
    let range = |range: &Range<u32>| range.start as usize..range.end as usize;
    let mut text = String::new();
    let mut base_index = 0;
    for chunk in chunks {
        text.extend(
            base.get(base_index..chunk.base.start as usize)?
                .iter()
                .copied(),
        );
        let words = match chunk.kind {
            ChunkKind::OursOnly | ChunkKind::BothSame => ours.get(range(&chunk.ours))?,
            ChunkKind::TheirsOnly => theirs.get(range(&chunk.theirs))?,
            ChunkKind::Conflict => return None,
        };
        text.extend(words.iter().copied());
        base_index = chunk.base.end as usize;
    }
    text.extend(base.get(base_index..)?.iter().copied());
    Some(text)
}

#[derive(Clone, Copy)]
enum Tokens {
    Lines,
    Words,
}

fn hunks(input: &InternedInput<&str>, tokens: Tokens) -> Vec<Hunk> {
    let mut diff = Diff::compute(Algorithm::Histogram, input);
    match tokens {
        Tokens::Lines => diff.postprocess_lines(input),
        Tokens::Words => diff.postprocess_no_heuristic(input),
    }
    diff.hunks().collect()
}

fn merge_tokens<'a>(
    base: impl Iterator<Item = &'a str>,
    ours: impl Iterator<Item = &'a str>,
    theirs: impl Iterator<Item = &'a str>,
    tokens: Tokens,
) -> Vec<Chunk> {
    let mut input = InternedInput::default();
    input.update_before(base);
    input.update_after(ours);
    let ours_hunks = hunks(&input, tokens);
    let ours_tokens = std::mem::take(&mut input.after);
    input.update_after(theirs);
    let theirs_hunks = hunks(&input, tokens);
    let theirs_tokens = &input.after;
    diff3(&ours_hunks, &theirs_hunks, |ours_range, theirs_range| {
        let ours_range = ours_range.start as usize..ours_range.end as usize;
        let theirs_range = theirs_range.start as usize..theirs_range.end as usize;
        ours_tokens.get(ours_range) == theirs_tokens.get(theirs_range)
    })
}

/// A base position and the side position it corresponds to, where the side is unchanged.
#[derive(Clone, Copy, Default)]
struct SyncPoint {
    base: u32,
    side: u32,
}

fn diff3(
    ours: &[Hunk],
    theirs: &[Hunk],
    same_change: impl Fn(&Range<u32>, &Range<u32>) -> bool,
) -> Vec<Chunk> {
    let mut chunks = Vec::new();
    let mut ours_index = 0;
    let mut theirs_index = 0;
    let mut ours_sync = SyncPoint::default();
    let mut theirs_sync = SyncPoint::default();
    loop {
        let start = match (ours.get(ours_index), theirs.get(theirs_index)) {
            (None, None) => break,
            (Some(hunk), None) | (None, Some(hunk)) => hunk.before.start,
            (Some(ours_hunk), Some(theirs_hunk)) => {
                ours_hunk.before.start.min(theirs_hunk.before.start)
            }
        };
        let mut base = start..start;
        let first_ours = ours_index;
        let first_theirs = theirs_index;
        loop {
            if let Some(hunk) = ours.get(ours_index)
                && hunk.before.start <= base.end
            {
                base.end = base.end.max(hunk.before.end);
                ours_index += 1;
            } else if let Some(hunk) = theirs.get(theirs_index)
                && hunk.before.start <= base.end
            {
                base.end = base.end.max(hunk.before.end);
                theirs_index += 1;
            } else {
                break;
            }
        }
        let ours_hunks = &ours[first_ours..ours_index];
        let theirs_hunks = &theirs[first_theirs..theirs_index];
        let ours_range = side_range(ours_hunks, &base, &mut ours_sync);
        let theirs_range = side_range(theirs_hunks, &base, &mut theirs_sync);
        let kind = if ours_hunks.is_empty() {
            ChunkKind::TheirsOnly
        } else if theirs_hunks.is_empty() {
            ChunkKind::OursOnly
        } else if same_change(&ours_range, &theirs_range) {
            ChunkKind::BothSame
        } else {
            ChunkKind::Conflict
        };
        chunks.push(Chunk {
            base,
            ours: ours_range,
            theirs: theirs_range,
            kind,
            resolution: None,
        });
    }
    chunks
}

fn side_range(hunks: &[Hunk], base: &Range<u32>, sync: &mut SyncPoint) -> Range<u32> {
    match (hunks.first(), hunks.last()) {
        (Some(first), Some(last)) => {
            let start = first.after.start - (first.before.start - base.start);
            let end = last.after.end + (base.end - last.before.end);
            *sync = SyncPoint {
                base: last.before.end,
                side: last.after.end,
            };
            start..end
        }
        _ => {
            let start = sync.side + (base.start - sync.base);
            start..start + (base.end - base.start)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;
    use std::process::Command;

    const LABELS: MarkerLabels = MarkerLabels {
        ours: "ours",
        base: Some("base"),
        theirs: "theirs",
    };

    #[allow(clippy::disallowed_methods)]
    fn git_merge_file(base: &str, ours: &str, theirs: &str) -> String {
        let directory = tempfile::tempdir().expect("temporary directory");
        for (name, content) in [("base", base), ("ours", ours), ("theirs", theirs)] {
            std::fs::write(directory.path().join(name), content).expect("write input");
        }
        let output = Command::new("git")
            .current_dir(directory.path())
            .env("GIT_CONFIG_GLOBAL", "")
            .env("GIT_CONFIG_SYSTEM", "")
            .args(["merge-file", "-p", "--diff3", "-L", "ours", "-L", "base"])
            .args(["-L", "theirs", "ours", "base", "theirs"])
            .output()
            .expect("run git merge-file");
        String::from_utf8(output.stdout).expect("utf-8 output")
    }

    #[test]
    fn line_merge_matches_git() {
        for (name, base, ours, theirs) in [
            ("no change", "a\nb\n", "a\nb\n", "a\nb\n"),
            ("ours only", "a\nb\nc\n", "a\nB\nc\n", "a\nb\nc\n"),
            ("theirs only", "a\nb\nc\n", "a\nb\nc\n", "a\nb\nC\n"),
            ("same change", "a\nb\nc\n", "a\nB\nc\n", "a\nB\nc\n"),
            (
                "adjacent changes",
                "a\nb\nc\nd\n",
                "a\nB\nc\nd\n",
                "a\nb\nC\nd\n",
            ),
            (
                "separated changes",
                "a\nb\nc\nd\n",
                "a\nB\nc\nd\n",
                "a\nb\nc\nD\n",
            ),
            (
                "insertions at one point",
                "a\nb\n",
                "a\nx\nb\n",
                "a\ny\nb\n",
            ),
            ("identical insertions", "a\nb\n", "a\nx\nb\n", "a\nx\nb\n"),
            ("final newline", "a\nb\nc\n", "a\nb\nc\nx\n", "a\nb\nc"),
            ("modify and delete", "a\nb\nc\n", "a\nX\nc\n", "a\nc\n"),
            (
                "one conflict among changes",
                "a\nb\nc\nd\ne\n",
                "a\nb1\nc\nd1\ne\n",
                "a\nb2\nc\nd\ne\n",
            ),
            ("insertion at start", "a\nb\n", "x\na\nb\n", "a\nb\ny\n"),
            ("empty base", "", "a\n", "b\n"),
            ("appends on both sides", "a\n", "a\nx\n", "a\ny\n"),
            ("one side empties the file", "a\nb\n", "", "a\nB\n"),
            (
                "deletions at both ends",
                "a\nb\nc\nd\n",
                "b\nc\nd\n",
                "a\nb\nc\n",
            ),
            (
                "imports added on both sides",
                "use a;\nuse b;\n\nfn main() {}\n",
                "use a;\nuse b;\nuse c;\n\nfn main() {}\n",
                "use a;\nuse b;\nuse d;\n\nfn main() {}\n",
            ),
            (
                "several conflicts",
                "1\n2\n3\n4\n5\n6\n7\n8\n9\n",
                "1\n2a\n3\n4\n5a\n5b\n6\n7\n9\n",
                "1\n2b\n3\n4\n5\n6c\n7\n8\n9\nend\n",
            ),
        ] {
            let document = merge_texts(Some(base), ours, theirs);
            assert_eq!(
                document.render(&LABELS),
                git_merge_file(base, ours, theirs),
                "{name}"
            );
        }
    }

    #[test]
    fn chunk_kinds_and_ranges() {
        let document = merge_texts(
            Some("a\nb\nc\nd\ne\n"),
            "a\nB\nc\nd\ne\nf\n",
            "a\nB\nc\nD\ne\n",
        );
        assert_eq!(
            document.chunks(),
            [
                Chunk {
                    base: 1..2,
                    ours: 1..2,
                    theirs: 1..2,
                    kind: ChunkKind::BothSame,
                    resolution: None,
                },
                Chunk {
                    base: 3..4,
                    ours: 3..4,
                    theirs: 3..4,
                    kind: ChunkKind::TheirsOnly,
                    resolution: None,
                },
                Chunk {
                    base: 5..5,
                    ours: 5..6,
                    theirs: 5..5,
                    kind: ChunkKind::OursOnly,
                    resolution: None,
                },
            ]
        );
        assert_eq!(document.conflict_count(), 0);
        assert_eq!(document.render(&LABELS), "a\nB\nc\nD\ne\nf\n");
    }

    #[test]
    fn markers_without_base_section() {
        let document = merge_texts(Some("a\nb\n"), "a\nx\n", "a\ny\n");
        let labels = MarkerLabels {
            ours: "HEAD",
            base: None,
            theirs: "feature",
        };
        assert_eq!(
            document.render(&labels),
            "a\n<<<<<<< HEAD\nx\n=======\ny\n>>>>>>> feature\n"
        );
    }

    #[test]
    fn markers_follow_crlf_line_endings() {
        let document = merge_texts(Some("a\r\nb\r\n"), "a\r\nx\r\n", "a\r\ny\r\n");
        assert_eq!(
            document.render(&LABELS),
            "a\r\n<<<<<<< ours\r\nx\r\n||||||| base\r\nb\r\n=======\r\ny\r\n>>>>>>> theirs\r\n"
        );
    }

    #[test]
    fn files_added_on_both_sides() {
        let document = merge_texts(None, "a\nb\nc\nd\n", "a\nB\nc\nd\ne\n");
        assert_eq!(document.conflict_count(), 2);
        assert_eq!(
            document.render(&LABELS),
            "a\n<<<<<<< ours\nb\n||||||| base\n=======\nB\n>>>>>>> theirs\nc\nd\n\
             <<<<<<< ours\n||||||| base\n=======\ne\n>>>>>>> theirs\n"
        );

        let mut document = merge_texts(None, "a\n", "a\n\n");
        resolve_whitespace(&mut document);
        assert_eq!(
            document.conflict_count(),
            1,
            "an empty side is not an unchanged side without a base"
        );
    }

    #[test]
    fn whitespace_resolution() {
        let mut document = merge_texts(Some("a\nb  \nc\n"), "a\nb\nc\n", "a\nB  \nc\n");
        resolve_whitespace(&mut document);
        assert_eq!(document.conflict_count(), 0);
        assert_eq!(document.render(&LABELS), "a\nB  \nc\n");

        let mut document = merge_texts(Some("a\nb\nc\n"), "a\nB \nc\n", "a\nB\nc\n");
        resolve_whitespace(&mut document);
        assert_eq!(document.render(&LABELS), "a\nB \nc\n");

        let mut document = merge_texts(Some("a\nb\n"), "a\nb\r\n", "a\nB\n");
        resolve_whitespace(&mut document);
        assert_eq!(document.render(&LABELS), "a\nB\n");

        let mut document = merge_texts(Some("a\nb\n"), "a\n    b\n", "a\nB\n");
        resolve_whitespace(&mut document);
        assert_eq!(document.conflict_count(), 1, "indentation is significant");
    }

    #[test]
    fn word_resolution() {
        let mut document = merge_texts(
            Some("let x = foo(a);\n"),
            "let x = foo(a, b);\n",
            "let y = foo(a);\n",
        );
        resolve_words(&mut document);
        assert_eq!(document.conflict_count(), 0);
        assert_eq!(
            document.chunks()[0]
                .resolution
                .as_ref()
                .map(|resolution| resolution.strategy),
            Some(Strategy::Words)
        );
        assert_eq!(document.render(&LABELS), "let y = foo(a, b);\n");

        let mut document =
            merge_texts(Some("x = 1\ny = 2\n"), "x = 10\ny = 2\n", "x = 1\ny = 20\n");
        resolve_words(&mut document);
        assert_eq!(document.render(&LABELS), "x = 10\ny = 20\n");

        let mut document = merge_texts(Some("value = 1\n"), "value = 2\n", "value = 3\n");
        resolve_words(&mut document);
        assert_eq!(
            document.conflict_count(),
            1,
            "the same word changed differently"
        );

        let mut document = merge_texts(Some("a b\n"), "a x b\n", "a y b\n");
        resolve_words(&mut document);
        assert_eq!(document.conflict_count(), 1, "insertions at the same point");
    }

    #[test]
    fn word_tokens() {
        assert_eq!(
            words("let x_1 = foo(a,  b);\n"),
            [
                "let", " ", "x_1", " ", "=", " ", "foo", "(", "a", ",", "  ", "b", ")", ";", "\n"
            ]
        );
        assert_eq!(words("é\r\n\n"), ["é", "\r", "\n", "\n"]);
    }
}
