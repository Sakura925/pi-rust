//! TuiAltScreen - Alternate screen TUI with constrained layout.
//!
//! Uses the alternate screen buffer for fullscreen mode with a scrollable
//! transcript and fixed bottom dock, as described in `tui-plan.md`.

use std::any::Any;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use super::component::Component;
use super::container::Container;
use super::layout::{
    composite_tui_line, extract_cursor_position, render_layout_frame,
    render_layout_frame_reusing_scroll_content, LayoutFrame,
};
use super::overlay::OverlayManager;
use super::scroll_view::ScrollView;
use super::tui::{OverlayHandle, OverlayOptions, TuiMode, TuiStopOptions, TUI};
use crate::ansi::{visible_width, CURSOR_MARKER};
use crate::terminal::{InputEvent, Terminal, TerminalInfo};

/// Symbol for ViewportTUI capability check.
pub const VIEWPORT_TUI: &[u8] = b"rpi-tui/viewport";

/// Upper bound on the coalesced repaint rate, matching upstream's
/// `TuiBase.MIN_RENDER_INTERVAL_MS` (16 ms ≈ 60 fps).
///
/// A provider streams one `MessageUpdate` per delta, and every delta used to
/// repaint the *whole* transcript (full markdown rebuild + full layout pass +
/// a full-viewport terminal write). Throttling that to one frame per interval
/// is what keeps long, fast output from saturating the terminal — and the
/// GPU-backed compositor behind it.
pub const MIN_RENDER_INTERVAL_MS: u64 = 16;

/// Coalesced repaint interval for terminals without CSI 2026 (synchronized
/// output) support. Every rpi frame is wrapped in `?2026h`/`?2026l`; a
/// terminal that ignores the pair exposes the clear-then-rewrite window of a
/// full rebuild as visible flicker. Slowing the repaint cadence to ~15 fps on
/// those terminals trades smoothness for a calmer screen; synchronized-output
/// terminals keep [`MIN_RENDER_INTERVAL_MS`].
pub const FALLBACK_RENDER_INTERVAL_MS: u64 = 66;

/// Merge an incoming frame request into one that is already parked.
///
/// `reuse_scroll_content = false` means "the transcript changed, re-render it";
/// `true` means "only the viewport/dock moved, the cached transcript is fine".
/// A parked full render is never downgraded to a cached one, so a streamed
/// delta that lands inside a pure-scroll frame window is still painted.
fn merge_pending_render(parked: Option<bool>, reuse_scroll_content: bool) -> Option<bool> {
    match parked {
        Some(parked) => Some(parked && reuse_scroll_content),
        None => Some(reuse_scroll_content),
    }
}

/// Coalescing render-scheduler state — the Rust port of upstream's
/// `TuiBase.{renderRequested, renderTimer, lastRenderAt}` trio.
struct RenderSchedulerState {
    /// `None` = no frame is owed. `Some(reuse_scroll_content)` records which
    /// render variant the parked request needs.
    pending: Option<bool>,
    /// Set by [`TuiAltScreen::stop`] to end the scheduler thread.
    shutdown: bool,
    /// When the last frame was painted (upstream `lastRenderAt`).
    last_render_at: Instant,
}

/// One scheduler per TUI (never global), so a parked frame can only ever wake
/// the thread that belongs to its own screen.
struct RenderScheduler {
    state: Mutex<RenderSchedulerState>,
    cv: Condvar,
}

impl RenderScheduler {
    fn new() -> Self {
        Self {
            state: Mutex::new(RenderSchedulerState {
                pending: None,
                shutdown: false,
                last_render_at: Instant::now(),
            }),
            cv: Condvar::new(),
        }
    }

    /// Record that a frame is owed and wake the scheduler thread.
    fn park(&self, reuse_scroll_content: bool) {
        if let Ok(mut state) = self.state.lock() {
            state.pending = merge_pending_render(state.pending, reuse_scroll_content);
        }
        // The predicate is checked under the same mutex the waiter holds, so the
        // wakeup cannot be lost between the store above and the wait below.
        self.cv.notify_all();
    }

    /// Note that a frame was just painted, restarting the throttle window.
    fn mark_painted(&self) {
        if let Ok(mut state) = self.state.lock() {
            state.last_render_at = Instant::now();
        }
    }

    /// Take the parked frame, if any.
    fn take_pending(&self) -> Option<bool> {
        self.state.lock().ok()?.pending.take()
    }

    /// Discard any parked frame (an immediate render supersedes it).
    fn clear_pending(&self) {
        if let Ok(mut state) = self.state.lock() {
            state.pending = None;
        }
    }

    /// Signal the scheduler thread to exit and reclaim the parked frame.
    fn shutdown(&self) {
        if let Ok(mut state) = self.state.lock() {
            state.shutdown = true;
            state.pending = None;
        }
        self.cv.notify_all();
    }
}

/// Alternate screen TUI with application-owned scrolling.
pub struct TuiAltScreen {
    /// Serializes the complete diff/render/write transaction. Input, streaming
    /// events, and loader ticks can request frames from different threads; if
    /// they race, an older frame can otherwise overwrite `previous_screen`
    /// after a newer frame has already reached the terminal.
    render_lock: Mutex<()>,
    jump_to_end: crate::jump_to_end::JumpToEnd,
    terminal: Arc<Mutex<Box<dyn Terminal>>>,
    terminal_proxy: TerminalProxy,
    container: Container,
    layout_root: Mutex<Option<Arc<dyn Component>>>,
    running: Arc<Mutex<bool>>,
    show_hardware_cursor: Mutex<bool>,
    clear_on_shrink: Mutex<bool>,
    focused: Mutex<Option<Arc<dyn Component>>>,
    full_redraw_count: Mutex<usize>,
    previous_screen: Mutex<Vec<String>>,
    scroll_top: Mutex<usize>,
    stick_to_bottom: Mutex<bool>,
    current_frame: Mutex<Option<LayoutFrame>>,
    /// When true, render into the terminal's main buffer and let the terminal
    /// own scrollback (upstream's default `regular` mode).
    main_screen_mode: Mutex<bool>,
    main_previous_width: Mutex<usize>,
    main_previous_height: Mutex<usize>,
    main_hardware_row: Mutex<usize>,
    main_viewport_top: Mutex<usize>,
    main_previous_cursor: Mutex<Option<(usize, usize)>>,
    // Input handlers
    #[allow(dead_code)]
    input_handler: Mutex<Option<Arc<dyn Fn(InputEvent) + Send + Sync>>>,
    #[allow(dead_code)]
    resize_handler: Mutex<Option<Arc<dyn Fn() + Send + Sync>>>,
    overlays: Arc<OverlayManager>,
    render_suspended: Mutex<bool>,
    /// Coalescing render scheduler (upstream `TuiBase` frame throttle). While
    /// it is active, `request_render(false)` and
    /// `request_render_reusing_scroll_content()` park a frame instead of
    /// repainting, and the scheduler thread paints at most one frame per
    /// [`MIN_RENDER_INTERVAL_MS`].
    scheduler: Arc<RenderScheduler>,
    /// Whether [`TuiAltScreen::start_render_scheduler`] succeeded. When false,
    /// render requests paint inline (standalone/unit-test behaviour).
    scheduler_started: AtomicBool,
    /// The scheduler thread, joined by [`TuiAltScreen::stop`] so a parked frame
    /// cannot repaint the terminal after shutdown.
    scheduler_thread: Mutex<Option<JoinHandle<()>>>,
    /// Current scheduler throttle window, read by the scheduler thread on
    /// every wake so [`TuiAltScreen::set_render_interval_ms`] takes effect
    /// without a restart. Defaults to [`MIN_RENDER_INTERVAL_MS`].
    render_interval_ms: AtomicU64,
}

impl TuiAltScreen {
    /// Create a new alternate screen TUI.
    pub fn new(
        terminal: Box<dyn Terminal>,
        show_hardware_cursor: bool,
        _log_directory: Option<&str>,
    ) -> Self {
        let terminal = Arc::new(Mutex::new(terminal));
        let terminal_proxy = TerminalProxy {
            terminal: terminal.clone(),
        };
        Self {
            render_lock: Mutex::new(()),
            jump_to_end: crate::jump_to_end::JumpToEnd::default(),
            terminal,
            terminal_proxy,
            container: Container::new(),
            layout_root: Mutex::new(None),
            running: Arc::new(Mutex::new(false)),
            show_hardware_cursor: Mutex::new(show_hardware_cursor),
            clear_on_shrink: Mutex::new(false),
            focused: Mutex::new(None),
            full_redraw_count: Mutex::new(0),
            previous_screen: Mutex::new(Vec::new()),
            scroll_top: Mutex::new(0),
            stick_to_bottom: Mutex::new(true),
            current_frame: Mutex::new(None),
            main_screen_mode: Mutex::new(false),
            main_previous_width: Mutex::new(0),
            main_previous_height: Mutex::new(0),
            main_hardware_row: Mutex::new(0),
            main_viewport_top: Mutex::new(0),
            main_previous_cursor: Mutex::new(None),
            input_handler: Mutex::new(None),
            resize_handler: Mutex::new(None),
            overlays: Arc::new(OverlayManager::new()),
            render_suspended: Mutex::new(false),
            scheduler: Arc::new(RenderScheduler::new()),
            scheduler_started: AtomicBool::new(false),
            scheduler_thread: Mutex::new(None),
            render_interval_ms: AtomicU64::new(MIN_RENDER_INTERVAL_MS),
        }
    }

    /// Adjust the coalesced repaint interval (milliseconds, clamped to
    /// at least [`MIN_RENDER_INTERVAL_MS`] so a bad value cannot disable the
    /// throttle entirely). The scheduler thread picks the new window up on
    /// its next wake; hosts call this before [`TuiAltScreen::start_render_scheduler`]
    /// with [`FALLBACK_RENDER_INTERVAL_MS`] when the terminal lacks
    /// synchronized-output support (see
    /// [`crate::terminal::supports_synchronized_output`]).
    pub fn set_render_interval_ms(&self, ms: u64) {
        let clamped = ms.max(MIN_RENDER_INTERVAL_MS);
        self.render_interval_ms.store(clamped, Ordering::Release);
    }

    /// Current scheduler throttle window in milliseconds (test/diagnostic).
    pub fn render_interval_ms(&self) -> u64 {
        self.render_interval_ms.load(Ordering::Acquire)
    }

