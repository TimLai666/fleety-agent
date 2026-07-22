//! Cursor-aware single-line editor shared by the TUI input boxes.
//!
//! The cursor is an **extended grapheme cluster index** (not bytes, Unicode
//! scalars, or columns). Every mutation shares the same segmentation boundary,
//! so combining marks, emoji modifiers, flags, and ZWJ families are never
//! split. Display math uses terminal columns measured through ratatui's
//! `Line::width`.

use ratatui::text::Line;
use unicode_segmentation::UnicodeSegmentation;

fn grapheme_width(grapheme: &str) -> usize {
    Line::from(grapheme).width()
}

/// Single-line text buffer with a movable cursor.
#[derive(Debug, Default, Clone)]
pub struct LineEditor {
    text: String,
    /// Cursor as a grapheme index, 0..=grapheme count.
    cursor: usize,
}

impl LineEditor {
    fn grapheme_count(&self) -> usize {
        self.text.graphemes(true).count()
    }

    fn grapheme_offsets(&self) -> Vec<usize> {
        self.text
            .grapheme_indices(true)
            .map(|(byte, _)| byte)
            .chain(std::iter::once(self.text.len()))
            .collect()
    }

    /// Byte offset of the cursor (text.len() when the cursor is at the end).
    fn byte_offset(&self) -> usize {
        self.text
            .grapheme_indices(true)
            .nth(self.cursor)
            .map(|(byte, _)| byte)
            .unwrap_or(self.text.len())
    }

    /// Place the cursor at the first complete grapheme ending at or after a
    /// byte position. Inserted combining/ZWJ content can merge with an adjacent
    /// cluster, so simply adding a scalar count would create an interior cursor.
    fn set_cursor_after_byte(&mut self, byte: usize) {
        self.cursor = self
            .text
            .grapheme_indices(true)
            .position(|(start, grapheme)| start + grapheme.len() >= byte)
            .map(|index| index + 1)
            .unwrap_or_else(|| self.grapheme_count());
    }

    pub fn insert(&mut self, c: char) {
        let b = self.byte_offset();
        self.text.insert(b, c);
        self.set_cursor_after_byte(b + c.len_utf8());
    }

    /// Insert a whole string at the cursor (paste path).
    pub fn insert_str(&mut self, s: &str) {
        if s.is_empty() {
            return;
        }
        let b = self.byte_offset();
        self.text.insert_str(b, s);
        self.set_cursor_after_byte(b + s.len());
    }

    /// Delete the grapheme before the cursor.
    pub fn backspace(&mut self) {
        if self.cursor == 0 {
            return;
        }
        let offsets = self.grapheme_offsets();
        let end = offsets[self.cursor];
        let start = offsets[self.cursor - 1];
        self.text.replace_range(start..end, "");
        self.cursor -= 1;
    }

    /// Delete the grapheme under the cursor.
    pub fn delete(&mut self) {
        let offsets = self.grapheme_offsets();
        if self.cursor + 1 < offsets.len() {
            self.text
                .replace_range(offsets[self.cursor]..offsets[self.cursor + 1], "");
        }
    }

    pub fn left(&mut self) {
        self.cursor = self.cursor.saturating_sub(1);
    }

    pub fn right(&mut self) {
        if self.cursor < self.grapheme_count() {
            self.cursor += 1;
        }
    }

    /// Move to the start of the current line (line-relative; on a single-line
    /// buffer this is the absolute start).
    pub fn home(&mut self) {
        let graphemes: Vec<&str> = self.text.graphemes(true).collect();
        let mut i = self.cursor.min(graphemes.len());
        while i > 0 && !graphemes[i - 1].contains('\n') {
            i -= 1;
        }
        self.cursor = i;
    }

    /// Move to the end of the current line (line-relative; on a single-line
    /// buffer this is the absolute end).
    pub fn end(&mut self) {
        let graphemes: Vec<&str> = self.text.graphemes(true).collect();
        let mut i = self.cursor.min(graphemes.len());
        while i < graphemes.len() && !graphemes[i].contains('\n') {
            i += 1;
        }
        self.cursor = i;
    }

    /// Insert a line break at the cursor (multi-line composition). Grapheme-index
    /// and UTF-8 boundary guarantees are the same as any other insert.
    pub fn insert_newline(&mut self) {
        self.insert('\n');
    }

    /// Number of logical lines: 1 plus the count of embedded newlines.
    pub fn line_count(&self) -> usize {
        self.text.split('\n').count()
    }

