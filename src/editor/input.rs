//! Editing operations: keyboard/clipboard intents → document mutations.
//!
//! Every operation records an undo snapshot first, mutates the [`Doc`] in raw
//! source coordinates and re-derives the display map for the touched blocks.
//! Pure logic — no egui geometry here (that lives in `mod.rs`).

use super::doc::{continuation_prefix, prefix_len, BlockKind, Doc, DocPos};
use super::inline::{self, DisplayText};
use super::undo::{EditKind, Snapshot};
use super::EditorWidget;

impl EditorWidget {
    pub(crate) fn block_display(&self, bi: usize) -> DisplayText {
        self.cache
            .get(bi)
            .filter(|cached| cached.text == self.doc.blocks[bi].text)
            .map(|cached| cached.display.clone())
            .unwrap_or_else(|| {
                let block = &self.doc.blocks[bi];
                inline::parse(&block.text, block.kind)
            })
    }

    fn snapshot(&self) -> Snapshot {
        Snapshot {
            text: self.doc.text(),
            cursor: self.cursor,
            anchor: self.anchor,
        }
    }

    pub(crate) fn record(&mut self, kind: EditKind) {
        self.preferred_x = None;
        let snapshot = self.snapshot();
        self.undo.record(snapshot, kind);
    }

    fn restore(&mut self, snapshot: Snapshot) {
        self.doc = Doc::parse(&snapshot.text);
        self.cursor = self.doc.clamp_pos(snapshot.cursor);
        self.anchor = self.doc.clamp_pos(snapshot.anchor);
        self.preedit = None;
        self.dirty = true;
        self.scroll_to_cursor = true;
    }

    /// Normalized selection range, if any.
    pub fn selection(&self) -> Option<(DocPos, DocPos)> {
        if self.cursor == self.anchor {
            None
        } else if self.cursor < self.anchor {
            Some((self.cursor, self.anchor))
        } else {
            Some((self.anchor, self.cursor))
        }
    }

    pub(crate) fn set_cursor(&mut self, pos: DocPos, extend: bool) {
        self.undo.break_coalescing();
        let pos = self.doc.clamp_pos(pos);
        self.cursor = pos;
        if !extend {
            self.anchor = pos;
        }
        self.preedit = None;
    }

    /// Remove the current selection; returns true when one existed.
    pub(crate) fn delete_selection(&mut self) -> bool {
        let Some((a, b)) = self.selection() else {
            return false;
        };
        let cursor = self.doc.delete(a, b);
        self.cursor = self.doc.clamp_pos(cursor);
        self.anchor = self.cursor;
        self.dirty = true;
        true
    }

    /// Insert raw markdown at the cursor (typing, paste, IME commit).
    pub fn insert_text(&mut self, text: &str) {
        self.insert_with(text, EditKind::Typing);
    }

    pub fn insert_with(&mut self, text: &str, kind: EditKind) {
        if text.is_empty() {
            return;
        }
        self.record(kind);
        self.delete_selection();
        let cursor = self.doc.insert(self.cursor, text);
        self.cursor = self.doc.clamp_pos(cursor);
        self.anchor = self.cursor;
        self.preedit = None;
        self.dirty = true;
        self.scroll_to_cursor = true;
    }

    /// Enter: split the block (list/quote continue themselves), or insert a
    /// newline inside code/tables. An empty list item or heading is "exited".
    pub fn enter(&mut self) {
        self.record(EditKind::Structural);
        self.delete_selection();
        let bi = self.cursor.block;
        if bi >= self.doc.blocks.len() {
            return;
        }
        let (kind, text, content_start, content_end) = {
            let block = &self.doc.blocks[bi];
            (
                block.kind,
                block.text.clone(),
                block.content_start(),
                block.content_end(),
            )
        };
        if kind.is_multiline() {
            let cursor = self.doc.insert(self.cursor, "\n");
            self.cursor = cursor;
        } else {
            let body = text[content_start..content_end].trim();
            let exitable = body.is_empty()
                && matches!(
                    kind,
                    BlockKind::ListItem { .. } | BlockKind::Quote | BlockKind::Heading(_)
                );
            if exitable {
                // Empty list item / heading / quote: drop the marker instead of
                // creating yet another empty block (Notion-style).
                let at = self.strip_prefix(bi);
                self.cursor = self.doc.clamp_pos(DocPos::new(bi, at));
            } else {
                let continuation = continuation_prefix(&text, kind).unwrap_or_default();
                let cursor = self.doc.split_block(self.cursor, &continuation);
                self.cursor = self.doc.clamp_pos(cursor);
            }
        }
        self.anchor = self.cursor;
        self.dirty = true;
        self.scroll_to_cursor = true;
    }

