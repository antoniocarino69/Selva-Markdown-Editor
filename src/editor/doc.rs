//! Block model for the WYSIWYG editor.
//!
//! The markdown file on disk is the single source of truth: a [`Doc`] is only a
//! structured cache of the file text. `Doc::parse` splits the source into blocks
//! whose `text` fields are *exact* source fragments — concatenating them yields
//! the original bytes (`serialize(parse(x)) == x`), so unedited blocks are saved
//! back byte-identical and no vault can be corrupted by a round-trip.
//!
//! Blocks are line-based: one source line per block, except multi-line
//! constructs (fenced code blocks, tables) which are a single block. Block kinds
//! are derived from the raw text, so "input rules" come for free: typing `# `
//! turns the line into a heading, `- ` into a list item, and so on.

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum BlockKind {
    Paragraph,
    Heading(u8),
    /// `- item` / `1. item`; `task` marks `- [ ]` checkboxes.
    ListItem { ordered: bool, task: bool },
    Quote,
    /// Fenced code block (one or more source lines, fence included).
    Code,
    /// `---`, `***`, `___`
    Rule,
    /// GFM table (contiguous lines containing `|` with a separator row).
    Table,
    /// A line that is exactly one image embed.
    Image,
}

impl BlockKind {
    /// Multi-line blocks edit their raw text in place (newlines never split them).
    pub fn is_multiline(self) -> bool {
        matches!(self, BlockKind::Code | BlockKind::Table)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Block {
    /// Exact source fragment, including the trailing line terminator.
    pub text: String,
    pub kind: BlockKind,
}

/// A position in the document: byte offset inside `blocks[block].text`.
#[derive(Clone, Copy, PartialEq, Eq, Debug, PartialOrd, Ord, Default)]
pub struct DocPos {
    pub block: usize,
    pub off: usize,
}

impl DocPos {
    pub fn new(block: usize, off: usize) -> Self {
        Self { block, off }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Doc {
    pub blocks: Vec<Block>,
}

impl Doc {
    pub fn parse(source: &str) -> Self {
        let mut blocks = Vec::new();
        let lines: Vec<&str> = source.split_inclusive('\n').collect();
        let mut i = 0;
        while i < lines.len() {
            let line = lines[i];
            let content = line_content(line);
            if fence_open(content).is_some() {
                // Fenced code block: consume until the closing fence (or EOF).
                let mut j = i + 1;
                while j < lines.len() && fence_close(content, line_content(lines[j])).is_none() {
                    j += 1;
                }
                if j < lines.len() {
                    j += 1;
                }
                blocks.push(Block {
                    text: lines[i..j].concat(),
                    kind: BlockKind::Code,
                });
                i = j;
            } else if content.contains('|') && i + 1 < lines.len() && is_table_separator(line_content(lines[i + 1]))
            {
                // GFM table: a run of contiguous lines containing `|`.
                let mut j = i;
                while j < lines.len() && line_content(lines[j]).contains('|') {
                    j += 1;
                }
                blocks.push(Block {
                    text: lines[i..j].concat(),
                    kind: BlockKind::Table,
                });
                i = j;
            } else {
                let kind = classify_line(content);
                blocks.push(Block {
                    text: line.to_string(),
                    kind,
                });
                i += 1;
            }
        }
        Self { blocks }
    }

    pub fn text(&self) -> String {
        self.blocks.iter().map(|b| b.text.as_str()).collect()
    }

    pub fn pos_start(&self) -> DocPos {
        DocPos::new(0, self.blocks.first().map_or(0, |b| b.content_start()))
    }

    pub fn pos_end(&self) -> DocPos {
        match self.blocks.len() {
            0 => DocPos::default(),
            n => DocPos::new(n - 1, self.blocks[n - 1].content_end()),
        }
    }

    /// Clamp a position to raw block-text bounds (prefix included).
    fn clamp_raw(&self, pos: DocPos) -> DocPos {
        if self.blocks.is_empty() {
            return DocPos::default();
        }
        let bi = pos.block.min(self.blocks.len() - 1);
        DocPos::new(bi, pos.off.min(self.blocks[bi].text.len()))
    }

    /// Clamp a position into valid bounds (after edits may shift ranges).
    pub fn clamp_pos(&self, pos: DocPos) -> DocPos {
        let Some(block) = self.blocks.get(pos.block.min(self.blocks.len().saturating_sub(1))) else {
            return DocPos::default();
        };
        let bi = pos.block.min(self.blocks.len().saturating_sub(1));
        let off = pos.off.clamp(block.content_start().min(block.content_end()), block.content_end());
        DocPos::new(bi, off)
    }

    /// Insert raw markdown at `pos`. Multi-line inserts split single-line
    /// blocks; multi-line blocks (code/tables) take the text as-is.
    /// Returns the cursor position after the inserted text.
    pub fn insert(&mut self, pos: DocPos, inserted: &str) -> DocPos {
        if self.blocks.is_empty() {
            self.blocks.push(Block {
                text: String::new(),
                kind: BlockKind::Paragraph,
            });
        }
        let pos = self.clamp_raw(pos);
        let bi = pos.block;
        if self.blocks[bi].kind.is_multiline() || !inserted.contains('\n') {
            self.blocks[bi].text.insert_str(pos.off, inserted);
            self.blocks[bi].reclassify();
            return DocPos::new(bi, pos.off + inserted.len());
        }
        // Split the inserted text at line boundaries into new blocks: every
        // newline of the insert terminates a line, and the old tail joins the
        // last inserted segment (keeping its own terminator).
        let tail = self.blocks[bi].text.split_off(pos.off);
        let mut segments = inserted.split('\n');
        let first = segments.next().unwrap_or("");
        self.blocks[bi].text.push_str(first);
        self.blocks[bi].text.push('\n');
        self.blocks[bi].reclassify();
        let mut last_index = bi;
        for segment in segments {
            let mut text = String::from(segment);
            text.push('\n');
            self.blocks.insert(
                last_index + 1,
                Block {
                    text,
                    kind: BlockKind::Paragraph,
                },
            );
            last_index += 1;
            self.blocks[last_index].reclassify();
        }
        let last = &mut self.blocks[last_index];
        last.text.pop(); // the synthetic terminator, replaced by the tail
        last.text.push_str(&tail);
        last.reclassify();
        DocPos::new(last_index, self.blocks[last_index].text.len() - tail.len())
    }

    /// Delete `start..end` (normalized). Returns the cursor at the deletion.
    pub fn delete(&mut self, start: DocPos, end: DocPos) -> DocPos {
        let (a, b) = if start <= end { (start, end) } else { (end, start) };
        if self.blocks.is_empty() {
            return DocPos::default();
        }
        let a = self.clamp_raw(a);
        let b = self.clamp_raw(b);
        if a == b {
            return a;
        }
        if a.block == b.block {
            self.blocks[a.block].text.replace_range(a.off..b.off, "");
            self.blocks[a.block].reclassify();
            return self.clamp_pos(a);
        }
        let tail = self.blocks[b.block].text[b.off..].to_string();
        self.blocks[a.block].text.truncate(a.off);
        self.blocks[a.block].text.push_str(&tail);
        self.blocks[a.block].reclassify();
        self.blocks.drain(a.block + 1..=b.block);
        self.clamp_pos(a)
    }

    /// Split the block at `pos`: the right part becomes a new block prefixed
    /// with `continuation` (e.g. the next bullet). Returns the cursor placed at
    /// the start of the new block's own content.
    pub fn split_block(&mut self, pos: DocPos, continuation: &str) -> DocPos {
        let pos = self.clamp_pos(pos);
        let bi = pos.block;
        let tail = self.blocks[bi].text.split_off(pos.off);
        // The terminator travels with the tail: give the left part a new one.
        self.blocks[bi].text.push('\n');
        self.blocks[bi].reclassify();
        let mut text = String::from(continuation);
        let content_at = text.len();
        text.push_str(&tail);
        self.blocks.insert(
            bi + 1,
            Block {
                text,
                kind: BlockKind::Paragraph,
            },
        );
        self.blocks[bi + 1].reclassify();
        DocPos::new(bi + 1, content_at)
    }

    /// Merge block `bi` with the following one (delete between them).
    pub fn join_next(&mut self, bi: usize) {
        if bi + 1 >= self.blocks.len() {
            return;
        }
        let end = self.blocks[bi].content_end();
        self.delete(DocPos::new(bi, end), DocPos::new(bi + 1, 0));
    }

    pub fn replace_block(&mut self, bi: usize, text: &str) {
        if let Some(block) = self.blocks.get_mut(bi) {
            block.text = text.to_string();
            block.reclassify();
        }
    }
}

impl Block {
    pub fn reclassify(&mut self) {
        self.kind = classify_block(&self.text);
    }

    /// Length of the trailing line terminator (`"\r\n"`, `"\n"` or none).
    pub fn terminator_len(&self) -> usize {
        if self.text.ends_with("\r\n") {
            2
        } else if self.text.ends_with('\n') {
            1
        } else {
            0
        }
    }

    pub fn content_end(&self) -> usize {
        self.text.len() - self.terminator_len()
    }

    /// Byte offset where the editable text starts, i.e. after the hidden
    /// block prefix (`# `, `- `, `> ` …).
    pub fn content_start(&self) -> usize {
        prefix_len(&self.text, self.kind)
    }
}

/// Line without its trailing `\r\n` / `\n`.
pub fn line_content(line: &str) -> &str {
    line.strip_suffix("\r\n")
        .or_else(|| line.strip_suffix('\n'))
        .unwrap_or(line)
}

fn classify_block(text: &str) -> BlockKind {
    let content = line_content(text);
    if fence_open(content).is_some() {
        return BlockKind::Code;
    }
    let lines: Vec<&str> = text.split('\n').map(line_content).collect();
    if lines.len() >= 2
        && lines[0].contains('|')
        && is_table_separator(lines[1])
        && lines.iter().all(|l| l.contains('|'))
    {
        return BlockKind::Table;
    }
    classify_line(content)
}

fn classify_line(content: &str) -> BlockKind {
    let trimmed = content.trim_start();
    let indent = content.len() - trimmed.len();
    if indent <= 3 {
        if let Some(level) = atx_heading(trimmed) {
            return BlockKind::Heading(level);
        }
        if is_rule(trimmed) {
            return BlockKind::Rule;
        }
        if trimmed.starts_with('>') {
            return BlockKind::Quote;
        }
    }
    if let Some((ordered, task)) = list_marker(trimmed) {
        return BlockKind::ListItem { ordered, task };
    }
    if is_image_line(trimmed) {
        return BlockKind::Image;
    }
    BlockKind::Paragraph
}

/// `# ` … `###### ` heading level, if this is an ATX heading.
fn atx_heading(trimmed: &str) -> Option<u8> {
    let hashes = trimmed.bytes().take_while(|b| *b == b'#').count();
    if !(1..=6).contains(&hashes) {
        return None;
    }
    match trimmed.as_bytes().get(hashes) {
        None => Some(hashes as u8),
        Some(b' ') | Some(b'\t') => Some(hashes as u8),
        _ => None,
    }
}

fn is_rule(trimmed: &str) -> bool {
    let bytes = trimmed.as_bytes();
    if bytes.len() < 3 {
        return false;
    }
    let marker = bytes[0];
    if !matches!(marker, b'-' | b'*' | b'_') {
        return false;
    }
    let mut count = 0;
    for b in bytes {
        match b {
            b' ' | b'\t' => {}
            b if *b == marker => count += 1,
            _ => return false,
        }
    }
    count >= 3
}

/// `(ordered, task)` when `trimmed` starts with a list marker.
fn list_marker(trimmed: &str) -> Option<(bool, bool)> {
    let bytes = trimmed.as_bytes();
    let mut i = 0;
    let ordered = bytes.first()?.is_ascii_digit();
    if ordered {
        while i < bytes.len() && bytes[i].is_ascii_digit() && i < 9 {
            i += 1;
        }
        if !matches!(bytes.get(i), Some(b'.') | Some(b')')) {
            return None;
        }
        i += 1;
    } else {
        if !matches!(bytes.first(), Some(b'-') | Some(b'*') | Some(b'+')) {
            return None;
        }
        i = 1;
    }
    // The marker must be followed by whitespace (or be alone on the line).
    if i < bytes.len() && !matches!(bytes[i], b' ' | b'\t') {
        return None;
    }
    let mut task = false;
    let rest = &trimmed[i..];
    let after_ws = rest.trim_start();
    if after_ws.len() >= 3
        && after_ws.starts_with('[')
        && matches!(after_ws.as_bytes()[1], b' ' | b'x' | b'X')
        && after_ws.as_bytes()[2] == b']'
    {
        task = true;
    }
    Some((ordered, task))
}

fn is_image_line(trimmed: &str) -> bool {
    if let Some(rest) = trimmed.strip_prefix("![[").and_then(|r| r.strip_suffix("]]")) {
        return !rest.is_empty() && !rest.contains("[[");
    }
    if trimmed.starts_with("![") {
        if let Some(close) = trimmed.find("](") {
            return trimmed.ends_with(')') && close > 2;
        }
    }
    false
}

fn fence_open(content: &str) -> Option<(char, usize, &str)> {
    let trimmed = content.trim_start();
    let marker = *trimmed.as_bytes().first()?;
    if marker != b'`' && marker != b'~' {
        return None;
    }
    let run = trimmed.bytes().take_while(|b| *b == marker).count();
    if run < 3 {
        return None;
    }
    let info = &trimmed[run..];
    // Backtick fences cannot carry backticks in the info string.
    if marker == b'`' && info.contains('`') {
        return None;
    }
    Some((marker as char, run, info))
}

fn fence_close(open: &str, candidate: &str) -> Option<()> {
    let (marker, run, _) = fence_open(open)?;
    let trimmed = candidate.trim_start();
    let count = trimmed.bytes().take_while(|b| *b == marker as u8).count();
    if count >= run && trimmed[count..].trim().is_empty() {
        Some(())
    } else {
        None
    }
}

fn is_table_separator(line: &str) -> bool {
    let trimmed = line.trim();
    if !trimmed.contains('|') || !trimmed.contains('-') {
        return false;
    }
    trimmed
        .bytes()
        .all(|b| matches!(b, b'|' | b'-' | b' ' | b'\t' | b':'))
}

/// Byte length of the hidden block prefix (`# `, `- [x] `, `> ` …).
pub fn prefix_len(text: &str, kind: BlockKind) -> usize {
    let content = line_content(text);
    let trimmed = content.trim_start();
    let indent = content.len() - trimmed.len();
    match kind {
        BlockKind::Heading(level) => {
            indent + level as usize + trailing_ws_len(&trimmed[level as usize..])
        }
        BlockKind::ListItem { .. } => {
            let Some((_, task)) = list_marker(trimmed) else {
                return 0;
            };
            let bytes = trimmed.as_bytes();
            let mut i = 1;
            if bytes[0].is_ascii_digit() {
                while i < bytes.len() && bytes[i].is_ascii_digit() {
                    i += 1;
                }
                i += 1; // the '.' or ')'
            }
            i += trailing_ws_len(&trimmed[i..]);
            if task {
                let rest = &trimmed[i..];
                if let Some(close) = rest.find(']').map(|c| c + 1) {
                    i += close;
                    i += trailing_ws_len(&trimmed[i..]);
                }
            }
            indent + i
        }
        BlockKind::Quote => {
            let mut i = 0;
            let bytes = trimmed.as_bytes();
            while i < bytes.len() && bytes[i] == b'>' {
                i += 1;
                if matches!(bytes.get(i), Some(b' ')) {
                    i += 1;
                }
            }
            indent + i
        }
        _ => 0,
    }
}

fn trailing_ws_len(text: &str) -> usize {
    text.len() - text.trim_start_matches([' ', '\t']).len()
}

/// Marker that `Enter` should repeat on the continuation line, or `None`.
pub fn continuation_prefix(text: &str, kind: BlockKind) -> Option<String> {
    let content = line_content(text);
    let trimmed = content.trim_start();
    let indent = &content[..content.len() - trimmed.len()];
    match kind {
        BlockKind::ListItem { ordered, task } => {
            let marker_end = if ordered {
                let digits: String = trimmed.chars().take_while(|c| c.is_ascii_digit()).collect();
                let sep = trimmed[digits.len()..].chars().next()?;
                let number: u64 = digits.parse().ok()?;
                format!("{}{}{} ", indent, number + 1, sep)
            } else {
                let bullet = trimmed.chars().next()?;
                format!("{indent}{bullet} ")
            };
            Some(if task {
                format!("{marker_end}[ ] ")
            } else {
                marker_end
            })
        }
        BlockKind::Quote => {
            let markers: String = trimmed.chars().take_while(|c| *c == '>').collect();
            if markers.is_empty() {
                None
            } else {
                Some(format!("{indent}{markers} "))
            }
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn corpus() -> Vec<String> {
        vec![
            String::new(),
            "\n".into(),
            "# Titolo\n\nTesto semplice.\n".into(),
            "No newline at end".into(),
            "---\ntitle: front matter\ntags: [a, b]\n---\n\n# Dopo\n".into(),
            "- uno\n- due\n  - annidato\n\n1. primo\n2. secondo\n".into(),
            "> ciao\n> > annidato\n\ncontinua\n".into(),
            "```rust\nfn main() {\n    println!(\"ciao\");\n}\n```\n".into(),
            "```\nsenza linguaggio\n```\n".into(),
            "| a | b |\n| --- | ---: |\n| 1 | 2 |\n".into(),
            "Testo con **grassetto**, *corsivo*, `codice` e [[link|alias]].\n".into(),
            "![[immagine.png]]\n".into(),
            "Riga con spazi    finali   \n\n\tTab iniziale\n".into(),
            "Riga Windows\r\nseconda riga\r\n".into(),
            "Caratteri speciali: àèéòù ñ 漢字 🌿\n".into(),
            "   # non heading\n###### sei\n####### sette\n".into(),
            "- [ ] task aperta\n- [x] task chiusa\n".into(),
            "3) lista con parentesi\n\n***\n___\n".into(),
            "```non chiuso\ntesto dentro\n".into(),
            "a\n\n\n\nb\n".into(),
        ]
    }

    #[test]
    fn round_trip_is_byte_identical() {
        for source in corpus() {
            let doc = Doc::parse(&source);
            assert_eq!(doc.text(), source, "round-trip changed bytes for {source:?}");
            let again = Doc::parse(&doc.text());
            assert_eq!(again, doc, "re-parse diverged for {source:?}");
        }
    }

    #[test]
    fn block_kinds_are_classified() {
        let doc = Doc::parse(
            "# H\nplain\n- item\n1. ord\n> quote\n```\ncode\n```\n---\n| a |\n| - |\n![[i.png]]\n",
        );
        let kinds: Vec<BlockKind> = doc.blocks.iter().map(|b| b.kind).collect();
        assert_eq!(kinds[0], BlockKind::Heading(1));
        assert_eq!(kinds[1], BlockKind::Paragraph);
        assert_eq!(
            kinds[2],
            BlockKind::ListItem {
                ordered: false,
                task: false
            }
        );
        assert_eq!(
            kinds[3],
            BlockKind::ListItem {
                ordered: true,
                task: false
            }
        );
        assert_eq!(kinds[4], BlockKind::Quote);
        assert_eq!(kinds[5], BlockKind::Code);
        assert_eq!(kinds[6], BlockKind::Rule);
        assert_eq!(kinds[7], BlockKind::Table);
        assert_eq!(kinds[8], BlockKind::Image);
    }

    #[test]
    fn prefix_lengths_skip_hidden_markers() {
        let cases = [
            ("# Titolo\n", BlockKind::Heading(1), 2),
            ("###  Titolo\n", BlockKind::Heading(3), 5),
            ("- voce\n", BlockKind::ListItem { ordered: false, task: false }, 2),
            ("- [x] voce\n", BlockKind::ListItem { ordered: false, task: true }, 6),
            ("12. voce\n", BlockKind::ListItem { ordered: true, task: false }, 4),
            ("> ciao\n", BlockKind::Quote, 2),
            (">> ciao\n", BlockKind::Quote, 3),
            ("plain\n", BlockKind::Paragraph, 0),
        ];
        for (text, kind, expected) in cases {
            assert_eq!(prefix_len(text, kind), expected, "{text:?}");
        }
    }

    #[test]
    fn insert_splits_single_line_blocks_and_keeps_multiline_intact() {
        let mut doc = Doc::parse("prima\nseconda\n");
        let end = doc.blocks[0].content_end();
        let cursor = doc.insert(DocPos::new(0, end), "A\nB");
        assert_eq!(doc.text(), "primaA\nB\nseconda\n");
        assert_eq!(cursor, DocPos::new(1, 1));
        assert_eq!(doc.blocks[0].text, "primaA\n");
        assert_eq!(doc.blocks[1].text, "B\n");

        let mut code = Doc::parse("```\nriga\n```\n");
        assert_eq!(code.blocks.len(), 1);
        let cursor = code.insert(DocPos::new(0, 5), "a\nb");
        assert_eq!(code.blocks.len(), 1, "code blocks must not split");
        assert_eq!(code.text(), "```\nra\nbiga\n```\n");
        assert_eq!(cursor, DocPos::new(0, 8));
    }

    #[test]
    fn delete_joins_blocks_without_losing_bytes() {
        let mut doc = Doc::parse("prima\nseconda\nterza\n");
        // Delete from the middle of block 0 to the middle of block 2.
        let cursor = doc.delete(DocPos::new(0, 2), DocPos::new(2, 3));
        assert_eq!(doc.text(), "prza\n");
        assert_eq!(cursor, DocPos::new(0, 2));
        assert_eq!(doc.blocks.len(), 1);
    }

    #[test]
    fn delete_inside_one_block_reclassifies() {
        let mut doc = Doc::parse("# Titolo\n");
        let end = doc.blocks[0].content_end();
        doc.delete(DocPos::new(0, 0), DocPos::new(0, 2));
        assert_eq!(doc.text(), "Titolo\n");
        assert_eq!(doc.blocks[0].kind, BlockKind::Paragraph);
        assert_eq!(doc.blocks[0].content_start(), 0);
        let _ = end;
    }

    #[test]
    fn split_block_carries_the_continuation() {
        let mut doc = Doc::parse("- voce\n");
        let cursor = doc.split_block(DocPos::new(0, 5), "- ");
        assert_eq!(doc.text(), "- voc\n- e\n");
        assert_eq!(cursor, DocPos::new(1, 2));
        assert_eq!(
            doc.blocks[1].kind,
            BlockKind::ListItem {
                ordered: false,
                task: false
            }
        );
    }

    #[test]
    fn join_next_merges_lines() {
        let mut doc = Doc::parse("a\nb\n");
        doc.join_next(0);
        assert_eq!(doc.text(), "ab\n");
        assert_eq!(doc.blocks.len(), 1);
    }

    #[test]
    fn continuation_prefixes() {
        assert_eq!(
            continuation_prefix("- voce\n", BlockKind::ListItem { ordered: false, task: false }),
            Some("- ".into())
        );
        assert_eq!(
            continuation_prefix("3. voce\n", BlockKind::ListItem { ordered: true, task: false }),
            Some("4. ".into())
        );
        assert_eq!(
            continuation_prefix("- [ ] voce\n", BlockKind::ListItem { ordered: false, task: true }),
            Some("- [ ] ".into())
        );
        assert_eq!(
            continuation_prefix("> ciao\n", BlockKind::Quote),
            Some("> ".into())
        );
        assert_eq!(continuation_prefix("# H\n", BlockKind::Heading(1)), None);
    }
}