    /// Start the coalescing render scheduler — the Rust port of upstream's
    /// `TuiBase.scheduleRender()` timer. Idempotent.
    ///
    /// Every `request_render(false)` / `request_render_reusing_scroll_content()`
    /// call after this parks a frame instead of repainting immediately, and the
    /// scheduler thread paints at most one frame per [`MIN_RENDER_INTERVAL_MS`].
    /// A burst of streamed deltas therefore collapses into a single repaint of
    /// the transcript instead of one per token.
    ///
    /// Hosts that drive their own event loop may instead leave this off and call
    /// [`TuiAltScreen::flush_pending_render`] from their own tick; without either
    /// the requests paint inline (the pre-throttle behaviour).
    ///
    /// Takes `&Arc<Self>` because the scheduler thread keeps the TUI alive until
    /// [`TuiAltScreen::stop`] shuts it down; call it after the TUI has been
    /// wrapped in the `Arc` the host renders through.
    pub fn start_render_scheduler(self: &Arc<Self>) {
        if self.scheduler_started.swap(true, Ordering::AcqRel) {
            return;
        }
        let tui = self.clone();
        let spawned = std::thread::Builder::new()
            .name("rpi-tui-render".to_string())
            .spawn(move || tui.run_render_scheduler());
        match spawned {
            Ok(handle) => {
                if let Ok(mut slot) = self.scheduler_thread.lock() {
                    *slot = Some(handle);
                }
            }
            Err(_) => {
                // A failed spawn must not silently disable repaints: fall back
                // to the synchronous path rather than parking frames forever.
                self.scheduler_started.store(false, Ordering::Release);
            }
        }
    }

    /// Scheduler thread body: sleep until a frame is owed, then paint at most one
    /// frame per the current interval (see
    /// [`TuiAltScreen::set_render_interval_ms`]). Idle screens pay no wakeups at
    /// all — the thread is parked on the condvar.
    fn run_render_scheduler(self: Arc<Self>) {
        loop {
            let interval = Duration::from_millis(self.render_interval_ms.load(Ordering::Acquire));
            // ---- Wait for a frame to be owed (or shutdown) ----
            {
                let Ok(mut state) = self.scheduler.state.lock() else {
                    return;
                };
                while state.pending.is_none() && !state.shutdown {
                    let Ok(next) = self.scheduler.cv.wait(state) else {
                        return;
                    };
                    state = next;
                }
                if state.shutdown {
                    return;
                }
            }
            // ---- Hold the frame back until the throttle window elapses ----
            loop {
                let remaining = {
                    let Ok(state) = self.scheduler.state.lock() else {
                        return;
                    };
                    match interval.checked_sub(state.last_render_at.elapsed()) {
                        Some(remaining) => remaining,
                        None => break,
                    }
                };
                let Ok(state) = self.scheduler.state.lock() else {
                    return;
                };
                if state.shutdown {
                    return;
                }
                let Ok((state, _)) = self.scheduler.cv.wait_timeout(state, remaining) else {
                    return;
                };
                if state.shutdown {
                    return;
                }
            }
            self.flush_pending_render();
        }
    }

    /// Paint a frame parked by [`TUI::request_render`] /
    /// [`TuiAltScreen::request_render_reusing_scroll_content`].
    ///
    /// Returns `true` when a frame was painted, `false` when nothing was owed
    /// (or the owed frame was dropped because the TUI is stopped or a foreign
    /// runtime owns the terminal — both of those paths force a fresh full frame
    /// when they end, so nothing is lost on screen).
    ///
    /// The scheduler thread calls this; it is also the deterministic hook a host
    /// tick or a unit test can use to flush a request synchronously.
    pub fn flush_pending_render(&self) -> bool {
        let Some(reuse_scroll_content) = self.scheduler.take_pending() else {
            return false;
        };
        if !self.is_running() || self.is_render_suspended() {
            return false;
        }
        self.do_render(reuse_scroll_content);
        true
    }

    /// Park a frame request, or paint it inline when no scheduler is running.
    fn park_render(&self, reuse_scroll_content: bool) {
        if !self.scheduler_started.load(Ordering::Acquire) {
            self.do_render(reuse_scroll_content);
            return;
        }
        self.scheduler.park(reuse_scroll_content);
    }

    /// Test seam: enable frame parking *without* spawning the scheduler thread,
    /// so a test can drive [`TuiAltScreen::flush_pending_render`] itself and
    /// observe the coalescing deterministically.
    #[cfg(test)]
    fn enable_render_coalescing(&self) {
        self.scheduler_started.store(true, Ordering::Release);
    }

    /// Record the frame timestamp that gates the next coalesced repaint.
    fn mark_frame_painted(&self) {
        self.scheduler.mark_painted();
    }

    /// Use the terminal's main screen and native scrollback instead of the
    /// constrained alternate-screen viewport. Must be set before start.
    pub fn set_main_screen_mode(&self, enabled: bool) {
        if !self.is_running() {
            *self.main_screen_mode.lock().unwrap() = enabled;
        }
    }

    fn uses_main_screen(&self) -> bool {
        self.main_screen_mode
            .lock()
            .map(|mode| *mode)
            .unwrap_or(false)
    }

    /// Set the layout root component.
    pub fn set_layout_root(&self, component: Option<Arc<dyn Component>>) {
        if let Ok(mut root) = self.layout_root.lock() {
            *root = component;
        }
        self.request_render(false);
    }

    /// Get the layout root.
    pub fn get_layout_root(&self) -> Option<Arc<dyn Component>> {
        self.layout_root.lock().ok()?.clone()
    }

    fn is_running(&self) -> bool {
        self.running.lock().map(|running| *running).unwrap_or(false)
    }

    /// Temporarily suspend the outer application renderer while a foreign
    /// runtime owns the terminal (for example a Node extension's fullscreen
    /// component). The terminal remains in raw/alternate-screen mode; only
    /// background repaint requests are suppressed.
    pub fn set_render_suspended(&self, suspended: bool) {
        if let Ok(mut value) = self.render_suspended.lock() {
            *value = suspended;
        }
        if !suspended {
            self.request_render(true);
        }
    }

    pub fn is_render_suspended(&self) -> bool {
        self.render_suspended
            .lock()
            .map(|value| *value)
            .unwrap_or(false)
    }

    /// Get the current scroll position.
    pub fn viewport_top(&self) -> usize {
        if let Some(scroll) = self.get_primary_scroll_view() {
            return scroll.scroll_top();
        }
        self.scroll_top.lock().map(|s| *s).unwrap_or(0)
    }

    /// Check if following output.
    pub fn is_following_output(&self) -> bool {
        if let Some(scroll) = self.get_primary_scroll_view() {
            return scroll.is_following_end();
        }
        self.stick_to_bottom.lock().map(|s| *s).unwrap_or(true)
    }

    /// Set a clickable label shown while the follow-end primary view is scrolled up.
    /// The label is recomputed for each frame, allowing live theme and key hints.
    pub fn set_scroll_to_end_indicator(
        &self,
        label: Option<Arc<dyn Fn() -> String + Send + Sync>>,
    ) {
        self.jump_to_end.set_label(label);
        if self.is_running() {
            self.request_render_reusing_scroll_content();
        }
    }

    /// Handle a click before the host starts text selection or other mouse actions.
    pub fn handle_scroll_to_end_indicator_mouse_event(
        &self,
        event: &crossterm::event::MouseEvent,
    ) -> bool {
        if self.uses_main_screen()
            || self.is_render_suspended()
            || self.has_overlay()
            || !self.jump_to_end.hit(event)
        {
            return false;
        }
        let Some(scroll) = self.get_primary_scroll_view() else {
            return false;
        };
        if scroll.is_following_end() || !scroll.follows_end() {
            return false;
        }
        self.scroll_to_bottom();
        true
    }

    /// Scroll by the given number of lines.
    pub fn scroll_by(&self, lines: i32) {
        if let Some(scroll) = self.get_primary_scroll_view() {
            scroll.scroll_by(lines);
            self.request_render_reusing_scroll_content();
            return;
        }
        if let Ok(mut scroll_top) = self.scroll_top.lock() {
            if lines > 0 {
                *scroll_top = scroll_top.saturating_add(lines as usize);
            } else {
                *scroll_top = scroll_top.saturating_sub((-lines) as usize);
            }
        }
        if lines != 0 {
            if let Ok(mut stick) = self.stick_to_bottom.lock() {
                *stick = lines > 0; // Re-enable stick on scroll down
            }
        }
        self.request_render(false);
    }

    /// Scroll to the top.
    pub fn scroll_to_top(&self) {
        if let Some(scroll) = self.get_primary_scroll_view() {
            scroll.scroll_to_start();
            self.request_render_reusing_scroll_content();
            return;
        }
        if let Ok(mut scroll_top) = self.scroll_top.lock() {
            *scroll_top = 0;
        }
        if let Ok(mut stick) = self.stick_to_bottom.lock() {
            *stick = false;
        }
        self.request_render(false);
    }

    /// Scroll to the bottom.
    pub fn scroll_to_bottom(&self) {
        if let Some(scroll) = self.get_primary_scroll_view() {
            scroll.scroll_to_end();
            self.request_render_reusing_scroll_content();
            return;
        }
        if let Ok(mut stick) = self.stick_to_bottom.lock() {
            *stick = true;
        }
        self.request_render(false);
    }

    /// Get the primary scroll view if any.
    pub fn get_primary_scroll_view(&self) -> Option<Arc<ScrollView>> {
        self.current_frame
            .lock()
            .ok()?
            .as_ref()?
            .primary_scroll_view
            .clone()
    }

    /// Get the current terminal column count (cached, refreshed on resize).
    /// Used by callers that need a width for off-layout rendering (e.g.
    /// diff-line width sizing) without re-reading the terminal themselves.
    pub fn width(&self) -> usize {
        self.terminal.lock().map(|t| t.columns()).unwrap_or(80)
    }

    /// Set the terminal window/tab title.
    ///
    /// The [`Terminal`] trait's `set_title` on the real `ProcessTerminal` emits
    /// `\x1b]2;{title}\x07` (OSC 2); the `DummyTerminal` stub is a no-op. This
    /// accessor locks the real underlying terminal (bypassing the no-op trait
    /// impl returned by [`TUI::terminal`]) so hosts can reflect run state in
    /// the window title — e.g. "rpi — working" while a turn is in flight.
    pub fn set_title(&self, title: &str) {
        if let Ok(terminal) = self.terminal.lock() {
            terminal.set_title(title);
            terminal.flush();
        }
    }

    /// Check if this implements ViewportTUI.
    pub fn is_viewport_tui(&self) -> bool {
        true
    }

    /// Enter alternate screen mode.
    /// Refresh the cached terminal size (call on `Event::Resize`) and force a
    /// full redraw so the constrained layout re-fits the new dimensions.
    pub fn refresh_size(&self) {
        if let Ok(terminal) = self.terminal.lock() {
            terminal.refresh_size();
        }
        // Alt-screen frames need every viewport row repainted. Main-screen mode
        // keeps the previous document so its renderer can detect width changes
        // and deliberately rebuild terminal scrollback once.
        if !self.uses_main_screen() {
            if let Ok(mut prev) = self.previous_screen.lock() {
                prev.clear();
            }
        }
        self.request_render(false);
    }