    /// Backspace: delete the previous visible character, strip the block
    /// marker at content start, or join with the previous block.
    pub fn backspace(&mut self) {
        if self.selection().is_some() {
            self.record(EditKind::Structural);
            self.delete_selection();
            return;
        }
        let bi = self.cursor.block;
        if bi >= self.doc.blocks.len() {
            return;
        }
        let disp = self.block_display(bi);
        let content_start = self.doc.blocks[bi].content_start();
        if self.cursor.off > content_start {
            let index = disp.disp_index(self.cursor.off);
            if index == 0 {
                return;
            }
            let Some(range) = disp.char_range(index - 1) else {
                return;
            };
            self.record(EditKind::Typing);
            let cursor = self.doc.delete(
                DocPos::new(bi, range.start),
                DocPos::new(bi, range.end),
            );
            self.cursor = self.doc.clamp_pos(cursor);
            self.anchor = self.cursor;
            self.dirty = true;
            return;
        }
        // At content start: strip the block marker when there is one.
        let kind = self.doc.blocks[bi].kind;
        let text = &self.doc.blocks[bi].text;
        let has_marker = prefix_len(text, kind) > 0
            && !matches!(kind, BlockKind::Paragraph | BlockKind::Code | BlockKind::Table);
        match kind {
            BlockKind::Heading(level) if level > 1 => {
                self.record(EditKind::Structural);
                // Demote the heading one level instead of destroying it.
                let text = self.doc.blocks[bi].text.clone();
                let indent = text.len() - text.trim_start().len();
                let old_prefix = prefix_len(&text, kind);
                let mut replacement = text[..indent].to_string();
                replacement.push_str(&"#".repeat(level as usize - 1));
                replacement.push(' ');
                replacement.push_str(&text[old_prefix..]);
                self.doc.replace_block(bi, &replacement);
                self.cursor = self
                    .doc
                    .clamp_pos(DocPos::new(bi, indent + level as usize));
                self.anchor = self.cursor;
                self.dirty = true;
            }
            _ if has_marker => {
                self.record(EditKind::Structural);
                let at = self.strip_prefix(bi);
                self.cursor = self.doc.clamp_pos(DocPos::new(bi, at));
                self.anchor = self.cursor;
                self.dirty = true;
            }
            _ if bi > 0 => {
                self.record(EditKind::Structural);
                let previous_end = self.doc.blocks[bi - 1].content_end();
                self.doc.join_next(bi - 1);
                let cursor = self.doc.clamp_pos(DocPos::new(bi - 1, previous_end));
                self.cursor = cursor;
                self.anchor = self.cursor;
                self.dirty = true;
            }
            _ => {}
        }
    }

    /// Delete: remove the next visible character or join with the next block.
    pub fn delete_forward(&mut self) {
        if self.selection().is_some() {
            self.record(EditKind::Structural);
            self.delete_selection();
            return;
        }
        let bi = self.cursor.block;
        if bi >= self.doc.blocks.len() {
            return;
        }
        let disp = self.block_display(bi);
        if let Some(range) = disp.char_range(disp.disp_index(self.cursor.off)) {
            self.record(EditKind::Typing);
            let cursor = self
                .doc
                .delete(DocPos::new(bi, range.start), DocPos::new(bi, range.end));
            self.cursor = self.doc.clamp_pos(cursor);
            self.anchor = self.cursor;
            self.dirty = true;
        } else if bi + 1 < self.doc.blocks.len() {
            self.record(EditKind::Structural);
            let end = self.doc.blocks[bi].content_end();
            let cursor = self.doc.delete(
                DocPos::new(bi, end),
                DocPos::new(bi + 1, self.doc.blocks[bi + 1].content_start()),
            );
            self.cursor = self.doc.clamp_pos(cursor);
            self.anchor = self.cursor;
            self.dirty = true;
        }
    }

