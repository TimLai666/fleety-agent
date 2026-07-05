//! Cursor-aware single-line editor shared by the TUI input boxes.
//!
//! The cursor is a **char index** (not bytes, not columns); every mutation
//! converts to a byte offset via `char_indices`, so UTF-8 boundaries can never
//! be split (this crate is never-crash). Display math uses terminal columns
//! (CJK fullwidth = 2), measured through ratatui's `Line::width` so we stay on
//! the exact same unicode-width tables the renderer uses — without adding a
//! direct dependency.

use ratatui::text::Line;

/// Display width of one char in terminal columns (via ratatui, see module doc).
fn ch_width(c: char) -> usize {
    let mut buf = [0u8; 4];
    Line::from(&*c.encode_utf8(&mut buf)).width()
}

/// Single-line text buffer with a movable cursor.
#[derive(Debug, Default, Clone)]
pub struct LineEditor {
    text: String,
    /// Cursor as a char index, 0..=char count (== count means "after the end").
    cursor: usize,
}

impl LineEditor {
    /// Byte offset of the cursor (text.len() when the cursor is at the end).
    fn byte_offset(&self) -> usize {
        self.text
            .char_indices()
            .nth(self.cursor)
            .map(|(b, _)| b)
            .unwrap_or(self.text.len())
    }

    pub fn insert(&mut self, c: char) {
        let b = self.byte_offset();
        self.text.insert(b, c);
        self.cursor += 1;
    }

    /// Insert a whole string at the cursor (paste path).
    pub fn insert_str(&mut self, s: &str) {
        let b = self.byte_offset();
        self.text.insert_str(b, s);
        self.cursor += s.chars().count();
    }

    /// Delete the char before the cursor.
    pub fn backspace(&mut self) {
        if self.cursor == 0 {
            return;
        }
        self.cursor -= 1;
        let b = self.byte_offset();
        self.text.remove(b);
    }

    /// Delete the char under the cursor.
    pub fn delete(&mut self) {
        let b = self.byte_offset();
        if b < self.text.len() {
            self.text.remove(b);
        }
    }

    pub fn left(&mut self) {
        self.cursor = self.cursor.saturating_sub(1);
    }

    pub fn right(&mut self) {
        if self.cursor < self.text.chars().count() {
            self.cursor += 1;
        }
    }

    pub fn home(&mut self) {
        self.cursor = 0;
    }

    pub fn end(&mut self) {
        self.cursor = self.text.chars().count();
    }

    /// Jump to the start of the previous word (whitespace-delimited).
    pub fn word_left(&mut self) {
        let chars: Vec<char> = self.text.chars().collect();
        let mut i = self.cursor.min(chars.len());
        while i > 0 && chars[i - 1].is_whitespace() {
            i -= 1;
        }
        while i > 0 && !chars[i - 1].is_whitespace() {
            i -= 1;
        }
        self.cursor = i;
    }

    /// Jump past the current word to the start of the next (or the end).
    pub fn word_right(&mut self) {
        let chars: Vec<char> = self.text.chars().collect();
        let n = chars.len();
        let mut i = self.cursor.min(n);
        while i < n && !chars[i].is_whitespace() {
            i += 1;
        }
        while i < n && chars[i].is_whitespace() {
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
        self.cursor = text.chars().count();
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
        self.text.chars().take(self.cursor).map(ch_width).sum()
    }

    /// The slice of text to show in a box `width` columns wide, plus the
    /// cursor's column within that slice. When the text overflows, the window
    /// scrolls just enough to keep the cursor strictly inside (never on the
    /// border), starting and ending on char boundaries so a fullwidth char is
    /// never half-shown.
    pub fn display_window(&self, width: usize) -> (&str, u16) {
        if width == 0 {
            return ("", 0);
        }
        let chars: Vec<(usize, usize)> = self
            .text
            .char_indices()
            .map(|(b, c)| (b, ch_width(c)))
            .collect();
        let cursor_col: usize = chars
            .iter()
            .take(self.cursor)
            .map(|&(_, w)| w)
            .sum();
        let total: usize = chars.iter().map(|&(_, w)| w).sum();
        // `<` not `<=`: the cursor needs its own column when sitting past the
        // last char.
        if total < width {
            return (&self.text, cursor_col as u16);
        }
        // Scroll right until the cursor column fits in [0, width-1].
        let min_start = cursor_col.saturating_sub(width - 1);
        let mut start_idx = chars.len();
        let mut start_col = total;
        let mut acc = 0;
        for (i, &(_, w)) in chars.iter().enumerate() {
            if acc >= min_start {
                start_idx = i;
                start_col = acc;
                break;
            }
            acc += w;
        }
        // Fill the window with whole chars only.
        let mut end_idx = start_idx;
        let mut used = 0;
        for &(_, w) in chars.iter().skip(start_idx) {
            if used + w > width {
                break;
            }
            used += w;
            end_idx += 1;
        }
        let start_b = chars.get(start_idx).map(|&(b, _)| b).unwrap_or(self.text.len());
        let end_b = chars.get(end_idx).map(|&(b, _)| b).unwrap_or(self.text.len());
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