    fn enter_alt_screen(&self) {
        if let Ok(terminal) = self.terminal.lock() {
            // Enter alternate screen buffer and disable autowrap
            terminal.write("\x1b[?1049h\x1b[?7l");
            // Clear screen and hide cursor
            terminal.write("\x1b[2J\x1b[H\x1b[?25l");
            terminal.flush();
        }
    }

    /// Exit alternate screen mode.
    fn exit_alt_screen(&self, preserve_screen: bool) {
        if let Ok(terminal) = self.terminal.lock() {
            // Autowrap is a terminal mode, not restored by leaving the buffer.
            terminal.write("\x1b[?7h");
            if preserve_screen {
                terminal.write("\x1b[?1049l\x1b[?25h");
            } else {
                // Exit the alt buffer CLEANLY: back to the main buffer, clear
                // it, and show the cursor. The old path re-rendered the full
                // TUI frame (borders, backgrounds, spinner state, cursor
                // markers) into the main buffer, which is exactly the garbage
                // that made quitting look scrambled. The caller prints its own
                // farewell after this.
                terminal.write("\x1b[?1049l\x1b[2J\x1b[H\x1b[?25h");
            }
            terminal.flush();
        }
    }

    /// Hand the terminal back to the shell so the process can be stopped
    /// (upstream's `handleCtrlZ`): pause the renderer, leave the alternate
    /// buffer, and restore cooked mode. [`TuiAltScreen::resume`] reverses it
    /// once `SIGTSTP` returns.
    ///
    /// Unlike the private `stop`, this keeps the render scheduler alive, so the
    /// same TUI can be resumed in place.
    pub fn suspend(&self) {
        self.set_render_suspended(true);
        if let Ok(terminal) = self.terminal.lock() {
            terminal.disable_mouse();
            terminal.exit_raw_mode();
            terminal.flush();
        }
        if !self.uses_main_screen() {
            // `preserve_screen`: leave the alt buffer without clearing the
            // user's main-buffer scrollback (unlike a normal shutdown).
            self.exit_alt_screen(true);
        }
    }

    /// Re-enter raw mode and the alternate buffer after the process resumes
    /// from `SIGTSTP`, then force a full repaint (the alt buffer came back
    /// empty — `set_render_suspended` is what schedules it).
    pub fn resume(&self) {
        if !self.uses_main_screen() {
            self.enter_alt_screen();
        }
        if let Ok(terminal) = self.terminal.lock() {
            terminal.enter_raw_mode();
            if self.uses_main_screen() {
                terminal.disable_mouse();
            } else {
                terminal.enable_mouse();
            }
            terminal.flush();
        }
        self.set_render_suspended(false);
    }

    /// True when both rows render as pure whitespace AND carry only "neutral"
    /// escapes — SGR reset / default fg / default bg / normal intensity, and
    /// the OSC 8 hyperlink close. Those have no visible effect on a blank row.
    /// Rows with painted attributes (backgrounds, colors, non-SGR CSI moves)
    /// are NOT equivalent: the scrollbar and tool-panel blank rows rely on
    /// their painted background, and a lost repaint would leave a stale bg.
    fn blank_row_pair(old: Option<&String>, new: Option<&String>) -> bool {
        match (old, new) {
            (Some(old), Some(new)) => Self::neutral_blank_row(old) && Self::neutral_blank_row(new),
            _ => false,
        }
    }

    /// Manual scan of one row: every byte must be a space, an `\x1b[0m`-family
    /// SGR (params all in {empty, 0, 22, 39, 49}), or the OSC 8 close
    /// `\x1b]8;;\x07`. Anything else — painted SGR, other CSI, tabs, text —
    /// disqualifies the row.
    fn neutral_blank_row(line: &str) -> bool {
        let bytes = line.as_bytes();
        let mut i = 0usize;
        while i < bytes.len() {
            match bytes[i] {
                b' ' => i += 1,
                0x1b => {
                    let rest = &line[i..];
                    if rest.starts_with("\x1b]8;;\x07") {
                        i += "\x1b]8;;\x07".len();
                        continue;
                    }
                    if !rest.starts_with("\x1b[") {
                        return false;
                    }
                    // Only m-terminated SGR can be neutral; other CSI (moves,
                    // erases) is positional and must be repainted.
                    let Some(end) = rest[2..].bytes().position(|b| (0x40..=0x7e).contains(&b))
                    else {
                        return false;
                    };
                    if &rest[2 + end..3 + end] != "m" {
                        return false;
                    }
                    let params = &rest[2..2 + end];
                    if !params
                        .split(';')
                        .all(|p| matches!(p, "" | "0" | "m" | "22" | "39" | "49"))
                    {
                        return false;
                    }
                    i += 3 + end;
                }
                _ => return false,
            }
        }
        true
    }

    /// Render the complete component tree into the main terminal buffer. This
    /// follows upstream's regular-mode strategy: append growth with CRLF so
    /// the terminal creates real scrollback, and rewrite only the changed tail.
    /// Rewrite the whole main-screen document without a leading blank-out.
    ///
    /// The classic rebuild cleared with `\x1b[2J` first and then rewrote every
    /// row, painting a fully blank frame in between. Terminals that ignore the
    /// CSI 2026 synchronized-output wrapper (tty7, Apple Terminal, Alacritty)
    /// expose that window as visible flicker. Rewriting over the old content
    /// instead — home, overwrite each row, erase each line's leftover tail
    /// with EL, then one ED0 for the viewport remainder — keeps every row
    /// populated at all times, shrinking the artifact to a transient per-row
    /// tear. Synchronized-output terminals are unaffected: the whole frame is
    /// atomic there, so the ordering does not matter.
    ///
    /// `clear_scrollback` additionally emits ED3 for full rebuilds (size
    /// change or a scrollback hit), mirroring the original rebuild semantics;
    /// the first-frame path passes `false` so the user's own shell scrollback
    /// is not wiped when the TUI starts.
    fn write_main_screen_rebuild(
        &self,
        terminal: &dyn Terminal,
        lines: &[String],
        width: usize,
        height: usize,
        clear_scrollback: bool,
    ) -> usize {
        terminal.write("\x1b[?2026h");
        if clear_scrollback {
            // The scrollback is not addressable by cursor movement, so stale
            // history rows must be dropped wholesale on a rebuild.
            terminal.write("\x1b[3J");
        }
        terminal.write("\x1b[H");
        let mut rebuilt = String::new();
        for (index, line) in lines.iter().enumerate() {
            if index > 0 {
                rebuilt.push_str("\r\n");
            }
            rebuilt.push_str(line);
            // EL0 erases *including* the cursor cell and cancels pending wrap
            // (Ghostty/xterm semantics), so a full-width row would lose its
            // last character. A full-width row cannot have a longer leftover
            // anyway — only shorter rows need the tail erase.
            if visible_width(line) < width {
                rebuilt.push_str("\x1b[K");
            }
        }
        terminal.write(&rebuilt);
        // Tail wipe. ED0 likewise starts at the cursor cell, so after a
        // full-width last row (cursor pending-wrap on its last column) a bare
        // ED0 would erase that row's final character. When the document is
        // shorter than the viewport the last row sits above the bottom, so a
        // CR+LF is safe there (it cancels pending wrap and moves one row
        // down without scrolling) and the ED0 wipes the old rows below the
        // shrunken document. When the document fills the viewport the
        // cursor is already on the bottom row — no residue exists and a LF
        // would scroll, so skip the wipe entirely.
        let last_row_full = lines
            .last()
            .map(|line| visible_width(line) >= width)
            .unwrap_or(false);
        let has_residual = lines.len() < height;
        if has_residual {
            if last_row_full {
                terminal.write("\r\n\x1b[J");
                terminal.write("\x1b[?2026l");
                return lines.len();
            }
            terminal.write("\x1b[J");
        }
        terminal.write("\x1b[?2026l");
        lines.len().saturating_sub(1)
    }