    /// Replace the block prefix (`# `, `- `, `> `) with its indentation only.
    /// Returns the new content start (where the caret belongs).
    fn strip_prefix(&mut self, bi: usize) -> usize {
        let text = self.doc.blocks[bi].text.clone();
        let kind = self.doc.blocks[bi].kind;
        let prefix = prefix_len(&text, kind);
        if prefix == 0 {
            return 0;
        }
        let indent: String = text.chars().take_while(|c| *c == ' ' || *c == '\t').collect();
        let at = indent.len();
        let mut replacement = indent;
        replacement.push_str(&text[prefix..]);
        self.doc.replace_block(bi, &replacement);
        at
    }

    pub fn move_left(&mut self, extend: bool, word: bool) {
        if !extend {
            if let Some((a, b)) = self.selection() {
                self.set_cursor(a, false);
                let _ = b;
                return;
            }
        }
        let bi = self.cursor.block;
        if bi >= self.doc.blocks.len() {
            return;
        }
        let disp = self.block_display(bi);
        let target = if word {
            let word_pos = disp.prev_word_position(self.cursor.off);
            if word_pos == self.cursor.off {
                disp.position_before(self.cursor.off)
            } else {
                Some(word_pos)
            }
        } else if extend {
            disp.char_end_before(self.cursor.off)
        } else {
            disp.position_before(self.cursor.off)
        };
        match target {
            Some(off) if off >= self.doc.blocks[bi].content_start() => {
                self.set_cursor(DocPos::new(bi, off), extend)
            }
            _ if bi > 0 => {
                let prev = bi - 1;
                let end = self.doc.blocks[prev].content_end();
                self.set_cursor(DocPos::new(prev, end), extend);
                self.preferred_x = None;
            }
            _ => self.set_cursor(self.doc.pos_start(), extend),
        }
    }

    pub fn move_right(&mut self, extend: bool, word: bool) {
        if !extend {
            if let Some((_, b)) = self.selection() {
                self.set_cursor(b, false);
                return;
            }
        }
        let bi = self.cursor.block;
        if bi >= self.doc.blocks.len() {
            return;
        }
        let disp = self.block_display(bi);
        let target = if word {
            let word_pos = disp.next_word_position(self.cursor.off);
            if word_pos == self.cursor.off {
                disp.position_after(self.cursor.off)
            } else {
                Some(word_pos)
            }
        } else if extend {
            disp.char_end_after(self.cursor.off)
        } else {
            disp.position_after(self.cursor.off)
        };
        match target {
            Some(off) if off <= self.doc.blocks[bi].content_end() => {
                self.set_cursor(DocPos::new(bi, off), extend)
            }
            _ if bi + 1 < self.doc.blocks.len() => {
                let start = self.doc.blocks[bi + 1].content_start();
                self.set_cursor(DocPos::new(bi + 1, start), extend);
                self.preferred_x = None;
            }
            _ => self.set_cursor(self.doc.pos_end(), extend),
        }
    }

    /// PageUp/PageDown: jump whole blocks keeping the column.
    pub fn move_blocks(&mut self, delta: i32, extend: bool) {
        if self.doc.blocks.is_empty() {
            return;
        }
        let bi = self.cursor.block as i32 + delta;
        let target = bi.clamp(0, self.doc.blocks.len().saturating_sub(1) as i32) as usize;
        let disp = self.block_display(target);
        let column = self.block_display(self.cursor.block).disp_index(self.cursor.off);
        let off = disp.src_at_disp(column.min(disp.chars.len()));
        self.set_cursor(DocPos::new(target, off), extend);
        self.scroll_to_cursor = true;
    }

    /// Home/End: start/end of the caret's visual row (content bounds fallback).
    pub fn home(&mut self, extend: bool) {
        let bi = self.cursor.block;
        if bi >= self.doc.blocks.len() {
            return;
        }
        let disp = self.block_display(bi);
        let off = self
            .row_bounds(bi, &disp)
            .map(|(start, _)| start)
            .unwrap_or_else(|| self.doc.blocks[bi].content_start());
        self.set_cursor(DocPos::new(bi, off), extend);
    }

