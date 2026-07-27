//! Markdown rendering for assistant replies in the chat TUI.
//!
//! The rendering itself is `fleety-markdown`, a vendored copy of grok-build's
//! renderer (see that crate's README for provenance). This module is only the
//! adapter: it owns the syntax-highlighting theme, pins the one style and
//! rendering mode the chat pane uses, and keeps the pure
//! `&str -> Vec<Line<'static>>` shape the transcript expects.
//!
//! It replaced a hand-written line-oriented renderer that understood a small
//! subset of markdown. The vendored renderer is CommonMark via `pulldown-cmark`
//! with `syntect` highlighting, so fenced code blocks are now coloured by
//! language, and tables, task lists and links render properly.

use std::sync::LazyLock;

use anstyle::{AnsiColor, Effects, Style};
use ratatui::text::Line;

/// The chat pane's markdown palette.
///
/// `MarkdownStyle::default()` is entirely unstyled — upstream fills it from its
/// own theme layer, which is not part of the vendored crate — so the colours
/// are Fleety's to choose. Styles are `anstyle`, not ratatui: the renderer
/// converts them on the way out.
///
/// The `_outer` styles are the source markers (`**`, backticks, `#`). They stay
/// on screen but dimmed, so a reply still reads as the markdown it was written
/// as without the punctuation competing with the words.
///
/// Everything uses the 16 ANSI colours, so the palette follows whatever theme
/// the user's terminal already has instead of fighting it.
fn chat_style() -> fleety_markdown::MarkdownStyle {
    let dim = Style::new() | Effects::DIMMED;
    let fg = |c: AnsiColor| Style::new().fg_color(Some(c.into()));
    let headings = std::array::from_fn(|level| {
        let base = fg(AnsiColor::Cyan);
        // h1/h2 carry weight; deeper levels stay quieter so a long reply does
        // not read as a wall of bold.
        if level < 2 {
            base | Effects::BOLD
        } else {
            base
        }
    });
    fleety_markdown::MarkdownStyle {
        heading_inner: headings,
        heading_outer: [dim; 6],
        strong_inner: Style::new() | Effects::BOLD,
        strong_outer: dim,
        emphasis_inner: Style::new() | Effects::ITALIC,
        emphasis_outer: dim,
        strikethrough_inner: Style::new() | Effects::STRIKETHROUGH,
        strikethrough_outer: dim,
        inline_code_inner: fg(AnsiColor::BrightYellow),
        inline_code_outer: dim,
        blockquote_outer: fg(AnsiColor::Green) | Effects::DIMMED,
        task_checked: fg(AnsiColor::Green),
        task_unchecked: dim,
        list_item: fg(AnsiColor::Cyan),
        rule: dim,
        link_outer: dim,
        link_text: fg(AnsiColor::Blue) | Effects::UNDERLINE,
        link_url: fg(AnsiColor::Blue) | Effects::DIMMED,
        link_title: dim,
        code_outer: dim,
        code_language: fg(AnsiColor::Magenta),
        // An untagged fence gets no syntax highlighting, so it carries its own
        // foreground to stay distinct from prose.
        code_untagged: fg(AnsiColor::BrightYellow),
        // A fenced block reads as a block because it sits on its own ground.
        // This is what replaces the `│ ` gutter the old renderer drew.
        code_background: Style::new().bg_color(Some(AnsiColor::Black.into())),
        table_outer: dim,
        text: Style::new(),
        math: fg(AnsiColor::Magenta),
    }
}

/// Syntax-highlighting theme, built once.
///
/// `Syntect::new` parses the theme and builds the syntax set, which is far too
/// expensive to redo per frame — `render` runs for every message on every draw.
/// The bytes come from the vendored crate's own asset directory; a re-sync that
/// moves it breaks this at compile time, which is the intent.
static SYNTECT: LazyLock<fleety_markdown::Syntect> = LazyLock::new(|| {
    fleety_markdown::Syntect::new(include_bytes!(
        "../../fleety-markdown/assets/tokyo-night.tmTheme"
    ))
});

/// Render `text` into styled display lines. Pure and total: malformed markdown
/// degrades to literal text rather than disappearing.
///
/// Built from the parser directly rather than the one-call helper, because the
/// helper collapses soft breaks the way CommonMark says to. Models put meaning
/// in a bare newline, so a reply written as two lines has to stay two lines;
/// joining them into a paragraph would silently reflow what the model wrote.
pub fn render(text: &str) -> Vec<Line<'static>> {
    let mut buffers = fleety_markdown::MarkdownBuffers::new();
    let (output, _checkpoint) =
        fleety_markdown::MarkdownParser::new(text, chat_style(), &mut buffers, Some(&SYNTECT))
            .collapse_soft_breaks(false)
            .parse()
            .render_ratatui(true);
    let lines = output.lines;
    // An empty reply still occupies one line so the role label has somewhere to
    // attach, which is what the transcript's line accounting expects.
    if lines.is_empty() {
        return vec![Line::from(String::new())];
    }
    lines
}

