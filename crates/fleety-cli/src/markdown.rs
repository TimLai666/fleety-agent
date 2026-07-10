//! Minimal, self-built markdown renderer for assistant replies in the chat TUI.
//!
//! This is deliberately NOT a CommonMark implementation. It recognizes the small,
//! line-oriented subset that actually shows up in chat answers — ATX headings,
//! ordered and unordered lists, blockquotes, inline `` `code` `` and `**bold**`,
//! and triple-backtick fenced code blocks — and renders everything it does not
//! understand as plain text. `render` is a pure function
//! (`&str -> Vec<Line<'static>>`) that leans only on ratatui's own styling, so it
//! stays cheap to unit-test and never pulls a heavyweight parser into the CLI.
//!
//! Safety: the renderer never panics and never drops content. An unterminated
//! fence, a stray delimiter, or malformed emphasis all degrade to literal text.

use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};

/// Left gutter drawn on every fenced-code line so the block reads as set apart
/// from the surrounding prose even in a purely monospaced terminal.
const CODE_GUTTER: &str = "│ ";
/// Left gutter for blockquote lines.
const QUOTE_GUTTER: &str = "▏ ";

fn bold_style() -> Style {
    Style::default().add_modifier(Modifier::BOLD)
}

fn code_style() -> Style {
    Style::default().fg(Color::Cyan)
}

fn code_block_style() -> Style {
    Style::default().fg(Color::Cyan).add_modifier(Modifier::DIM)
}

fn quote_style() -> Style {
    Style::default().add_modifier(Modifier::DIM)
}

/// True when `line` is a triple-backtick fence delimiter (optionally followed by
/// an info string / language tag).
fn is_fence(line: &str) -> bool {
    line.trim_start().starts_with("```")
}

/// Render `text` into styled display lines. Pure and total: any unrecognized or
/// malformed construct falls back to literal text.
pub fn render(text: &str) -> Vec<Line<'static>> {
    let raw: Vec<&str> = text.split('\n').collect();

    // Pair fence delimiters up front. A trailing, unmatched fence (an
    // unterminated code block) is intentionally left unpaired so its lines are
    // rendered as ordinary text rather than swallowed into a never-closed block.
    let fences: Vec<usize> = raw
        .iter()
        .enumerate()
        .filter(|(_, l)| is_fence(l))
        .map(|(i, _)| i)
        .collect();
    let mut pairs: Vec<(usize, usize)> = Vec::new();
    let mut k = 0;
    while k + 1 < fences.len() {
        pairs.push((fences[k], fences[k + 1]));
        k += 2;
    }
    // Classify a row: Some(true) = inside a matched fence (code content),
    // Some(false) = a matched fence delimiter (hidden), None = outside any fence.
    let classify = |i: usize| -> Option<bool> {
        for &(open, close) in &pairs {
            if i == open || i == close {
                return Some(false);
            }
            if i > open && i < close {
                return Some(true);
            }
        }
        None
    };

    let mut out: Vec<Line<'static>> = Vec::with_capacity(raw.len());
    for (i, line) in raw.iter().enumerate() {
        match classify(i) {
            // Hide the ``` delimiter of a matched pair.
            Some(false) => {}
            Some(true) => out.push(code_block_line(line)),
            None => {
                if is_fence(line) {
                    // A leftover (unmatched) fence: show it literally instead of
                    // inline-parsing its backticks into an empty code span.
                    out.push(Line::from((*line).to_string()));
                } else {
                    out.push(prose_line(line));
                }
            }
        }
    }
    // Never hand back nothing, so callers can always touch the first line.
    if out.is_empty() {
        out.push(Line::from(String::new()));
    }
    out
}

/// A single line inside a fenced code block: monospaced-styled, gutter-marked,
/// and NOT inline-parsed (so `**x**` inside code stays literal).
fn code_block_line(content: &str) -> Line<'static> {
    Line::from(vec![
        Span::styled(CODE_GUTTER, code_block_style()),
        Span::styled(content.to_string(), code_block_style()),
    ])
}