    pub fn end(&mut self, extend: bool) {
        let bi = self.cursor.block;
        if bi >= self.doc.blocks.len() {
            return;
        }
        let disp = self.block_display(bi);
        let off = self
            .row_bounds(bi, &disp)
            .map(|(_, end)| end)
            .unwrap_or_else(|| self.doc.blocks[bi].content_end());
        self.set_cursor(DocPos::new(bi, off), extend);
    }

    pub fn select_all(&mut self) {
        self.undo.break_coalescing();
        self.anchor = self.doc.pos_start();
        self.cursor = self.doc.pos_end();
    }

    /// Raw markdown of the selection (or an empty string).
    pub fn copy_selection(&self) -> String {
        let Some((a, b)) = self.selection() else {
            return String::new();
        };
        let mut out = String::new();
        for bi in a.block..=b.block.min(self.doc.blocks.len().saturating_sub(1)) {
            let text = &self.doc.blocks[bi].text;
            let start = if bi == a.block { a.off } else { 0 };
            let end = if bi == b.block {
                b.off.min(text.len())
            } else {
                text.len()
            };
            out.push_str(&text[start.min(end)..end]);
        }
        out
    }

    /// Ctrl+B / Ctrl+I / Ctrl+E: toggle a marker pair around the selection.
    pub fn toggle_wrap(&mut self, marker: &str) {
        match self.selection() {
            Some((a, b)) if a.block == b.block => {
                self.record(EditKind::Structural);
                let bi = a.block;
                let text = self.doc.blocks[bi].text.clone();
                let m = marker.len();
                let wrapped = a.off + m <= b.off
                    && text[a.off..].starts_with(marker)
                    && text[..b.off].ends_with(marker);
                if wrapped {
                    self.doc
                        .delete(DocPos::new(bi, b.off - m), DocPos::new(bi, b.off));
                    self.doc
                        .delete(DocPos::new(bi, a.off), DocPos::new(bi, a.off + m));
                    self.cursor = self.doc.clamp_pos(DocPos::new(bi, b.off - 2 * m));
                    self.anchor = self.doc.clamp_pos(DocPos::new(bi, a.off));
                } else {
                    self.doc.insert(DocPos::new(bi, b.off), marker);
                    self.doc.insert(DocPos::new(bi, a.off), marker);
                    self.cursor = self.doc.clamp_pos(DocPos::new(bi, b.off + 2 * m));
                    self.anchor = self.doc.clamp_pos(DocPos::new(bi, a.off));
                }
                self.dirty = true;
            }
            Some((a, b)) => {
                // Multi-block selection: wrap each block's own part.
                self.record(EditKind::Structural);
                for bi in a.block..=b.block.min(self.doc.blocks.len().saturating_sub(1)) {
                    let text = self.doc.blocks[bi].text.clone();
                    let kind = self.doc.blocks[bi].kind;
                    let content_end = self.doc.blocks[bi].content_end();
                    let (start, end) = if bi == a.block {
                        (a.off, content_end)
                    } else if bi == b.block {
                        (prefix_len(&text, kind).min(b.off), b.off)
                    } else {
                        (prefix_len(&text, kind), content_end)
                    };
                    if end > start {
                        self.doc.insert(DocPos::new(bi, end), marker);
                        self.doc.insert(DocPos::new(bi, start), marker);
                    }
                }
                self.cursor = self.doc.pos_end();
                self.anchor = self.doc.pos_start();
                self.dirty = true;
            }
            None => {
                let m = marker.len();
                self.insert_with(&format!("{marker}{marker}"), EditKind::Structural);
                let bi = self.cursor.block;
                let off = self.cursor.off - m;
                self.cursor = self.doc.clamp_pos(DocPos::new(bi, off));
                self.anchor = self.cursor;
            }
        }
    }