    /// The cursor's position as `(row, display_col)`: `row` counts newlines
    /// before the cursor; `display_col` is the terminal-column width of the
    /// current line up to the cursor (CJK fullwidth counts as two).
    pub fn cursor_row_col(&self) -> (usize, usize) {
        let mut row = 0;
        let mut col = 0;
        for (i, grapheme) in self.text.graphemes(true).enumerate() {
            if i == self.cursor {
                break;
            }
            if grapheme.contains('\n') {
                row += 1;
                col = 0;
            } else {
                col += grapheme_width(grapheme);
            }
        }
        (row, col)
    }

    /// Place the cursor at (or just before) `target_col` display columns into
    /// `target_row`, clamped to the row's end. Used by `up`/`down`.
    fn set_cursor_row_col(&mut self, target_row: usize, target_col: usize) {
        let graphemes: Vec<&str> = self.text.graphemes(true).collect();
        let mut i = 0;
        let mut row = 0;
        while i < graphemes.len() && row < target_row {
            if graphemes[i].contains('\n') {
                row += 1;
            }
            i += 1;
        }
        let mut col = 0;
        while i < graphemes.len() && !graphemes[i].contains('\n') && col < target_col {
            col += grapheme_width(graphemes[i]);
            i += 1;
        }
        self.cursor = i;
    }

    /// Move the cursor up one line, keeping the display column where possible.
    pub fn up(&mut self) {
        let (row, col) = self.cursor_row_col();
        if row == 0 {
            return;
        }
        self.set_cursor_row_col(row - 1, col);
    }

    /// Move the cursor down one line, keeping the display column where possible.
    pub fn down(&mut self) {
        let (row, col) = self.cursor_row_col();
        if row + 1 >= self.line_count() {
            return;
        }
        self.set_cursor_row_col(row + 1, col);
    }

    /// Jump to the start of the previous word (whitespace-delimited).
    pub fn word_left(&mut self) {
        let graphemes: Vec<&str> = self.text.graphemes(true).collect();
        let mut i = self.cursor.min(graphemes.len());
        while i > 0 && graphemes[i - 1].chars().all(char::is_whitespace) {
            i -= 1;
        }
        while i > 0 && !graphemes[i - 1].chars().all(char::is_whitespace) {
            i -= 1;
        }
        self.cursor = i;
    }

    /// Jump past the current word to the start of the next (or the end).
    pub fn word_right(&mut self) {
        let graphemes: Vec<&str> = self.text.graphemes(true).collect();
        let n = graphemes.len();
        let mut i = self.cursor.min(n);
        while i < n && !graphemes[i].chars().all(char::is_whitespace) {
            i += 1;
        }
        while i < n && graphemes[i].chars().all(char::is_whitespace) {
            i += 1;
        }
        self.cursor = i;
    }

    /// Take the text out (submit path), leaving an empty editor.
    pub fn take(&mut self) -> String {
        self.cursor = 0;
        std::mem::take(&mut self.text)
    }

    /// Replace the content (prefill path); cursor moves to the end.
    pub fn set_text(&mut self, text: String) {
        self.cursor = text.graphemes(true).count();
        self.text = text;
    }

    // Accessors used by the editors' tests and available to future callers of
    // this small reusable editor; allow(dead_code) so the bin build (which only
    // exercises some paths) doesn't warn.
    #[allow(dead_code)]
    pub fn text(&self) -> &str {
        &self.text
    }

    #[allow(dead_code)]
    pub fn is_empty(&self) -> bool {
        self.text.is_empty()
    }

    /// Cursor position in display columns from the start of the text. For
    /// callers that render the whole text after a prefix (config rows).
    #[allow(dead_code)]
    pub fn cursor_col(&self) -> usize {
        self.text
            .graphemes(true)
            .take(self.cursor)
            .map(grapheme_width)
            .sum()
    }