/// A prose line: detect a leading block construct (heading / list / quote), then
/// render the remainder with inline emphasis + code spans.
fn prose_line(line: &str) -> Line<'static> {
    if let Some(rest) = heading_body(line) {
        // Headings render bold; inline markers inside are kept as literal text.
        return Line::from(Span::styled(rest.to_string(), bold_style()));
    }
    if let Some(rest) = line.strip_prefix("> ").or_else(|| line.strip_prefix('>')) {
        let mut spans = vec![Span::styled(QUOTE_GUTTER, quote_style())];
        for mut s in inline_spans(rest) {
            s.style = s.style.patch(quote_style());
            spans.push(s);
        }
        return Line::from(spans);
    }
    if let Some(rest) = unordered_body(line) {
        let mut spans = vec![Span::raw("• ")];
        spans.extend(inline_spans(rest));
        return Line::from(spans);
    }
    if let Some((num, rest)) = ordered_body(line) {
        let mut spans = vec![Span::raw(format!("{num}. "))];
        spans.extend(inline_spans(rest));
        return Line::from(spans);
    }
    Line::from(inline_spans(line))
}

/// The text after a leading ATX heading marker (`#`..`######` then one space),
/// or `None` when `line` is not a heading.
fn heading_body(line: &str) -> Option<&str> {
    let hashes = line.chars().take_while(|&c| c == '#').count();
    if (1..=6).contains(&hashes) {
        // '#' is ASCII, so the char count is also the byte offset.
        return line[hashes..].strip_prefix(' ');
    }
    None
}

/// The text after a leading unordered-list marker (`- `, `* `, `+ `).
fn unordered_body(line: &str) -> Option<&str> {
    ["- ", "* ", "+ "]
        .into_iter()
        .find_map(|p| line.strip_prefix(p))
}

/// The number and text of an ordered-list item (`N. `), or `None`.
fn ordered_body(line: &str) -> Option<(String, &str)> {
    let digits: String = line.chars().take_while(|c| c.is_ascii_digit()).collect();
    if digits.is_empty() {
        return None;
    }
    // Digits are ASCII, so byte length equals char count.
    let rest = line[digits.len()..].strip_prefix(". ")?;
    Some((digits, rest))
}

/// Split one prose line into styled spans, applying `**bold**` and `` `code` ``.
/// Unmatched delimiters are emitted as literal characters.
fn inline_spans(s: &str) -> Vec<Span<'static>> {
    let chars: Vec<char> = s.chars().collect();
    let mut spans: Vec<Span<'static>> = Vec::new();
    let mut buf = String::new();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        // Inline code: `...` (its content is never further parsed).
        if c == '`' {
            if let Some(close) = ((i + 1)..chars.len()).find(|&j| chars[j] == '`') {
                if !buf.is_empty() {
                    spans.push(Span::raw(std::mem::take(&mut buf)));
                }
                let code: String = chars[(i + 1)..close].iter().collect();
                spans.push(Span::styled(code, code_style()));
                i = close + 1;
                continue;
            }
            buf.push('`');
            i += 1;
            continue;
        }
        // Bold: **...**
        if c == '*' && i + 1 < chars.len() && chars[i + 1] == '*' {
            if let Some(close) = find_double_star(&chars, i + 2) {
                if !buf.is_empty() {
                    spans.push(Span::raw(std::mem::take(&mut buf)));
                }
                let bold: String = chars[(i + 2)..close].iter().collect();
                spans.push(Span::styled(bold, bold_style()));
                i = close + 2;
                continue;
            }
            buf.push('*');
            buf.push('*');
            i += 2;
            continue;
        }
        buf.push(c);
        i += 1;
    }
    if !buf.is_empty() {
        spans.push(Span::raw(buf));
    }
    if spans.is_empty() {
        // Keep a blank line as a real (empty) line rather than collapsing it.
        spans.push(Span::raw(String::new()));
    }
    spans
}