    /// Ctrl+K: wrap the selection in a markdown link, caret in the URL slot.
    pub fn insert_link(&mut self) {
        match self.selection() {
            Some((a, b)) if a.block == b.block => {
                self.record(EditKind::Structural);
                self.doc.insert(DocPos::new(b.block, b.off), "]()");
                self.doc.insert(DocPos::new(a.block, a.off), "[");
                self.cursor = self.doc.clamp_pos(DocPos::new(a.block, b.off + 3));
                self.anchor = self.cursor;
                self.dirty = true;
            }
            Some(_) => self.insert_with("[]()", EditKind::Structural),
            None => {
                let at = self.cursor;
                self.insert_with("[]()", EditKind::Structural);
                self.cursor = self
                    .doc
                    .clamp_pos(DocPos::new(at.block, at.off + 3));
                self.anchor = self.cursor;
            }
        }
    }

    /// Tab / Shift+Tab: change list nesting, or indent plain text.
    pub fn indent(&mut self, out: bool) {
        let bi = self.cursor.block;
        if bi >= self.doc.blocks.len() {
            return;
        }
        self.record(EditKind::Structural);
        let text = self.doc.blocks[bi].text.clone();
        if out {
            let removable = text.chars().take_while(|c| *c == ' ').count().min(2);
            if removable > 0 {
                self.doc
                    .delete(DocPos::new(bi, 0), DocPos::new(bi, removable));
                self.cursor = self.doc.clamp_pos(DocPos::new(bi, self.cursor.off.saturating_sub(removable)));
                self.anchor = self.cursor;
                self.dirty = true;
            }
        } else if matches!(
            self.doc.blocks[bi].kind,
            BlockKind::ListItem { .. } | BlockKind::Quote
        ) {
            self.doc.insert(DocPos::new(bi, 0), "  ");
            self.cursor = self.doc.clamp_pos(DocPos::new(bi, self.cursor.off + 2));
            self.anchor = self.cursor;
            self.dirty = true;
        } else {
            self.insert_with("  ", EditKind::Structural);
        }
    }

    /// Toggle the `- [ ]` checkbox of the block under the cursor.
    pub fn toggle_task(&mut self, bi: usize) {
        let is_task = matches!(
            self.doc.blocks.get(bi).map(|b| b.kind),
            Some(BlockKind::ListItem { task: true, .. })
        );
        if !is_task {
            return;
        }
        self.record(EditKind::Structural);
        let text = self.doc.blocks[bi].text.clone();
        if let Some(bracket) = text.find('[') {
            let mark = bracket + 1;
            if mark < text.len() {
                let checked = text.as_bytes()[mark] == b'x' || text.as_bytes()[mark] == b'X';
                let replacement = format!(
                    "{}{}{}",
                    &text[..mark],
                    if checked { ' ' } else { 'x' },
                    &text[mark + 1..]
                );
                self.doc.replace_block(bi, &replacement);
                self.cursor = self.doc.clamp_pos(self.cursor);
                self.anchor = self.cursor;
                self.dirty = true;
            }
        }
    }

    pub fn undo(&mut self) {
        let current = self.snapshot();
        if let Some(previous) = self.undo.undo(current) {
            self.restore(previous);
        }
    }