    fn do_render_main_screen(&self) {
        let Ok(_render_guard) = self.render_lock.lock() else {
            return;
        };
        // A render request may have passed the public suspension check while
        // a custom UI was opening. Re-check after taking the render lock so an
        // already queued outer frame cannot overwrite the foreign screen.
        if self.is_render_suspended() {
            return;
        }
        let Ok(terminal) = self.terminal.lock() else {
            return;
        };
        let width = terminal.columns();
        let height = terminal.rows();
        let root: Arc<dyn Component> = self
            .get_layout_root()
            .unwrap_or_else(|| Arc::new(self.container.clone()));
        let raw_lines = root.render(width);
        let cursor = raw_lines.iter().enumerate().rev().find_map(|(row, line)| {
            line.find(CURSOR_MARKER)
                .map(|idx| (row, visible_width(&line[..idx])))
        });
        let lines: Vec<String> = raw_lines
            .into_iter()
            .map(|line| line.replace(CURSOR_MARKER, ""))
            .collect();
        let previous = self
            .previous_screen
            .lock()
            .map(|p| p.clone())
            .unwrap_or_default();
        let previous_width = *self.main_previous_width.lock().unwrap();
        let previous_height = *self.main_previous_height.lock().unwrap();
        let previous_cursor = *self.main_previous_cursor.lock().unwrap();
        terminal.write(&crate::terminal_image::removed_kitty_placements(
            &previous, &lines,
        ));

        let mut first_changed = None;
        let mut last_changed = None;
        for index in 0..previous.len().max(lines.len()) {
            if previous.get(index) != lines.get(index) {
                first_changed.get_or_insert(index);
                last_changed = Some(index);
            }
        }
        let viewport_top = *self.main_viewport_top.lock().unwrap();
        let changed_in_scrollback = first_changed.is_some_and(|first| {
            first < viewport_top || lines.len().saturating_sub(1) < viewport_top
        });

        // Saved scrollback cannot be addressed by cursor movement or CSI T:
        // SD only inserts blanks in the live screen. Rebuild the document when
        // an old row changes, otherwise repainting its tail appends duplicate
        // history until a bounded terminal buffer evicts the conversation start.
        // Size changes also require a rebuild; ordinary screen-local updates
        // remain incremental.
        if (previous_width != 0 && previous_width != width)
            || (previous_height != 0 && previous_height != height)
            || changed_in_scrollback
        {
            *self.main_hardware_row.lock().unwrap() =
                self.write_main_screen_rebuild(&**terminal, &lines, width, height, true);
            *self.main_viewport_top.lock().unwrap() = lines.len().saturating_sub(height);
        } else if previous.is_empty() {
            // First frame: the baseline behaviour appends from wherever the
            // shell cursor sits, leaving the user's visible output on screen
            // (only the saved scrollback is preserved by *not* emitting ED3).
            // Homing to the top would paint over that visible output, so keep
            // the original append path — full-width-row EL/ED0 hazards do not
            // apply here because nothing is erased.
            terminal.write("\x1b[?2026h");
            terminal.write(&lines.join("\r\n"));
            terminal.write("\x1b[?2026l");
            *self.main_hardware_row.lock().unwrap() = lines.len().saturating_sub(1);
            *self.main_viewport_top.lock().unwrap() = lines.len().saturating_sub(height);
        } else {
            if first_changed.is_none() && cursor == previous_cursor {
                return;
            }
            if let (Some(first), Some(last)) = (first_changed, last_changed) {
                let mut hardware_row = *self.main_hardware_row.lock().unwrap();
                let mut viewport_top = *self.main_viewport_top.lock().unwrap();
                let viewport_bottom = viewport_top + height.saturating_sub(1);
                let deleted_tail_only = first >= lines.len();
                let append_start = first == previous.len() && first > 0;
                let move_target = if deleted_tail_only {
                    lines.len().saturating_sub(1)
                } else if append_start {
                    first - 1
                } else {
                    first
                };
                let mut output = String::from("\x1b[?2026h");
                if move_target > viewport_bottom {
                    let current_screen = hardware_row
                        .saturating_sub(viewport_top)
                        .min(height.saturating_sub(1));
                    let down = height.saturating_sub(1).saturating_sub(current_screen);
                    if down > 0 {
                        output.push_str(&format!("\x1b[{down}B"));
                    }
                    let scroll = move_target - viewport_bottom;
                    output.push_str(&"\r\n".repeat(scroll));
                    viewport_top += scroll;
                    hardware_row = move_target;
                }
                let current_screen = hardware_row.saturating_sub(viewport_top);
                let target_screen = move_target.saturating_sub(viewport_top);
                if target_screen > current_screen {
                    output.push_str(&format!("\x1b[{}B", target_screen - current_screen));
                } else if current_screen > target_screen {
                    output.push_str(&format!("\x1b[{}A", current_screen - target_screen));
                }
                output.push_str(if append_start { "\r\n" } else { "\r" });
                let render_end = last.min(lines.len().saturating_sub(1));
                if !deleted_tail_only {
                    for index in first..=render_end {
                        if index > first {
                            output.push_str("\r\n");
                        }
                        // Blank-vs-blank rows carry no visible content: the
                        // only diff between them is a style tail. Emitting a
                        // column jump plus resets for each such row is pure
                        // noise in the output byte stream, so keep the row
                        // structure but write no content.
                        if Self::blank_row_pair(previous.get(index), lines.get(index)) {
                            continue;
                        }
                        if let Some((column, text)) = previous
                            .get(index)
                            .and_then(|old| crate::line_diff::patch(old, &lines[index]))
                        {
                            if column > 0 {
                                output.push_str(&format!("\x1b[{column}C"));
                            }
                            output.push_str(&text);
                        } else {
                            output.push_str(&lines[index]);
                            if index < previous.len()
                                && visible_width(&lines[index]) < visible_width(&previous[index])
                            {
                                output.push_str("\x1b[K");
                            }
                        }
                    }
                }
                let mut final_row = render_end;
                if previous.len() > lines.len() {
                    let extra_lines = previous.len() - lines.len();
                    let clear_start_offset = usize::from(!lines.is_empty());
                    if clear_start_offset > 0 {
                        output.push_str("\x1b[1B");
                    }
                    for index in 0..extra_lines {
                        output.push_str("\r\x1b[2K");
                        if index + 1 < extra_lines {
                            output.push_str("\x1b[1B");
                        }
                    }
                    let move_back = extra_lines.saturating_sub(1) + clear_start_offset;
                    if move_back > 0 {
                        output.push_str(&format!("\x1b[{move_back}A"));
                    }
                    if extra_lines > 0 {
                        final_row = lines.len().saturating_sub(1);
                    }
                }
                output.push_str("\x1b[?2026l");
                terminal.write(&output);
                let advanced = final_row.saturating_sub(viewport_top + height.saturating_sub(1));
                viewport_top += advanced;
                *self.main_hardware_row.lock().unwrap() = final_row;
                *self.main_viewport_top.lock().unwrap() = viewport_top;
            }
        }

        if let Some((row, col)) = cursor {
            let hardware_row = *self.main_hardware_row.lock().unwrap();
            let mut viewport_top = *self.main_viewport_top.lock().unwrap();
            let viewport_bottom = viewport_top + height.saturating_sub(1);

            if row > viewport_bottom {
                // The editor row is below the visible window (a write scrolled
                // up to an edit above the old viewport). Scroll back down so
                // the editor is the bottom row, then move the cursor there.
                let scroll = row - viewport_bottom;
                let mut output = String::new();
                let current_screen = hardware_row
                    .saturating_sub(viewport_top)
                    .min(height.saturating_sub(1));
                let down = height.saturating_sub(1).saturating_sub(current_screen);
                if down > 0 {
                    output.push_str(&format!("\x1b[{down}B"));
                }
                output.push_str(&"\r\n".repeat(scroll));
                terminal.write(&output);
                viewport_top += scroll;
            } else if row < viewport_top {
                // The editor row is above the visible window. Scroll up so it
                // becomes the top row, then repaint the revealed viewport — SD
                // leaves the scrolled-off bottom rows blank otherwise.
                let scroll = viewport_top - row;
                terminal.write(&format!("\x1b[H\x1b[{scroll}T"));
                viewport_top = row;
                let mut output = String::from("\x1b[?2026h");
                for i in 0..height {
                    let line = lines
                        .get(viewport_top + i)
                        .map(String::as_str)
                        .unwrap_or("");
                    if i > 0 {
                        output.push_str("\r\n");
                    }
                    output.push_str("\x1b[2K");
                    output.push_str(line);
                }
                // Return the cursor to the top row (the editor row) so the
                // column move below lands on it.
                output.push_str(&format!("\x1b[{}A", height.saturating_sub(1)));
                output.push_str("\x1b[?2026l");
                terminal.write(&output);
            } else {
                // Within the window: a relative move is enough (and never
                // scrolls, so it cannot drift the viewport bookkeeping).
                let current_screen = hardware_row
                    .saturating_sub(viewport_top)
                    .min(height.saturating_sub(1));
                let target_screen = row - viewport_top;
                if target_screen > current_screen {
                    terminal.write(&format!("\x1b[{}B", target_screen - current_screen));
                } else if current_screen > target_screen {
                    terminal.write(&format!("\x1b[{}A", current_screen - target_screen));
                }
            }

            terminal.write(&format!("\r\x1b[{}C", col));
            terminal.write("\x1b[?25h");
            *self.main_hardware_row.lock().unwrap() = row;
            *self.main_viewport_top.lock().unwrap() = viewport_top;
        } else {
            terminal.write("\x1b[?25l");
        }
        terminal.flush();
        *self.previous_screen.lock().unwrap() = lines;
        *self.main_previous_width.lock().unwrap() = width;
        *self.main_previous_height.lock().unwrap() = height;
        *self.main_previous_cursor.lock().unwrap() = cursor;
    }

    /// Perform a differential render with constrained layout.
    fn do_render(&self, reuse_scroll_content: bool) {
        if self.is_render_suspended() {
            return;
        }
        // upstream stamps the frame time immediately before `doRender()`; the
        // scheduler reads it to hold the next request back for the remainder of
        // the throttle window.
        self.mark_frame_painted();
        if self.uses_main_screen() {
            self.do_render_main_screen();
            return;
        }
        let Ok(_render_guard) = self.render_lock.lock() else {
            return;
        };
        if self.is_render_suspended() {
            return;
        }

        let (width, height) = if let Ok(terminal) = self.terminal.lock() {
            (terminal.columns(), terminal.rows())
        } else {
            return;
        };

        // Get layout root or use container
        let root = self.get_layout_root();
        let root_component: Arc<dyn Component> =
            root.unwrap_or_else(|| Arc::new(self.container.clone()));

        // Render layout frame
        let frame = if reuse_scroll_content {
            render_layout_frame_reusing_scroll_content(root_component.clone(), width, height)
        } else {
            render_layout_frame(root_component.clone(), width, height)
        };

        // Store the frame for input handling
        if let Ok(mut current) = self.current_frame.lock() {
            *current = Some(frame.clone());
        }

        // Get screen lines and composite modal overlays above the layout.
        let mut visible = frame.lines.clone();
        self.jump_to_end
            .paint(&frame, &mut visible, self.has_overlay());
        self.paint_overlays(&mut visible, width, height);

        // Cursor markers are an internal layout protocol, not terminal output.
        // Extract the location first, then strip every marker before diffing or
        // writing. Sending the APC marker to some Windows terminals caused the
        // character under the cursor to be erased when moving left/right.
        let cursor = extract_cursor_position(&visible, height);
        for line in &mut visible {
            *line = line.replace(CURSOR_MARKER, "");
        }

        // Check for full redraw
        let previous = self
            .previous_screen
            .lock()
            .map(|p| p.clone())
            .unwrap_or_default();
        let full_redraw = previous.len() != height || previous.is_empty();

        // Build output buffer
        let mut buffer = String::new();
        buffer.push_str(&crate::terminal_image::removed_kitty_placements(
            &previous, &visible,
        ));

        if let Ok(terminal) = self.terminal.lock() {
            if full_redraw {
                if let Ok(mut count) = self.full_redraw_count.lock() {
                    *count += 1;
                }
                // No leading `\x1b[2J`: blanking the screen before the rewrite
                // paints a fully blank frame that terminals without CSI 2026
                // support expose as flicker. Every row is rewritten below, so
                // overwriting in place + per-row EL is enough.
            }

            // Begin synchronized output
            buffer.push_str("\x1b[?2026h");

            // Write changed lines
            for (row, line) in visible.iter().enumerate() {
                if !full_redraw && row < previous.len() && &previous[row] == line {
                    continue; // Skip unchanged lines
                }
                // Overwrite before clearing any leftover tail. Clearing the
                // whole row first blanks stationary sidebars during scroll on
                // terminals that do not implement synchronized output.
                if let Some((column, text)) = (!full_redraw)
                    .then(|| crate::line_diff::patch(&previous[row], line))
                    .flatten()
                {
                    buffer.push_str(&format!("\x1b[{};{}H{}", row + 1, column + 1, text));
                } else {
                    buffer.push_str(&format!("\x1b[{};1H{}", row + 1, line));
                    // On a full redraw there is no previous row to compare
                    // widths against, but an old longer row may still be on
                    // screen (no leading clear), so erase the tail. EL0
                    // includes the cursor cell (and cancels pending wrap),
                    // so a full-width row must skip it — its last character
                    // sits exactly on the cursor cell — and cannot have a
                    // longer leftover anyway.
                    let row_full_width = visible_width(line) >= width;
                    let old_row_shorter = !full_redraw
                        && row < previous.len()
                        && visible_width(line) >= visible_width(&previous[row]);
                    if !old_row_shorter && !row_full_width {
                        buffer.push_str("\x1b[K");
                    }
                }
            }

            // Position the hardware cursor using the marker location captured
            // before internal markers were stripped from `visible`.
            if let Some((row, col)) = cursor {
                buffer.push_str(&format!("\x1b[{};{}H", row + 1, col + 1));
                if self.get_show_hardware_cursor() {
                    buffer.push_str("\x1b[?25h");
                }
            } else {
                // Hide cursor if no cursor marker found
                buffer.push_str("\x1b[?25l");
            }

            // End synchronized output
            buffer.push_str("\x1b[?2026l");

            terminal.write(&buffer);
            terminal.flush();
        }

        // Store current screen
        if let Ok(mut prev) = self.previous_screen.lock() {
            *prev = visible;
        }
    }

