//! The inline-viewport path, exercised against a captured byte stream.
//!
//! `emit_to_scrollback` writes escape sequences to whatever the terminal's
//! backend writes to, so pointing that at a buffer runs the real vendored code
//! without needing a physical terminal. What a TTY would receive is what these
//! tests read back.
//!
//! In-crate rather than under `tests/`: `fleety-cli` is a binary, so an
//! integration test could reach `fleety_inline` but not `App`, and the seam
//! worth testing is exactly where the two meet.

use std::io::Write;

use ratatui::backend::{Backend, CrosstermBackend, WindowSize};
use ratatui::layout::Size;

/// A `CrosstermBackend` over a buffer, with a fixed size: the real one asks the
/// operating system for the window size, and there is no window here.
pub(crate) struct Capture {
    inner: CrosstermBackend<Vec<u8>>,
    /// Everything written, kept alongside the backend because its own writer is
    /// private.
    pub(crate) written: Vec<u8>,
    /// Text snapshots of each frame passed to the backend. Tests that drive a
    /// full-screen editor can inspect an intermediate frame without relying
    /// on the terminal's final buffer.
    pub(crate) frames: Vec<String>,
    buffer: Vec<Vec<String>>,
    size: Size,
    fail_writes: usize,
    fail_clear_regions: usize,
}

impl Capture {
    pub(crate) fn new(width: u16, height: u16) -> Self {
        Self {
            inner: CrosstermBackend::new(Vec::new()),
            written: Vec::new(),
            frames: Vec::new(),
            buffer: vec![vec![" ".to_string(); width as usize]; height as usize],
            size: Size { width, height },
            fail_writes: 0,
            fail_clear_regions: 0,
        }
    }

    pub(crate) fn fail_write_after(&mut self, writes_before_failure: usize) {
        self.fail_writes = writes_before_failure.saturating_add(1);
    }

    pub(crate) fn fail_clear_region_after(&mut self, clears_before_failure: usize) {
        self.fail_clear_regions = clears_before_failure.saturating_add(1);
    }
}

impl Write for Capture {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        if self.fail_writes > 0 {
            self.fail_writes -= 1;
            if self.fail_writes == 0 {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::BrokenPipe,
                    "injected terminal write failure",
                ));
            }
        }
        self.written.extend_from_slice(buf);
        self.inner.write(buf)
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Write::flush(&mut self.inner)
    }
}

impl Backend for Capture {
    fn draw<'a, I>(&mut self, content: I) -> std::io::Result<()>
    where
        I: Iterator<Item = (u16, u16, &'a ratatui::buffer::Cell)>,
    {
        let cells: Vec<_> = content.map(|(x, y, cell)| (x, y, cell.clone())).collect();
        self.inner
            .draw(cells.iter().map(|(x, y, cell)| (*x, *y, cell)))?;
        for (x, y, cell) in &cells {
            if let Some(row) = self.buffer.get_mut(*y as usize) {
                if let Some(slot) = row.get_mut(*x as usize) {
                    *slot = cell.symbol().to_string();
                }
            }
        }
        self.frames.push(
            self.buffer
                .clone()
                .into_iter()
                .map(|row| row.concat())
                .collect::<Vec<_>>()
                .join("\n"),
        );
        Ok(())
    }
    fn hide_cursor(&mut self) -> std::io::Result<()> {
        self.inner.hide_cursor()
    }
    fn show_cursor(&mut self) -> std::io::Result<()> {
        self.inner.show_cursor()
    }
    fn get_cursor_position(&mut self) -> std::io::Result<ratatui::layout::Position> {
        Ok(ratatui::layout::Position { x: 0, y: 0 })
    }
    fn set_cursor_position<P: Into<ratatui::layout::Position>>(
        &mut self,
        position: P,
    ) -> std::io::Result<()> {
        self.inner.set_cursor_position(position)
    }
    fn clear(&mut self) -> std::io::Result<()> {
        self.inner.clear()
    }
    fn clear_region(&mut self, clear_type: ratatui::backend::ClearType) -> std::io::Result<()> {
        if self.fail_clear_regions > 0 {
            self.fail_clear_regions -= 1;
            if self.fail_clear_regions == 0 {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::BrokenPipe,
                    "injected terminal clear failure",
                ));
            }
        }
        self.inner.clear_region(clear_type)
    }
    fn append_lines(&mut self, n: u16) -> std::io::Result<()> {
        self.inner.append_lines(n)
    }
    fn size(&self) -> std::io::Result<Size> {
        Ok(self.size)
    }
    fn window_size(&mut self) -> std::io::Result<WindowSize> {
        Ok(WindowSize {
            columns_rows: self.size,
            pixels: Size {
                width: 0,
                height: 0,
            },
        })
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Backend::flush(&mut self.inner)
    }
}

