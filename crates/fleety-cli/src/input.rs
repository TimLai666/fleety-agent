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