/// Render `text` to an ANSI-escaped string, for content that is leaving the
/// viewport and becoming terminal history.
///
/// The scrollback path takes text, not ratatui lines: once content is handed to
/// the terminal it is the terminal's, and styling has to travel with it as the
/// escape sequences the terminal itself understands.
pub fn render_ansi(text: &str) -> String {
    let (ansi, _source_map) =
        fleety_markdown::render_markdown(text, chat_style(), true, Some(&SYNTECT));
    ansi
}

/// The byte offset up to which `text` has settled into closed markdown blocks.
///
/// Content before it can be handed to the terminal and never redrawn. Content
/// after it belongs to a block that is still open — a fence with no closing
/// fence, a half-written table — and must stay in the viewport, because how it
/// renders can still change as more of it arrives.
pub fn settled_prefix_len(text: &str) -> usize {
    let mut buffers = fleety_markdown::MarkdownBuffers::new();
    let (_out, checkpoint) =
        fleety_markdown::MarkdownParser::new(text, chat_style(), &mut buffers, Some(&SYNTECT))
            .collapse_soft_breaks(false)
            .parse()
            .render_ratatui(true);
    checkpoint.map_or(0, |c| c.source_bytes).min(text.len())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text_of(line: &Line) -> String {
        line.spans.iter().map(|s| s.content.as_ref()).collect()
    }

    fn rendered_text(src: &str) -> String {
        render(src)
            .iter()
            .map(text_of)
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[test]
    fn empty_input_yields_one_line() {
        assert_eq!(render("").len(), 1, "role label needs a line to attach to");
    }

    #[test]
    fn plain_text_survives_rendering() {
        assert!(rendered_text("hello world").contains("hello world"));
    }

    #[test]
    fn emphasis_is_styled_and_its_markers_are_dimmed_not_removed() {
        // The vendored renderer keeps `**` visible but styles it separately
        // from the emphasised text, so the source stays readable. Assert the
        // styling split rather than the removal the old renderer did.
        let lines = render("this is **bold**");
        let marker_style = lines
            .iter()
            .flat_map(|l| l.spans.iter())
            .find(|s| s.content.contains("**"))
            .map(|s| s.style);
        let word_style = lines
            .iter()
            .flat_map(|l| l.spans.iter())
            .find(|s| s.content.contains("bold"))
            .map(|s| s.style);
        assert!(marker_style.is_some(), "markers kept: {lines:?}");
        assert!(word_style.is_some(), "content kept: {lines:?}");
        assert_ne!(marker_style, word_style, "markers styled apart from text");
    }

    #[test]
    fn a_bare_newline_stays_a_line_break() {
        // CommonMark would fold these into one paragraph. Model replies mean
        // the break, so the adapter turns that collapsing off.
        assert_eq!(render("first line\nsecond line").len(), 2);
    }

    #[test]
    fn fenced_code_is_highlighted_by_language() {
        let lines = render("```rust\nfn main() {}\n```");
        let joined = lines.iter().map(text_of).collect::<Vec<_>>().join("\n");
        assert!(joined.contains("fn main()"), "code kept: {joined}");
        let coloured = lines
            .iter()
            .flat_map(|l| l.spans.iter())
            .any(|s| s.style.fg.is_some());
        assert!(coloured, "syntect coloured the block");
    }

    #[test]
    fn unterminated_fence_degrades_without_panic() {
        let out = rendered_text("```rust\nfn main() {}\n");
        assert!(out.contains("fn main()"), "content not dropped: {out}");
    }

    #[test]
    fn malformed_emphasis_stays_literal() {
        let out = rendered_text("a ** b");
        assert!(out.contains('a'), "content not dropped: {out}");
        assert!(out.contains('b'), "content not dropped: {out}");
    }

    #[test]
    fn headings_and_lists_keep_their_text() {
        let out = rendered_text("# Title\n\n- one\n- two");
        for needle in ["Title", "one", "two"] {
            assert!(out.contains(needle), "{needle} kept: {out}");
        }
    }

    #[test]
    fn mermaid_fences_render_as_box_drawn_diagrams() {
        // The vendored renderer draws `graph`/`flowchart`, `sequenceDiagram`
        // and `stateDiagram` blocks as Unicode line art. No graphics protocol
        // and no external process are involved, so this works on any terminal.
        let lines = render("```mermaid\ngraph TD\n  A[Start] --> B[Finish]\n```");
        let art = lines.iter().map(text_of).collect::<Vec<_>>().join("\n");
        assert!(
            art.contains("Start") && art.contains("Finish"),
            "labels kept: {art}"
        );
        assert!(
            art.contains('┌') && art.contains('└') && art.contains('▼'),
            "drawn as a box-and-arrow diagram, not left as source: {art}"
        );
        assert!(
            !art.contains("A[Start]"),
            "the mermaid source is replaced by the diagram: {art}"
        );
    }

    #[test]
    fn an_unsupported_mermaid_diagram_keeps_its_source() {
        let src = "```mermaid\npie title Votes\n  \"A\" : 10\n```";
        let out = rendered_text(src);
        assert!(out.contains("pie title Votes"), "source not dropped: {out}");
    }
}