    fn paint_overlays(&self, lines: &mut [String], width: usize, height: usize) {
        for overlay in self.overlays.render_visible(width, height) {
            let (x, y, overlay_width) = (overlay.x, overlay.y, overlay.width);
            let overlay_lines = overlay.lines;
            let overlay_height = overlay_lines.len();
            for (index, line) in overlay_lines.iter().take(overlay_height).enumerate() {
                let row = y + index;
                if row >= lines.len() {
                    break;
                }
                lines[row] = composite_tui_line(&lines[row], line, x, overlay_width, width);
            }
        }
    }
}

impl Component for TuiAltScreen {
    fn render(&self, width: usize) -> Vec<String> {
        self.get_layout_root()
            .map(|r| r.render(width))
            .unwrap_or_else(|| self.container.render(width))
    }

    fn invalidate(&self) {
        if let Some(root) = self.get_layout_root() {
            root.invalidate();
        } else {
            self.container.invalidate();
        }
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

impl TUI for TuiAltScreen {
    fn mode(&self) -> TuiMode {
        if self.uses_main_screen() {
            TuiMode::Regular
        } else {
            TuiMode::Fullscreen
        }
    }

    fn terminal(&self) -> &dyn Terminal {
        &self.terminal_proxy
    }

    fn children(&self) -> Vec<Arc<dyn Component>> {
        self.container.get_children()
    }

    fn add_child(&self, component: Arc<dyn Component>) {
        self.container.add_child(component);
    }

    fn remove_child(&self, component: &Arc<dyn Component>) {
        self.container.remove_child(component);
    }

    fn clear(&self) {
        self.container.clear();
    }

    fn get_show_hardware_cursor(&self) -> bool {
        self.show_hardware_cursor
            .lock()
            .map(|s| *s)
            .unwrap_or(false)
    }

    fn set_show_hardware_cursor(&self, enabled: bool) {
        if let Ok(mut show) = self.show_hardware_cursor.lock() {
            *show = enabled;
        }
    }

    fn get_clear_on_shrink(&self) -> bool {
        self.clear_on_shrink.lock().map(|s| *s).unwrap_or(false)
    }

    fn set_clear_on_shrink(&self, enabled: bool) {
        if let Ok(mut clear) = self.clear_on_shrink.lock() {
            *clear = enabled;
        }
    }

    fn set_focus(&self, component: Option<Arc<dyn Component>>) {
        if let Ok(mut focused) = self.focused.lock() {
            *focused = component;
        }
    }

    fn get_focus(&self) -> Option<Arc<dyn Component>> {
        self.focused.lock().ok()?.clone()
    }

    fn show_overlay(
        &self,
        component: Arc<dyn Component>,
        options: Option<OverlayOptions>,
    ) -> Arc<dyn OverlayHandle> {
        let handle = self.overlays.add(component, options.unwrap_or_default());
        self.request_render(false);
        Arc::new(ManagedOverlayHandle {
            handle,
            hidden: Mutex::new(false),
            focused: Mutex::new(true),
        })
    }

    fn hide_overlay(&self) {
        self.overlays.remove_topmost();
        self.request_render(false);
    }

    fn has_overlay(&self) -> bool {
        !self.overlays.get_visible().is_empty()
    }

    fn start(&self) {
        if let Ok(mut running) = self.running.lock() {
            *running = true;
        }

        // Clone the necessary Arc references for the closures
        let running = self.running.clone();

        // Keyboard protocol stacks belong to each screen buffer. Switch first
        // so the terminal enables its protocol on the screen that receives input.
        if !self.uses_main_screen() {
            self.enter_alt_screen();
        }

        // Get terminal and start it (spawns its own input-reader thread whose
        // callbacks are stubs below; kept for the non-pi-tui callers that still
        // rely on `start()`). The asynchronous interactive path uses
        // `start_readerless` instead to avoid a competing stdin reader.
        if let Ok(terminal) = self.terminal.lock() {
            terminal.start(
                Box::new(move |event: InputEvent| {
                    if !*running.lock().unwrap() {
                        return;
                    }
                    let _ = event; // stub: input handled by the caller's own loop
                }),
                Box::new(move || {}),
            );
            if self.uses_main_screen() {
                terminal.disable_mouse();
            } else {
                terminal.enable_mouse();
            }
            terminal.flush();
        }

        self.do_render(false);
    }

    fn stop(&self, options: TuiStopOptions) {
        if let Ok(mut running) = self.running.lock() {
            *running = false;
        }

        // Stop the coalescing scheduler *before* the terminal goes away, and
        // join it so a frame already in flight cannot repaint the screen after
        // shutdown (the class of bug the 0.1.18 regular-mode fix targets).
        self.scheduler.shutdown();
        if let Some(handle) = self.scheduler_thread.lock().ok().and_then(|mut s| s.take()) {
            let _ = handle.join();
        }

        if self.uses_main_screen() {
            if let Ok(terminal) = self.terminal.lock() {
                // Leave the shell prompt below the rendered footer while
                // preserving all conversation rows in terminal scrollback.
                terminal.write("\x1b[?25h\r\n");
                terminal.stop();
            }
        } else {
            // Pop the keyboard protocol on its owning screen before leaving it.
            if let Ok(terminal) = self.terminal.lock() {
                terminal.stop();
            }
            self.exit_alt_screen(options.preserve_screen);
        }
    }

    fn render_now(&self, force: bool) {
        // Layout is assembled before `start()`. Rendering during that phase
        // writes the future TUI frame into the main screen, which is then
        // restored on exit and appears as uncleared output.
        if !self.is_running() {
            return;
        }
        if self.is_render_suspended() {
            return;
        }
        if force {
            if let Ok(mut prev) = self.previous_screen.lock() {
                prev.clear();
            }
        }
        // An immediate frame supersedes a parked coalesced one: upstream's
        // `renderNow` clears `renderRequested` and cancels the render timer.
        self.scheduler.clear_pending();
        self.do_render(false);
    }

    fn request_render(&self, force: bool) {
        if self.is_render_suspended() {
            return;
        }
        if force {
            // upstream `requestRender(true)`: reset the diff state and paint an
            // immediate frame instead of waiting out the throttle window.
            self.render_now(true);
            return;
        }
        if !self.is_running() {
            return;
        }
        // Coalesced (upstream `requestRender(false)`): park the frame and let
        // the scheduler paint it at most once per `MIN_RENDER_INTERVAL_MS`.
        self.park_render(false);
    }

    fn full_redraws(&self) -> usize {
        self.full_redraw_count.lock().map(|c| *c).unwrap_or(0)
    }
}

impl TuiAltScreen {
    /// Set up the tty and render the first frame **without** spawning the
    /// `ProcessTerminal` input-reader thread. The caller owns the input loop
    /// (e.g. a `spawn_blocking` `event::read()` loop) and handles resize/key
    /// events directly via [`TuiAltScreen::refresh_size`] / its key dispatch.
    ///
    /// The first frame is painted inline. Hosts that want the native-pi frame
    /// throttle should also call [`TuiAltScreen::start_render_scheduler`].
    ///
    /// This avoids two `event::read()` consumers racing the same stdin queue
    /// (the stub thread in [`TUI::start`] dropped a fraction of keystrokes).
    /// The caller is responsible for `enable_raw_mode`-dependent event reads
    /// and for calling `refresh_size` on `Event::Resize`.
    pub fn start_readerless(&self) {
        if let Ok(mut running) = self.running.lock() {
            *running = true;
        }
        if !self.uses_main_screen() {
            self.enter_alt_screen();
        }
        if let Ok(terminal) = self.terminal.lock() {
            terminal.enter_raw_mode();
            if self.uses_main_screen() {
                // Give wheel/touchpad gestures back to the terminal emulator;
                // it can now scroll its native history smoothly.
                terminal.disable_mouse();
            } else {
                // On macOS enter_raw_mode intentionally leaves mouse tracking
                // disabled so the main screen can use native scrollback. The
                // alternate-screen transcript owns scrolling, however, so it
                // must explicitly re-enable wheel/touchpad events here.
                terminal.enable_mouse();
            }
            terminal.flush();
        }
        self.do_render(false);
    }

    /// Render a frame while reusing scroll-view content. This is appropriate
    /// when only the viewport or bottom dock changed. A missing cache or width
    /// change falls back to rendering fresh content automatically.
    ///
    /// Coalesced like [`TUI::request_render`]; a request that upgrades the parked
    /// frame to a full transcript render wins.
    pub fn request_render_reusing_scroll_content(&self) {
        if !self.is_running() || self.is_render_suspended() {
            return;
        }
        self.park_render(true);
    }
}

/// Check if a TUI implements ViewportTUI.
pub fn is_viewport_tui(_tui: &dyn TUI) -> bool {
    _tui.as_any().is::<TuiAltScreen>()
}

struct ManagedOverlayHandle {
    handle: super::overlay::OverlayHandle,
    hidden: Mutex<bool>,
    focused: Mutex<bool>,
}

impl OverlayHandle for ManagedOverlayHandle {
    fn hide(&self) {
        self.handle.hide();
        if let Ok(mut hidden) = self.hidden.lock() {
            *hidden = true;
        }
    }

    fn set_hidden(&self, hidden: bool) {
        self.handle.set_visible(!hidden);
        if let Ok(mut current) = self.hidden.lock() {
            *current = hidden;
        }
    }

    fn is_hidden(&self) -> bool {
        self.hidden.lock().map(|hidden| *hidden).unwrap_or(true)
    }

    fn focus(&self) {
        if let Ok(mut focused) = self.focused.lock() {
            *focused = true;
        }
    }

    fn is_focused(&self) -> bool {
        self.focused.lock().map(|focused| *focused).unwrap_or(false)
    }
}

/// Shared terminal proxy exposed through [`TUI::terminal`]. The TUI owns the
/// terminal behind a mutex because render and input paths can run concurrently,
/// while callers of the trait need a stable `&dyn Terminal` reference.
struct TerminalProxy {
    terminal: Arc<Mutex<Box<dyn Terminal>>>,
}

impl TerminalProxy {
    fn with<R>(&self, f: impl FnOnce(&dyn Terminal) -> R) -> Option<R> {
        self.terminal
            .lock()
            .ok()
            .map(|terminal| f(terminal.as_ref()))
    }
}

impl Terminal for TerminalProxy {
    fn info(&self) -> TerminalInfo {
        self.with(|terminal| terminal.info()).unwrap_or_default()
    }

    fn write(&self, data: &str) {
        let _ = self.with(|terminal| terminal.write(data));
    }

    fn hide_cursor(&self) {
        let _ = self.with(|terminal| terminal.hide_cursor());
    }

    fn show_cursor(&self) {
        let _ = self.with(|terminal| terminal.show_cursor());
    }

    fn move_cursor(&self, row: usize, col: usize) {
        let _ = self.with(|terminal| terminal.move_cursor(row, col));
    }

    fn clear_screen(&self) {
        let _ = self.with(|terminal| terminal.clear_screen());
    }

    fn set_title(&self, title: &str) {
        let _ = self.with(|terminal| terminal.set_title(title));
    }

    fn enable_mouse(&self) {
        let _ = self.with(|terminal| terminal.enable_mouse());
    }

    fn disable_mouse(&self) {
        let _ = self.with(|terminal| terminal.disable_mouse());
    }

    fn enter_raw_mode(&self) {
        let _ = self.with(|terminal| terminal.enter_raw_mode());
    }

    fn exit_raw_mode(&self) {
        let _ = self.with(|terminal| terminal.exit_raw_mode());
    }

    fn refresh_size(&self) {
        let _ = self.with(|terminal| terminal.refresh_size());
    }

    fn start(
        &self,
        on_input: Box<dyn Fn(InputEvent) + Send + Sync>,
        on_resize: Box<dyn Fn() + Send + Sync>,
    ) {
        let _ = self.with(|terminal| terminal.start(on_input, on_resize));
    }

    fn stop(&self) {
        let _ = self.with(|terminal| terminal.stop());
    }

    fn is_tty(&self) -> bool {
        self.with(|terminal| terminal.is_tty()).unwrap_or(false)
    }

    fn set_progress(&self, active: bool) {
        let _ = self.with(|terminal| terminal.set_progress(active));
    }

    fn flush(&self) {
        let _ = self.with(|terminal| terminal.flush());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Editor, Focusable, Text};
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

    #[test]
    fn changed_rows_overwrite_without_blanking_sidebar_and_clear_shorter_tails_afterwards() {
        for main_screen in [false, true] {
            let output = Arc::new(Mutex::new(String::new()));
            let tui = TuiAltScreen::new(
                Box::new(RecordingTerminal {
                    output: output.clone(),
                }),
                false,
                None,
            );
            let text = Arc::new(Text::new("chat one    │ RUN STATS", 0, 0));
            tui.set_main_screen_mode(main_screen);
            tui.set_layout_root(Some(text.clone()));
            tui.start_readerless();
            output.lock().unwrap().clear();
            text.set_text("chat two    │ RUN STATS");
            tui.do_render(false);
            let frame = output.lock().unwrap().clone();
            assert!(frame.contains("tw"));
            assert!(!frame.contains("RUN STATS"));
            assert!(!frame.contains("\x1b[2K"));
            assert!(!frame.contains("\x1b[2J"));
            output.lock().unwrap().clear();
            text.set_text("short");
            tui.do_render(false);
            let frame = output.lock().unwrap().clone();
            assert!(frame.contains("short"));
            assert!(!frame.contains("\x1b[2K"));
        }
    }

    #[test]
    fn jump_click_resumes_following_and_preserves_the_editor_draft_and_focus() {
        use crate::{StackChild, StackEntry, VStack};
        use crossterm::event::{MouseButton, MouseEvent, MouseEventKind};
        let tui = TuiAltScreen::new(
            Box::new(RecordingTerminal {
                output: Arc::new(Mutex::new(String::new())),
            }),
            false,
            None,
        );
        let text = Arc::new(Text::new(
            (1..=20)
                .map(|n| format!("message {n}"))
                .collect::<Vec<_>>()
                .join("\n"),
            0,
            0,
        ));
        let scroll = Arc::new(ScrollView::primary(text.clone()));
        let editor = Arc::new(Editor::simple());
        editor.set_text("未发送的草稿");
        let root = Arc::new(VStack::from_children(vec![
            StackChild::Entry(StackEntry::new(scroll.clone()).basis(0).grow(1).min_size(1)),
            StackChild::Entry(StackEntry::new(editor.clone())),
        ]));
        tui.set_layout_root(Some(root));
        tui.set_focus(Some(editor.clone()));
        tui.set_scroll_to_end_indicator(Some(Arc::new(|| {
            " ↓ Jump to latest message · End ".to_owned()
        })));
        tui.start_readerless();
        tui.scroll_by(-4);
        assert!(!scroll.is_following_end());
        assert!(tui
            .previous_screen
            .lock()
            .unwrap()
            .iter()
            .any(|line| line.contains("Jump to latest")));
        let click = MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: 20,
            row: (scroll.viewport_height() - 1) as u16,
            modifiers: KeyModifiers::NONE,
        };
        assert!(tui.handle_scroll_to_end_indicator_mouse_event(&click));
        assert!(scroll.is_following_end());
        assert!(!tui
            .previous_screen
            .lock()
            .unwrap()
            .iter()
            .any(|line| line.contains("Jump to latest")));
        assert_eq!(editor.get_text(), "未发送的草稿");
        let focused: Arc<dyn Component> = editor.clone();
        assert!(Arc::ptr_eq(&tui.get_focus().unwrap(), &focused));
        text.set_text(
            (1..=24)
                .map(|n| format!("message {n}"))
                .collect::<Vec<_>>()
                .join("\n"),
        );
        tui.do_render(false);
        assert_eq!(
            scroll.scroll_top(),
            scroll.content_height() - scroll.viewport_height()
        );
        assert!(tui
            .previous_screen
            .lock()
            .unwrap()
            .iter()
            .any(|line| line.contains("message 24")));
        tui.stop(Default::default());
    }

    struct RecordingTerminal {
        output: Arc<Mutex<String>>,
    }

    impl Terminal for RecordingTerminal {
        fn info(&self) -> TerminalInfo {
            TerminalInfo {
                columns: 40,
                rows: 8,
                ..Default::default()
            }
        }

        fn write(&self, data: &str) {
            self.output.lock().unwrap().push_str(data);
        }

        fn hide_cursor(&self) {}
        fn show_cursor(&self) {}
        fn move_cursor(&self, _row: usize, _col: usize) {}
        fn clear_screen(&self) {}
        fn set_title(&self, _title: &str) {}
        fn enable_mouse(&self) {}
        fn disable_mouse(&self) {}
        fn enter_raw_mode(&self) {
            self.write("<raw-on>");
        }
        fn exit_raw_mode(&self) {
            self.write("<raw-off>");
        }
        fn refresh_size(&self) {}
        fn start(
            &self,
            _on_input: Box<dyn Fn(InputEvent) + Send + Sync>,
            _on_resize: Box<dyn Fn() + Send + Sync>,
        ) {
            self.enter_raw_mode();
        }
        fn stop(&self) {
            self.write("<stop>");
        }
        fn is_tty(&self) -> bool {
            true
        }
        fn set_progress(&self, _active: bool) {}
        fn flush(&self) {}
    }

    #[test]
    fn keyboard_setup_and_teardown_stay_on_the_owning_screen_buffer() {
        for readerless in [true, false] {
            let output = Arc::new(Mutex::new(String::new()));
            let tui = TuiAltScreen::new(
                Box::new(RecordingTerminal {
                    output: output.clone(),
                }),
                true,
                None,
            );
            if readerless {
                tui.start_readerless();
            } else {
                tui.start();
            }
            let rendered = output.lock().unwrap().clone();
            assert!(rendered.find("\x1b[?1049h").unwrap() < rendered.find("<raw-on>").unwrap());
            output.lock().unwrap().clear();
            tui.suspend();
            let rendered = output.lock().unwrap().clone();
            assert!(rendered.find("<raw-off>").unwrap() < rendered.find("\x1b[?1049l").unwrap());
            assert!(rendered.contains("\x1b[?7h"));
            output.lock().unwrap().clear();
            tui.resume();
            let rendered = output.lock().unwrap().clone();
            assert!(rendered.find("\x1b[?1049h").unwrap() < rendered.find("<raw-on>").unwrap());
            output.lock().unwrap().clear();
            tui.stop(TuiStopOptions::default());
            let rendered = output.lock().unwrap().clone();
            assert!(rendered.find("<stop>").unwrap() < rendered.find("\x1b[?1049l").unwrap());
        }
    }

    #[test]
    fn regular_start_never_switches_to_the_alternate_screen() {
        let output = Arc::new(Mutex::new(String::new()));
        let tui = TuiAltScreen::new(
            Box::new(RecordingTerminal {
                output: output.clone(),
            }),
            true,
            None,
        );
        tui.set_main_screen_mode(true);
        tui.start();
        tui.suspend();
        tui.resume();
        tui.stop(TuiStopOptions::default());
        let rendered = output.lock().unwrap().clone();
        assert!(rendered.contains("<raw-on>"));
        assert!(!rendered.contains("\x1b[?1049"));
    }

    #[test]
    fn cursor_marker_is_never_written_to_the_terminal() {
        let output = Arc::new(Mutex::new(String::new()));
        let terminal = RecordingTerminal {
            output: output.clone(),
        };
        let tui = TuiAltScreen::new(Box::new(terminal), true, None);
        let editor = Arc::new(Editor::simple());
        editor.set_focused(true);
        editor.insert("hello");
        tui.set_layout_root(Some(editor.clone()));
        tui.start_readerless();

        editor.handle_key(KeyEvent::new(KeyCode::Left, KeyModifiers::NONE));
        tui.request_render(false);

        let rendered = output.lock().unwrap().clone();
        assert!(!rendered.contains(CURSOR_MARKER));
        assert!(rendered.contains("hello"));
        assert_eq!(editor.get_text(), "hello");
    }

    #[test]
    fn layout_does_not_touch_main_screen_and_stop_clears_it() {
        let output = Arc::new(Mutex::new(String::new()));
        let terminal = RecordingTerminal {
            output: output.clone(),
        };
        let tui = TuiAltScreen::new(Box::new(terminal), true, None);

        tui.set_layout_root(Some(Arc::new(Text::new("screen content", 0, 0))));
        assert!(output.lock().unwrap().is_empty());

        tui.start_readerless();
        assert!(output.lock().unwrap().contains("screen content"));

        tui.stop(TuiStopOptions::default());
        let rendered = output.lock().unwrap().clone();
        let content_position = rendered.find("screen content").unwrap();
        let clear_position = rendered
            .rfind("\x1b[?1049l\x1b[2J\x1b[H\x1b[?25h")
            .expect("stop should leave alt screen and clear the restored main screen");
        assert!(clear_position > content_position);
    }

    #[test]
    fn cached_redraw_is_suppressed_while_rendering_is_suspended() {
        let output = Arc::new(Mutex::new(String::new()));
        let terminal = RecordingTerminal {
            output: output.clone(),
        };
        let tui = TuiAltScreen::new(Box::new(terminal), true, None);
        tui.set_layout_root(Some(Arc::new(Text::new("outer frame", 0, 0))));
        tui.start_readerless();

        tui.set_render_suspended(true);
        let before = output.lock().unwrap().len();
        tui.request_render_reusing_scroll_content();
        assert_eq!(output.lock().unwrap().len(), before);

        tui.set_render_suspended(false);
        tui.set_layout_root(Some(Arc::new(Text::new("new frame", 0, 0))));
        assert!(output.lock().unwrap().len() > before);
    }

    struct StaticLines(Arc<Mutex<Vec<String>>>);

    impl Component for StaticLines {
        fn render(&self, _width: usize) -> Vec<String> {
            self.0.lock().unwrap().clone()
        }
        fn invalidate(&self) {}
        fn as_any(&self) -> &dyn std::any::Any {
            self
        }
    }

    #[test]
    fn regular_mode_rebuilds_when_scrollback_rows_change() {
        // e59a6bb 契约：变更起点落在已固化进 scrollback 的行
        // （first_changed < viewport_top）时放弃增量渲染——已固化的历史行
        // 无法被光标移动或 CSI T 寻址（SD 只在活动屏内插空行），尾部增量
        // 重写只会把历史内容重复追加一遍（2026-10-18 runloom 长会话实证：
        // 每条长回复在终端上重复两遍）。必须走全文档重建。
        //
        // 重建的落屏方式从「前置 \x1b[2J 清屏」改为覆盖式重绘（fork 的
        // sync-output-fallback）：home 后逐行覆盖 + 行尾 EL + 尾部 ED0，
        // ED3 清固化 scrollback。契约不变——重建分支仍整支文档重写；
        // 无同步输出的终端（tty7/Apple_Terminal/Alacritty）上不再出现
        // 整屏空白帧。
        let output = Arc::new(Mutex::new(String::new()));
        let terminal = RecordingTerminal {
            output: output.clone(),
        };
        let tui = TuiAltScreen::new(Box::new(terminal), true, None);
        let content = Arc::new(Text::new(
            &(0..60)
                .map(|i| format!("line {i}"))
                .collect::<Vec<_>>()
                .join("\n"),
            0,
            0,
        ));
        tui.set_main_screen_mode(true);
        tui.set_layout_root(Some(content.clone()));
        tui.start_readerless();
        tui.do_render(false); // 首帧：60 行文档，viewport_top = 60 - 8
        output.lock().unwrap().clear();

        // 模拟晚到的流式增量重排了视口上方（scrollback 固化区）的行：
        // line 10 内容变化 → first_changed = 10 < viewport_top(52)。
        content.set_text(
            &(0..60)
                .map(|i| {
                    if i == 10 {
                        "line 10 (re-wrapped)".to_string()
                    } else {
                        format!("line {i}")
                    }
                })
                .collect::<Vec<_>>()
                .join("\n"),
        );
        tui.do_render(false);

        let frame = output.lock().unwrap().clone();
        // 全文档重建（而非增量尾部重写）：ED3 只在重建分支发出，
        // 且从 home 起覆盖重写全部行。
        assert!(
            !frame.contains("\x1b[2J"),
            "rebuild must paint over the old rows, not blank first: {frame:?}"
        );
        assert!(
            frame.contains("\x1b[3J"),
            "scrollback hit must drop the fixed scrollback: {frame:?}"
        );
        assert!(
            frame.contains("\x1b[H"),
            "rebuild homes the cursor: {frame:?}"
        );
        assert!(frame.contains("line 0"));
        assert!(frame.contains("line 10 (re-wrapped)"));
        // 重建后视口跟踪重置到文档尾部。
        assert_eq!(*tui.main_viewport_top.lock().unwrap(), 60 - 8);
        tui.stop(Default::default());
    }

    #[test]
    fn regular_mode_skips_blank_row_style_diffs() {
        let output = Arc::new(Mutex::new(String::new()));
        let terminal = RecordingTerminal {
            output: output.clone(),
        };
        // apply_line_resets 的中性尾巴（SGR 0 + OSC8 close）：重置类序列在
        // 空白行上无可见效果，剪枝安全。
        let lines = Arc::new(Mutex::new(vec![
            "hello".to_string(),
            format!("{}\x1b[0m\x1b]8;;\x07", " ".repeat(40)),
        ]));
        let content = Arc::new(StaticLines(lines.clone()));
        let tui = TuiAltScreen::new(Box::new(terminal), true, None);
        tui.set_main_screen_mode(true);
        tui.set_layout_root(Some(content));
        tui.start_readerless();
        tui.do_render(false);
        output.lock().unwrap().clear();

        // 第二行仍是 40 列空白，只是尾巴从“reset+OSC8”变成纯空：两边都是
        // 中性空白 → 剪枝，不产生“跳列 + 无文本”帧字节。
        *lines.lock().unwrap() = vec!["hello".to_string(), " ".repeat(40)];
        tui.do_render(false);

        let frame = output.lock().unwrap().clone();
        // 空白行中性样式 diff 被剪枝：不产生“跳列 + 无文本”帧字节。
        assert!(!frame.contains("\x1b[40C"), "{frame:?}");
        tui.stop(Default::default());
    }

    #[test]
    fn regular_mode_repaints_painted_blank_rows() {
        // 带背景色的空白行是“实际绘制属性”（P2-3）：颜色变化必须重绘，
        // 否则工具面板/滚动条的空白背景行会残留旧色。
        let output = Arc::new(Mutex::new(String::new()));
        let terminal = RecordingTerminal {
            output: output.clone(),
        };
        let lines = Arc::new(Mutex::new(vec![
            "hello".to_string(),
            format!("\x1b[41m{}\x1b[0m", " ".repeat(40)), // 红底空白行
        ]));
        let content = Arc::new(StaticLines(lines.clone()));
        let tui = TuiAltScreen::new(Box::new(terminal), true, None);
        tui.set_main_screen_mode(true);
        tui.set_layout_root(Some(content));
        tui.start_readerless();
        tui.do_render(false);
        output.lock().unwrap().clear();

        // 红底 → 绿底：可见背景变化，不能被空白行剪枝吞掉。
        *lines.lock().unwrap() = vec![
            "hello".to_string(),
            format!("\x1b[42m{}\x1b[0m", " ".repeat(40)), // 绿底空白行
        ];
        tui.do_render(false);

        let frame = output.lock().unwrap().clone();
        assert!(frame.contains("\x1b[42m"), "{frame:?}");
        tui.stop(Default::default());
    }

    #[test]
    fn regular_mode_healed_frame_leaves_cursor_at_document_tail() {
        // P1-2 回归：自愈重发后光标停在文档末行，下一帧必须是纯 append
        // （无 A/B 光标移动），否则实际光标与 hardware_row 记录失配。
        let output = Arc::new(Mutex::new(String::new()));
        let terminal = RecordingTerminal {
            output: output.clone(),
        };
        let tui = TuiAltScreen::new(Box::new(terminal), true, None);
        let content = Arc::new(Text::new(
            &(0..60)
                .map(|i| format!("line {i}"))
                .collect::<Vec<_>>()
                .join("\n"),
            0,
            0,
        ));
        tui.set_main_screen_mode(true);
        tui.set_layout_root(Some(content.clone()));
        tui.start_readerless();
        tui.do_render(false);

        // 漂移 + 61 行 → 自愈全量重发。
        *tui.main_viewport_top.lock().unwrap() = 0;
        *tui.main_hardware_row.lock().unwrap() = 0;
        content.set_text(
            &(0..60)
                .map(|i| format!("line {i}"))
                .chain(std::iter::once("new tail".to_string()))
                .collect::<Vec<_>>()
                .join("\n"),
        );
        tui.do_render(false);
        output.lock().unwrap().clear();

        // 自愈后再追加一行：应输出纯 append，不带任何光标重定位。
        content.set_text(
            &(0..60)
                .map(|i| format!("line {i}"))
                .chain(["new tail".to_string(), "another line".to_string()])
                .collect::<Vec<_>>()
                .join("\n"),
        );
        tui.do_render(false);

        let frame = output.lock().unwrap().clone();
        assert!(frame.contains("another line"), "{frame:?}");
        assert!(
            !frame.contains("\x1b[1A"),
            "cursor must not reposition: {frame:?}"
        );
        assert!(
            !frame.contains("\x1b[1B"),
            "cursor must not reposition: {frame:?}"
        );
        assert!(!frame.contains("\x1b[2J"), "{frame:?}");
        tui.stop(Default::default());
    }

    #[test]
    fn regular_single_line_update_does_not_rewrite_unchanged_tail() {
        let output = Arc::new(Mutex::new(String::new()));
        let terminal = RecordingTerminal {
            output: output.clone(),
        };
        let tui = TuiAltScreen::new(Box::new(terminal), true, None);
        let content = Arc::new(Text::new("header\nstatus\neditor\nfooter", 0, 0));
        tui.set_main_screen_mode(true);
        tui.set_layout_root(Some(content.clone()));
        tui.start_readerless();
        output.lock().unwrap().clear();

        content.set_text("header\nstatus2\neditor\nfooter");
        tui.request_render(false);

        let rendered = output.lock().unwrap().clone();
        assert!(rendered.contains("\x1b[6C2"));
        assert!(!rendered.contains("editor"));
        assert!(!rendered.contains("footer"));
    }

    #[test]
    fn regular_unchanged_frame_writes_nothing() {
        let output = Arc::new(Mutex::new(String::new()));
        let terminal = RecordingTerminal {
            output: output.clone(),
        };
        let tui = TuiAltScreen::new(Box::new(terminal), true, None);
        tui.set_main_screen_mode(true);
        tui.set_layout_root(Some(Arc::new(Text::new(
            "header\nstatus\neditor\nfooter",
            0,
            0,
        ))));
        tui.start_readerless();
        output.lock().unwrap().clear();

        tui.request_render(false);

        assert!(output.lock().unwrap().is_empty());
    }

    #[test]
    fn merge_pending_render_never_downgrades_a_full_render() {
        // Nothing parked: the request's own variant wins.
        assert_eq!(merge_pending_render(None, false), Some(false));
        assert_eq!(merge_pending_render(None, true), Some(true));
        // A parked cached-content frame is upgraded by a full render...
        assert_eq!(merge_pending_render(Some(true), false), Some(false));
        // ...and stays cached when another cached request arrives.
        assert_eq!(merge_pending_render(Some(true), true), Some(true));
        // A parked full render is never downgraded by a cached request.
        assert_eq!(merge_pending_render(Some(false), true), Some(false));
        assert_eq!(merge_pending_render(Some(false), false), Some(false));
    }

    #[test]
    fn rapid_requests_coalesce_into_a_single_frame() {
        let output = Arc::new(Mutex::new(String::new()));
        let terminal = RecordingTerminal {
            output: output.clone(),
        };
        let tui = TuiAltScreen::new(Box::new(terminal), true, None);
        let content = Arc::new(Text::new("first", 0, 0));
        tui.set_layout_root(Some(content.clone()));
        tui.enable_render_coalescing();
        tui.start_readerless();
        output.lock().unwrap().clear();

        // A streaming burst: many requests, no painting until the host flush.
        content.set_text("second");
        for _ in 0..50 {
            tui.request_render(false);
        }
        assert!(
            output.lock().unwrap().is_empty(),
            "parked requests must not repaint the transcript"
        );

        // The host flush paints exactly one frame and clears what was owed.
        assert!(tui.flush_pending_render());
        let rendered = output.lock().unwrap().clone();
        assert!(rendered.contains("second"));
        assert!(!tui.flush_pending_render(), "nothing owed after the flush");
    }

    #[test]
    fn immediate_render_discards_a_parked_frame() {
        let output = Arc::new(Mutex::new(String::new()));
        let terminal = RecordingTerminal {
            output: output.clone(),
        };
        let tui = TuiAltScreen::new(Box::new(terminal), true, None);
        let content = Arc::new(Text::new("before", 0, 0));
        tui.set_layout_root(Some(content.clone()));
        tui.enable_render_coalescing();
        tui.start_readerless();
        output.lock().unwrap().clear();

        content.set_text("after");
        tui.request_render(false);
        // An immediate frame (keyboard input, selector open) wins over the
        // parked one instead of rendering the same content twice.
        tui.render_now(true);

        assert!(output.lock().unwrap().contains("after"));
        assert!(!tui.flush_pending_render());
    }

    /// End-to-end throttle check on the real scheduler thread. Each frame emits
    /// exactly one synchronized-output opener (`\x1b[?2026h`), so counting those
    /// counts painted frames. Before the throttle, every one of the 200
    /// "streamed deltas" repainted the whole transcript.
    #[test]
    fn streaming_burst_paints_at_most_one_frame_per_interval() {
        let output = Arc::new(Mutex::new(String::new()));
        let terminal = RecordingTerminal {
            output: output.clone(),
        };
        let tui = Arc::new(TuiAltScreen::new(Box::new(terminal), true, None));
        let content = Arc::new(Text::new("0", 0, 0));
        tui.set_layout_root(Some(content.clone()));
        tui.start_render_scheduler();
        tui.start_readerless();
        output.lock().unwrap().clear();

        for i in 0..200 {
            content.set_text(&format!("delta {i}"));
            tui.request_render(false);
            std::thread::sleep(Duration::from_millis(1));
        }
        std::thread::sleep(Duration::from_millis(50));

        let frames = output.lock().unwrap().matches("\x1b[?2026h").count();
        assert!(frames >= 1, "the scheduler must still paint frames");
        assert!(
            frames <= 40,
            "200 requests in ~250 ms must coalesce into ~16 ms frames, painted {frames}"
        );
        tui.stop(TuiStopOptions::default());
    }

    #[test]
    fn render_interval_is_clamped_and_runtime_adjustable() {
        let output = Arc::new(Mutex::new(String::new()));
        let tui = TuiAltScreen::new(
            Box::new(RecordingTerminal {
                output: output.clone(),
            }),
            true,
            None,
        );
        assert_eq!(tui.render_interval_ms(), MIN_RENDER_INTERVAL_MS);
        // A bad value cannot disable the throttle: zero clamps to the floor.
        tui.set_render_interval_ms(0);
        assert_eq!(tui.render_interval_ms(), MIN_RENDER_INTERVAL_MS);
        tui.set_render_interval_ms(FALLBACK_RENDER_INTERVAL_MS);
        assert_eq!(tui.render_interval_ms(), FALLBACK_RENDER_INTERVAL_MS);
        tui.stop(TuiStopOptions::default());
    }

    /// Resize-triggered full rebuilds must repaint the document row by row
    /// instead of blanking the screen first: a leading `\x1b[2J` paints a fully
    /// blank frame that terminals without CSI 2026 support expose as flicker.
    #[test]
    fn main_screen_rebuild_repaints_in_place_without_blank_frame() {
        let output = Arc::new(Mutex::new(String::new()));
        let size = Arc::new(Mutex::new((40usize, 8usize)));
        let terminal = ResizableTerminal {
            output: output.clone(),
            size: size.clone(),
        };
        let tui = TuiAltScreen::new(Box::new(terminal), true, None);
        tui.set_main_screen_mode(true);
        let content = Arc::new(Text::new("stable", 0, 0));
        tui.set_layout_root(Some(content.clone()));
        tui.start_readerless();
        output.lock().unwrap().clear();

        // A size change forces the full-rebuild branch (ED3 included).
        *size.lock().unwrap() = (40, 10);
        content.set_text("resized");
        tui.request_render(false);

        let rendered = output.lock().unwrap().clone();
        assert!(
            !rendered.contains("\x1b[2J"),
            "rebuild must not blank the screen first"
        );
        assert!(rendered.contains("\x1b[?2026h"), "frame stays atomic-able");
        assert!(
            rendered.contains("\x1b[3J"),
            "rebuild still clears scrollback"
        );
        assert!(rendered.contains("\x1b[H"), "repaint homes the cursor");
        assert!(rendered.contains("resized"), "new content is painted");
        assert!(rendered.contains("\x1b[K"), "row tails are erased");
        assert!(rendered.contains("\x1b[J"), "viewport remainder is wiped");
        tui.stop(TuiStopOptions::default());
    }

    /// First frame on the main screen appends from the shell cursor, exactly
    /// as the baseline did (codex P2): the user's visible shell output stays
    /// on screen and the saved scrollback is untouched.
    #[test]
    fn main_screen_first_frame_appends_from_the_shell_cursor() {
        let output = Arc::new(Mutex::new(String::new()));
        let tui = TuiAltScreen::new(
            Box::new(RecordingTerminal {
                output: output.clone(),
            }),
            true,
            None,
        );
        tui.set_main_screen_mode(true);
        let content = Arc::new(Text::new("initial", 0, 0));
        tui.set_layout_root(Some(content));
        tui.start_readerless();

        let rendered = output.lock().unwrap().clone();
        assert!(rendered.contains("initial"));
        assert!(
            !rendered.contains("\x1b[2J"),
            "first frame must not blank the screen"
        );
        assert!(
            !rendered.contains("\x1b[3J"),
            "first frame keeps the user's shell scrollback"
        );
        assert!(
            !rendered.contains("\x1b[H"),
            "first frame must not home over the shell's visible output"
        );
        assert!(
            !rendered.contains("\x1b[K"),
            "first frame appends without erasing (baseline behaviour)"
        );
        tui.stop(TuiStopOptions::default());
    }

    /// Alt-screen full redraws (viewport height change) must likewise paint
    /// every row over the old frame instead of clearing first.
    #[test]
    fn alt_screen_full_redraw_skips_leading_clear() {
        let output = Arc::new(Mutex::new(String::new()));
        let size = Arc::new(Mutex::new((40usize, 8usize)));
        let terminal = ResizableTerminal {
            output: output.clone(),
            size: size.clone(),
        };
        let tui = TuiAltScreen::new(Box::new(terminal), true, None);
        let content = Arc::new(Text::new("before", 0, 0));
        tui.set_layout_root(Some(content.clone()));
        tui.start_readerless();
        output.lock().unwrap().clear();

        // Viewport height change flips `full_redraw` on in `do_render`.
        *size.lock().unwrap() = (40, 10);
        content.set_text("after");
        tui.request_render(false);

        let rendered = output.lock().unwrap().clone();
        assert!(
            !rendered.contains("\x1b[2J"),
            "full redraw must not blank the viewport first"
        );
        assert!(rendered.contains("after"));
        assert!(
            rendered.contains("\x1b[K"),
            "rows erase their leftover tails"
        );
        assert!(rendered.contains("\x1b[?2026h"));
        tui.stop(TuiStopOptions::default());
    }

    struct ResizableTerminal {
        output: Arc<Mutex<String>>,
        size: Arc<Mutex<(usize, usize)>>,
    }

    impl Terminal for ResizableTerminal {
        fn info(&self) -> TerminalInfo {
            let (columns, rows) = *self.size.lock().unwrap();
            TerminalInfo {
                columns,
                rows,
                ..Default::default()
            }
        }

        fn write(&self, data: &str) {
            self.output.lock().unwrap().push_str(data);
        }

        fn hide_cursor(&self) {}
        fn show_cursor(&self) {}
        fn move_cursor(&self, _row: usize, _col: usize) {}
        fn clear_screen(&self) {}
        fn set_title(&self, _title: &str) {}
        fn enable_mouse(&self) {}
        fn disable_mouse(&self) {}
        fn enter_raw_mode(&self) {
            self.write("<raw-on>");
        }
        fn exit_raw_mode(&self) {
            self.write("<raw-off>");
        }
        fn refresh_size(&self) {}
        fn start(
            &self,
            _on_input: Box<dyn Fn(InputEvent) + Send + Sync>,
            _on_resize: Box<dyn Fn() + Send + Sync>,
        ) {
            self.enter_raw_mode();
        }
        fn stop(&self) {
            self.write("<stop>");
        }
        fn is_tty(&self) -> bool {
            true
        }
        fn set_progress(&self, _active: bool) {}
        fn flush(&self) {}
    }
}