    /// The slice of text to show in a box `width` columns wide, plus the
    /// cursor's column within that slice. When the text overflows, the window
    /// scrolls just enough to keep the cursor strictly inside (never on the
    /// border), starting and ending on grapheme boundaries so a user-perceived
    /// character is never half-shown.
    pub fn display_window(&self, width: usize) -> (&str, u16) {
        if width == 0 {
            return ("", 0);
        }
        let graphemes: Vec<(usize, usize)> = self
            .text
            .grapheme_indices(true)
            .map(|(byte, grapheme)| (byte, grapheme_width(grapheme)))
            .collect();
        let cursor_col: usize = graphemes.iter().take(self.cursor).map(|&(_, w)| w).sum();
        let total: usize = graphemes.iter().map(|&(_, w)| w).sum();
        // `<` not `<=`: the cursor needs its own column when sitting past the
        // last grapheme.
        if total < width {
            return (&self.text, cursor_col as u16);
        }
        // Scroll right until the cursor column fits in [0, width-1].
        let min_start = cursor_col.saturating_sub(width - 1);
        let mut start_idx = graphemes.len();
        let mut start_col = total;
        let mut acc = 0;
        for (i, &(_, w)) in graphemes.iter().enumerate() {
            if acc >= min_start {
                start_idx = i;
                start_col = acc;
                break;
            }
            acc += w;
        }
        // Fill the window with whole graphemes only.
        let mut end_idx = start_idx;
        let mut used = 0;
        for &(_, w) in graphemes.iter().skip(start_idx) {
            if used + w > width {
                break;
            }
            used += w;
            end_idx += 1;
        }
        let start_b = graphemes
            .get(start_idx)
            .map(|&(b, _)| b)
            .unwrap_or(self.text.len());
        let end_b = graphemes
            .get(end_idx)
            .map(|&(b, _)| b)
            .unwrap_or(self.text.len());
        (
            &self.text[start_b..end_b],
            u16::try_from(cursor_col - start_col).unwrap_or(u16::MAX),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ed(s: &str) -> LineEditor {
        let mut e = LineEditor::default();
        e.set_text(s.to_string());
        e
    }

    #[test]
    fn insert_and_delete_ascii() {
        let mut e = LineEditor::default();
        e.insert('a');
        e.insert('c');
        e.left();
        e.insert('b');
        assert_eq!(e.text(), "abc");
        e.backspace(); // removes the 'b' just typed
        assert_eq!(e.text(), "ac");
        e.delete(); // removes 'c' under the cursor
        assert_eq!(e.text(), "a");
        e.delete(); // cursor at end: no-op
        assert_eq!(e.text(), "a");
        e.home();
        e.backspace(); // cursor at start: no-op
        assert_eq!(e.text(), "a");
    }

    #[test]
    fn cjk_mixed_editing_stays_on_char_boundaries() {
        let mut e = ed("a漢b");
        // end→left twice: cursor between 'a' and '漢'.
        e.left();
        e.left();
        e.insert('字');
        assert_eq!(e.text(), "a字漢b");
        e.delete(); // deletes 漢 (a full char, not a byte)
        assert_eq!(e.text(), "a字b");
        e.backspace(); // deletes 字
        assert_eq!(e.text(), "ab");
        // Right past the end is clamped.
        e.end();
        e.right();
        e.insert('!');
        assert_eq!(e.text(), "ab!");
    }

    #[test]
    fn extended_graphemes_move_and_delete_as_indivisible_user_characters() {
        let mut e = ed("e\u{301}👍🏽👨‍👩‍👧‍👦🇹🇼");
        e.backspace();
        assert_eq!(e.text(), "e\u{301}👍🏽👨‍👩‍👧‍👦", "flag removed atomically");
        e.left();
        e.delete();
        assert_eq!(e.text(), "e\u{301}👍🏽", "ZWJ family removed atomically");
        e.backspace();
        assert_eq!(e.text(), "e\u{301}", "skin-tone emoji removed atomically");
        e.backspace();
        assert!(e.text().is_empty(), "combining sequence removed atomically");
    }

    #[test]
    fn grapheme_paste_navigation_and_window_share_the_same_boundaries() {
        let mut e = ed("A🇹🇼B");
        e.left();
        e.left();
        e.insert_str("e\u{301}👍🏽");
        assert_eq!(e.text(), "Ae\u{301}👍🏽🇹🇼B");
        e.left();
        e.backspace();
        assert_eq!(e.text(), "A👍🏽🇹🇼B", "combining grapheme deleted whole");
        let (view, _) = e.display_window(4);
        assert!(!view.starts_with('\u{301}'));
        assert!(!view.contains('🏽') || view.contains("👍🏽"));
    }

    #[test]
    fn multiline_grapheme_cursor_columns_use_rendered_cluster_width() {
        let mut e = ed("e\u{301}x\n👍🏽界");
        assert_eq!(e.cursor_row_col(), (1, 4));
        e.home();
        assert_eq!(e.cursor_row_col(), (1, 0));
        e.up();
        assert_eq!(e.cursor_row_col(), (0, 0));
        e.end();
        assert_eq!(e.cursor_row_col(), (0, 2));
    }

    #[test]
    fn insert_str_pastes_at_cursor() {
        let mut e = ed("你好");
        e.left();
        e.insert_str("wide 界");
        assert_eq!(e.text(), "你wide 界好");
        // Cursor advanced past the paste: typing lands right after it.
        e.insert('X');
        assert_eq!(e.text(), "你wide 界X好");
    }

    #[test]
    fn take_and_set_text() {
        let mut e = ed("送出");
        assert!(!e.is_empty());
        assert_eq!(e.take(), "送出");
        assert!(e.is_empty());
        // After take, editing starts fresh at position 0.
        e.insert('a');
        assert_eq!(e.text(), "a");
        e.set_text("預填值".to_string());
        e.backspace(); // cursor is at the end after set_text
        assert_eq!(e.text(), "預填");
    }

    #[test]
    fn word_jumps_over_whitespace_runs() {
        let mut e = ed("foo  bar 漢字");
        assert_eq!(e.cursor_col(), 13); // 9 ascii cols + 2*2 CJK
        e.word_left(); // → start of 漢字
        e.insert('|');
        assert_eq!(e.text(), "foo  bar |漢字");
        e.backspace();
        e.word_left(); // → start of bar
        e.word_left(); // → start of foo
        e.insert('|');
        assert_eq!(e.text(), "|foo  bar 漢字");
        e.backspace();
        e.word_right(); // past foo + both spaces → start of bar
        e.insert('|');
        assert_eq!(e.text(), "foo  |bar 漢字");
        // word_right from the last word stops at the end (no panic).
        e.end();
        e.word_right();
        assert_eq!(e.cursor_col(), 14);
    }

    #[test]
    fn newline_inserts_and_take_preserves_it() {
        let mut e = LineEditor::default();
        e.insert('a');
        e.insert_newline();
        e.insert('b');
        assert_eq!(e.text(), "a\nb");
        assert_eq!(e.line_count(), 2);
        // The submit path carries the embedded newline through unchanged.
        assert_eq!(e.take(), "a\nb");
        assert!(e.is_empty());
        assert_eq!(e.line_count(), 1); // empty buffer is one (empty) line
    }

    #[test]
    fn cursor_row_col_tracks_lines_and_cjk_columns() {
        let mut e = ed("ab\n漢c");
        // End: row 1, col = 2 (漢) + 1 (c) = 3.
        assert_eq!(e.cursor_row_col(), (1, 3));
        e.home(); // start of line 2
        assert_eq!(e.cursor_row_col(), (1, 0));
        e.up(); // up to line 1, col 0
        assert_eq!(e.cursor_row_col(), (0, 0));
        e.end(); // end of line 1 ("ab" = 2 columns)
        assert_eq!(e.cursor_row_col(), (0, 2));
        e.down(); // back to line 2, column clamped to the row width
        assert_eq!(e.cursor_row_col().0, 1);
    }

    #[test]
    fn left_right_cross_line_boundaries() {
        let mut e = ed("a\nb");
        e.home(); // line 2 start
        assert_eq!(e.cursor_row_col(), (1, 0));
        e.left(); // onto the newline → end of line 1
        assert_eq!(e.cursor_row_col(), (0, 1));
        e.left(); // before 'a'
        assert_eq!(e.cursor_row_col(), (0, 0));
        e.right();
        e.right(); // back across the newline into line 2
        assert_eq!(e.cursor_row_col(), (1, 0));
    }

    #[test]
    fn display_window_fits_short_text() {
        let e = ed("hi");
        let (view, x) = e.display_window(10);
        assert_eq!(view, "hi");
        assert_eq!(x, 2); // cursor after the text
    }

    #[test]
    fn display_window_scrolls_to_keep_cursor_visible() {
        let e = ed("abcdefghij"); // 10 cols, cursor at end
        let (view, x) = e.display_window(5);
        // Cursor stays strictly inside: 4 tail chars + the cursor column.
        assert_eq!(view, "ghij");
        assert_eq!(x, 4);
        // Cursor at the start shows the head instead.
        let mut e = ed("abcdefghij");
        e.home();
        let (view, x) = e.display_window(5);
        assert_eq!(view, "abcde");
        assert_eq!(x, 0);
    }

    #[test]
    fn display_window_counts_cjk_as_two_columns() {
        let e = ed("漢字測試"); // 8 cols, cursor at end
        let (view, x) = e.display_window(5);
        // Only two fullwidth chars fit ahead of the cursor column.
        assert_eq!(view, "測試");
        assert_eq!(x, 4);
        // A window too narrow for even one fullwidth char shows nothing but
        // still reports a safe in-range cursor (never panics, never splits).
        let mut e = ed("漢");
        e.home();
        let (view, x) = e.display_window(1);
        assert_eq!(view, "");
        assert_eq!(x, 0);
    }

    #[test]
    fn display_window_mid_text_cursor() {
        let mut e = ed("abc漢字def");
        e.home();
        for _ in 0..4 {
            e.right(); // cursor after 漢 (col 5)
        }
        let (view, x) = e.display_window(4);
        // min_start = 5-3 = 2 → window starts at 'c'; 漢 fills cols 1-2.
        assert_eq!(view, "c漢");
        assert_eq!(x, 3);
        // Zero width is a hard no-op, not a panic.
        assert_eq!(e.display_window(0), ("", 0));
    }
}