/// Index of the next `**` at or after `from`, or `None`.
fn find_double_star(chars: &[char], from: usize) -> Option<usize> {
    let mut j = from;
    while j + 1 < chars.len() {
        if chars[j] == '*' && chars[j + 1] == '*' {
            return Some(j);
        }
        j += 1;
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Concatenate a line's span text back into a plain string.
    fn text_of(line: &Line) -> String {
        line.spans.iter().map(|s| s.content.clone()).collect()
    }

    #[test]
    fn plain_lines_map_one_to_one() {
        let lines = render("first line\nsecond line");
        assert_eq!(lines.len(), 2);
        assert_eq!(text_of(&lines[0]), "first line");
        assert_eq!(text_of(&lines[1]), "second line");
    }

    #[test]
    fn fenced_block_is_monospaced_and_not_inline_parsed() {
        let lines = render("before\n```\n**not bold**\n```\nafter");
        // Three visible lines: prose, code, prose (the ``` fences are hidden).
        assert_eq!(lines.len(), 3);
        assert_eq!(text_of(&lines[0]), "before");
        assert_eq!(text_of(&lines[2]), "after");

        let code = &lines[1];
        let code_text = text_of(code);
        // The block content keeps its literal asterisks — inline markdown inside
        // a fence is NOT interpreted.
        assert!(
            code_text.contains("**not bold**"),
            "fence content stays literal: {code_text:?}"
        );
        // The gutter marker sets the block apart from prose…
        assert!(code_text.starts_with(CODE_GUTTER), "gutter marks the block");
        // …and the content carries the distinct code style.
        assert!(
            code.spans.iter().any(|s| s.style.fg == Some(Color::Cyan)),
            "code content is styled"
        );
        // Prose lines are not gutter-marked.
        assert!(!text_of(&lines[0]).starts_with(CODE_GUTTER));
    }

    #[test]
    fn inline_bold_and_code_are_styled_without_delimiters() {
        let lines = render("say **hi** and `x` please");
        assert_eq!(lines.len(), 1);
        let spans = &lines[0].spans;
        let bold = spans
            .iter()
            .find(|s| s.style.add_modifier.contains(Modifier::BOLD))
            .expect("a bold span");
        assert_eq!(bold.content.as_ref(), "hi");
        let code = spans
            .iter()
            .find(|s| s.style.fg == Some(Color::Cyan))
            .expect("a code span");
        assert_eq!(code.content.as_ref(), "x");
        // The reassembled visible text carries no delimiter characters.
        assert_eq!(text_of(&lines[0]), "say hi and x please");
    }

    #[test]
    fn unterminated_fence_degrades_to_text_without_panic() {
        // An opening fence with no close: nothing is dropped and the stray fence
        // is shown literally rather than hiding everything after it.
        let lines = render("intro\n```\nstill going");
        let joined = lines
            .iter()
            .map(|l| text_of(l))
            .collect::<Vec<_>>()
            .join("\n");
        assert!(joined.contains("intro"), "{joined:?}");
        assert!(joined.contains("still going"), "{joined:?}");
        assert!(joined.contains("```"), "stray fence shown literally: {joined:?}");
    }

    #[test]
    fn heading_and_lists_render_with_markers() {
        let lines = render("## Title\n- item one\n1. first");
        assert_eq!(lines.len(), 3);
        assert!(
            lines[0]
                .spans
                .iter()
                .any(|s| s.style.add_modifier.contains(Modifier::BOLD)),
            "heading is bold"
        );
        assert_eq!(text_of(&lines[0]), "Title");
        let bullet = text_of(&lines[1]);
        assert!(bullet.starts_with("• "), "{bullet:?}");
        assert!(bullet.contains("item one"));
        let ordered = text_of(&lines[2]);
        assert!(ordered.starts_with("1. "), "{ordered:?}");
        assert!(ordered.contains("first"));
    }

    #[test]
    fn malformed_emphasis_is_literal() {
        // A dangling ** and a lone ` must not panic and must stay visible.
        let lines = render("a **b and c ` d");
        assert_eq!(text_of(&lines[0]), "a **b and c ` d");
    }

    #[test]
    fn empty_input_yields_one_blank_line() {
        let lines = render("");
        assert_eq!(lines.len(), 1);
        assert_eq!(text_of(&lines[0]), "");
    }
}