pub(crate) fn terminal(width: u16, height: u16) -> fleety_inline::Terminal<Capture> {
    fleety_inline::Terminal::with_options(
        Capture::new(width, height),
        ratatui::TerminalOptions {
            viewport: ratatui::Viewport::Inline(4),
        },
    )
    .expect("inline terminal")
}

pub(crate) fn failing_terminal(width: u16, height: u16) -> fleety_inline::Terminal<Capture> {
    let mut terminal = terminal(width, height);
    terminal.backend_mut().fail_write_after(0);
    terminal
}

/// Everything written so far, with escape sequences stripped.
pub(crate) fn visible(terminal: &fleety_inline::Terminal<Capture>) -> String {
    let raw = String::from_utf8_lossy(&terminal.backend().written).to_string();
    let mut out = String::new();
    let mut chars = raw.chars();
    while let Some(c) = chars.next() {
        if c != '\u{1b}' {
            out.push(c);
            continue;
        }
        for c in chars.by_ref() {
            if c.is_ascii_alphabetic() {
                break;
            }
        }
    }
    out
}

#[cfg(test)]
#[test]
fn emitted_content_reaches_the_terminal() {
    let mut term = terminal(40, 20);
    fleety_inline::emit_to_scrollback(&mut term, "you: hello\n").expect("emit");
    fleety_inline::emit_to_scrollback(&mut term, "fleety: hi there\n").expect("emit");

    let seen = visible(&term);
    assert!(
        seen.contains("you: hello"),
        "first message written: {seen:?}"
    );
    assert!(
        seen.contains("fleety: hi there"),
        "second message written: {seen:?}"
    );
    assert!(
        seen.find("you: hello") < seen.find("fleety: hi there"),
        "written in order: {seen:?}"
    );
}

#[cfg(test)]
#[test]
fn styling_survives_the_trip_to_the_terminal() {
    let mut term = terminal(40, 20);
    // Red text, then reset — what the markdown renderer produces.
    fleety_inline::emit_to_scrollback(&mut term, "\u{1b}[31mred\u{1b}[0m\n").expect("emit");

    let raw = String::from_utf8_lossy(&term.backend().written).to_string();
    assert!(raw.contains("\u{1b}[31m"), "colour reached the terminal");
    assert!(visible(&term).contains("red"), "text reached the terminal");
}

#[cfg(test)]
#[test]
fn a_resize_replays_the_whole_history() {
    let mut term = terminal(40, 20);
    let history = "you: one\nfleety: two\n";
    fleety_inline::emit_to_scrollback(&mut term, history).expect("emit");

    let before = visible(&term).matches("fleety: two").count();
    fleety_inline::resize_purge_rerender(&mut term, history).expect("replay");
    let after = visible(&term).matches("fleety: two").count();

    assert!(
        after > before,
        "the history is written again at the new width, not left to the \
         terminal's own reflow (before {before}, after {after})"
    );
}

#[cfg(test)]
#[test]
fn the_viewport_can_grow_and_shrink_without_losing_history() {
    let mut term = terminal(40, 20);
    fleety_inline::emit_to_scrollback(&mut term, "kept: yes\n").expect("emit");
    term.set_viewport_height(12).expect("grow");
    term.set_viewport_height(4).expect("shrink");

    assert!(
        visible(&term).contains("kept: yes"),
        "content already handed over is not touched by viewport changes"
    );
}

/// The seam itself: App state in, terminal bytes out.
#[cfg(test)]
mod sync {
    use super::{failing_terminal, terminal, visible};
    use crate::tui::App;
    use crate::{sync_terminal, ViewportState};

    #[test]
    fn a_finished_exchange_reaches_the_terminal_and_leaves_the_viewport() {
        let mut term = terminal(60, 24);
        let mut state = ViewportState::new(&term);
        let mut app = App::new("ready");
        app.push("you", "hello");
        app.push("fleety", "hi there");

        sync_terminal(&mut app, &mut term, true, &mut state);

        let seen = visible(&term);
        assert!(seen.contains("you: hello"), "written out: {seen:?}");
        assert!(seen.contains("hi there"), "written out: {seen:?}");
        assert!(
            app.viewport_tail().is_empty(),
            "nothing settled is left for Fleety to draw"
        );
    }

    #[test]
    fn the_same_content_is_never_written_twice() {
        let mut term = terminal(60, 24);
        let mut state = ViewportState::new(&term);
        let mut app = App::new("ready");
        app.push("you", "once");

        sync_terminal(&mut app, &mut term, true, &mut state);
        let after_first = visible(&term).matches("once").count();
        sync_terminal(&mut app, &mut term, true, &mut state);
        let after_second = visible(&term).matches("once").count();

        assert_eq!(
            after_first, after_second,
            "a second frame must not re-emit what the terminal already has"
        );
    }