    pub fn redo(&mut self) {
        let current = self.snapshot();
        if let Some(next) = self.undo.redo(current) {
            self.restore(next);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::editor::EditorWidget;

    fn widget(text: &str) -> EditorWidget {
        let mut widget = EditorWidget::new(egui::Id::new("test"));
        widget.load(text);
        widget
    }

    #[test]
    fn typing_splits_and_joins_blocks() {
        let mut w = widget("prima\nseconda\n");
        w.set_cursor(DocPos::new(0, 5), false);
        w.enter();
        assert_eq!(w.text(), "prima\n\nseconda\n");
        w.insert_text("X");
        assert_eq!(w.text(), "prima\nX\nseconda\n");
        w.backspace();
        w.backspace(); // merge the empty line away
        assert_eq!(w.text(), "prima\nseconda\n");
        assert_eq!(w.cursor, DocPos::new(0, 5));
    }

    #[test]
    fn enter_continues_lists_and_exits_empty_items() {
        let mut w = widget("- voce\n");
        w.set_cursor(DocPos::new(0, w.doc.blocks[0].content_end()), false);
        w.enter();
        assert_eq!(w.text(), "- voce\n- \n");
        assert_eq!(w.cursor.block, 1);
        // Enter on the empty item removes the bullet.
        w.enter();
        assert_eq!(w.text(), "- voce\n\n");

        let mut w = widget("1. uno\n");
        w.set_cursor(DocPos::new(0, w.doc.blocks[0].content_end()), false);
        w.enter();
        assert_eq!(w.text(), "1. uno\n2. \n");
    }

    #[test]
    fn backspace_at_content_start_strips_markers_then_joins() {
        let mut w = widget("- voce\nprima\n");
        w.set_cursor(DocPos::new(0, 2), false);
        w.backspace();
        assert_eq!(w.text(), "voce\nprima\n");
        assert_eq!(w.cursor, DocPos::new(0, 0), "caret keeps its place");
        w.set_cursor(DocPos::new(1, 0), false);
        w.backspace();
        assert_eq!(w.text(), "voceprima\n");
        assert_eq!(w.cursor, DocPos::new(0, 4));
    }

    #[test]
    fn navigation_skips_hidden_markers() {
        let mut w = widget("a**x**b\n");
        w.set_cursor(DocPos::new(0, 0), false);
        w.move_right(false, false);
        assert_eq!(w.cursor.off, 3); // over the opening `**`
        w.move_right(false, false);
        assert_eq!(w.cursor.off, 6); // over the closing `**`
        w.move_left(false, false);
        assert_eq!(w.cursor.off, 3);
    }

    #[test]
    fn selection_delete_and_copy_are_raw_markdown() {
        let mut w = widget("ciao **mondo**\n");
        w.set_cursor(DocPos::new(0, 5), false);
        w.set_cursor(DocPos::new(0, 12), true);
        assert_eq!(w.copy_selection(), "**mondo");
        w.delete_forward();
        assert_eq!(w.text(), "ciao **\n");
    }

    #[test]
    fn toggle_wrap_bold_round_trips() {
        let mut w = widget("grasso\n");
        w.set_cursor(DocPos::new(0, 0), false);
        w.set_cursor(DocPos::new(0, 7), true);
        w.toggle_wrap("**");
        assert_eq!(w.text(), "**grasso**\n");
        w.toggle_wrap("**");
        assert_eq!(w.text(), "grasso\n");
    }

    #[test]
    fn link_insert_puts_the_caret_in_the_url_slot() {
        let mut w = widget("testo\n");
        w.set_cursor(DocPos::new(0, 0), false);
        w.set_cursor(DocPos::new(0, 5), true);
        w.insert_link();
        assert_eq!(w.text(), "[testo]()\n");
        assert_eq!(w.cursor.off, 8); // between `(` and `)`
    }

    #[test]
    fn tab_and_shift_tab_change_nesting() {
        let mut w = widget("- voce\n");
        w.set_cursor(DocPos::new(0, 4), false);
        w.indent(false);
        assert_eq!(w.text(), "  - voce\n");
        w.indent(true);
        assert_eq!(w.text(), "- voce\n");
    }

    #[test]
    fn task_toggle_flips_the_checkbox() {
        let mut w = widget("- [ ] compito\n");
        w.toggle_task(0);
        assert_eq!(w.text(), "- [x] compito\n");
        w.toggle_task(0);
        assert_eq!(w.text(), "- [ ] compito\n");
    }

    #[test]
    fn undo_restores_text_and_cursor() {
        let mut w = widget("base\n");
        w.set_cursor(DocPos::new(0, 4), false);
        w.insert_with("XYZ", EditKind::Structural);
        assert_eq!(w.text(), "baseXYZ\n");
        w.undo();
        assert_eq!(w.text(), "base\n");
        assert_eq!(w.cursor, DocPos::new(0, 4));
        w.redo();
        assert_eq!(w.text(), "baseXYZ\n");
    }

    #[test]
    fn code_blocks_edit_in_place() {
        let mut w = widget("```\nriga\n```\n");
        w.set_cursor(DocPos::new(0, 4), false);
        w.enter();
        assert_eq!(w.text(), "```\n\nriga\n```\n");
        assert_eq!(w.doc.blocks.len(), 1, "code must stay one block");
        w.backspace();
        assert_eq!(w.text(), "```\nriga\n```\n");
    }
}