    #[test]
    fn a_transient_scrollback_write_failure_replays_the_history() {
        let mut term = failing_terminal(60, 24);
        let mut state = ViewportState::new(&term);
        let mut app = App::new("ready");
        app.push("you", "recover me");

        sync_terminal(&mut app, &mut term, true, &mut state);

        let seen = visible(&term);
        assert!(
            seen.contains("you: recover me"),
            "recovered output: {seen:?}"
        );
        assert_eq!(
            seen.matches("recover me").count(),
            1,
            "the recovery redraw must not duplicate the transcript: {seen:?}"
        );
        assert_eq!(app.status, "ready", "recovery must not leave a stale error");
    }

    #[test]
    fn a_partially_emitted_scrollback_block_is_replayed_from_history() {
        let mut term = terminal(60, 24);
        term.backend_mut().fail_write_after(2);
        let mut state = ViewportState::new(&term);
        let mut app = App::new("ready");
        app.push("you", "recover after partial output");

        sync_terminal(&mut app, &mut term, true, &mut state);

        let seen = visible(&term);
        assert!(
            seen.contains("you: recover after partial output"),
            "recovered output: {seen:?}"
        );
        assert_eq!(
            seen.matches("recover after partial output").count(),
            1,
            "the recovery redraw must replace partial output: {seen:?}"
        );
    }

    #[test]
    fn a_failed_viewport_resize_is_reported_and_retried() {
        let mut term = terminal(60, 24);
        term.backend_mut().fail_clear_region_after(0);
        let mut state = ViewportState::new(&term);
        let mut app = App::new("ready");

        sync_terminal(&mut app, &mut term, true, &mut state);

        assert_eq!(state.rows, 0, "a failed resize must not be marked complete");
        assert!(
            app.status
                .contains("could not resize the terminal viewport"),
            "resize failure must be visible: {:?}",
            app.status
        );

        sync_terminal(&mut app, &mut term, true, &mut state);

        assert!(state.rows > 0, "the next sync must retry the resize");
        assert_eq!(
            app.status, "ready",
            "a recovered resize must restore status"
        );
    }

    #[test]
    fn a_final_viewport_clear_failure_is_retried() {
        let mut term = terminal(60, 24);
        term.backend_mut().fail_clear_region_after(1);
        let mut state = ViewportState::new(&term);
        let mut app = App::new("ready");

        sync_terminal(&mut app, &mut term, true, &mut state);

        assert_eq!(
            state.rows, 0,
            "a failed final clear must not be marked complete"
        );
        assert!(
            state.viewport_clear_needed,
            "the final clear must be retried"
        );

        sync_terminal(&mut app, &mut term, true, &mut state);

        assert!(
            state.rows > 0,
            "the final clear must recover on the next sync"
        );
        assert!(
            !state.viewport_clear_needed,
            "a successful clear must settle the retry"
        );
        assert_eq!(app.status, "ready", "a recovered clear must restore status");
    }

    #[test]
    fn terminal_retries_preserve_the_latest_application_status() {
        let mut term = terminal(60, 24);
        term.backend_mut().fail_clear_region_after(0);
        let mut state = ViewportState::new(&term);
        let mut app = App::new("ready");

        sync_terminal(&mut app, &mut term, true, &mut state);
        app.status = "streaming…".into();
        term.backend_mut().fail_clear_region_after(0);

        sync_terminal(&mut app, &mut term, true, &mut state);
        sync_terminal(&mut app, &mut term, true, &mut state);

        assert_eq!(
            app.status, "streaming…",
            "terminal recovery must not discard a newer application status"
        );
    }

    #[test]
    fn an_unclosed_code_fence_stays_in_the_viewport() {
        let mut term = terminal(60, 24);
        let mut state = ViewportState::new(&term);
        let mut app = App::new("ready");
        app.push_delta("intro paragraph.\n\n```rust\nlet x = 1;\n");

        sync_terminal(&mut app, &mut term, true, &mut state);

        let seen = visible(&term);
        assert!(
            seen.contains("intro paragraph."),
            "the closed paragraph went out: {seen:?}"
        );
        assert!(
            !seen.contains("let x = 1;"),
            "the open fence did not — it still renders differently once closed: {seen:?}"
        );
        let tail: String = app
            .viewport_tail()
            .iter()
            .flat_map(|l| l.spans.iter())
            .map(|s| s.content.as_ref())
            .collect();
        assert!(tail.contains("let x = 1;"), "and it is on screen: {tail:?}");
    }

    #[test]
    fn closing_the_fence_releases_it() {
        let mut term = terminal(60, 24);
        let mut state = ViewportState::new(&term);
        let mut app = App::new("ready");
        app.push_delta("```rust\nlet x = 1;\n");
        sync_terminal(&mut app, &mut term, true, &mut state);
        app.push_delta("```\n\n");
        sync_terminal(&mut app, &mut term, true, &mut state);

        assert!(
            visible(&term).contains("let x = 1;"),
            "once the block closes it becomes terminal history"
        );
    }
}
