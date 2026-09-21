// Arrow-key TUI for a bare `maskrun`: platforms on the left, that platform's
// secrets and the highlighted one's detail on the right. Everything that can
// be tested without a real terminal (grouping, key handling, the clipboard
// timeout, frame content) is a pure function or takes a small trait object in
// place of the real terminal/clipboard; only `run_overview`/`run_interactive`
// touch crossterm and arboard directly.

use std::io::Write;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use crossterm::cursor::{Hide, MoveTo, Show};
use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use crossterm::style::{Attribute, Print, SetAttribute};
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, size, Clear, ClearType, EnterAlternateScreen,
    LeaveAlternateScreen,
};
use crossterm::{execute, queue};
use zeroize::Zeroizing;

use crate::error::{Error, Result};
use crate::keyring::{Backend, FieldUpdate, MetaUpdate, SecretMeta};

const MIN_COLS: u16 = 60;
const MIN_ROWS: u16 = 15;
const CLIP_TIMEOUT: Duration = Duration::from_secs(45);
const MASKED_PLACEHOLDER: &str = "••••••••••••••••";
const LEFT_PANE_WIDTH: usize = 22;
const POLL_INTERVAL: Duration = Duration::from_millis(250);

// Entry point for the bare-`maskrun` TUI. Every early-return here goes
// through `crate::cmd_overview_summary()`, the exact same text-only path a
// non-interactive session already uses — no separate "why we bailed" UI to
// keep in sync with it.
pub fn run_overview() -> Result<i32> {
    // Defense in depth: `cmd_overview()` already gates this call on
    // `interactive_stdio()`, which folds in `!agent::in_agent()`. This
    // function is the one that reads secret values and writes to the
    // clipboard, so it does not trust that gate alone.
    if crate::agent::in_agent() {
        return crate::cmd_overview_summary();
    }

    let backend = match crate::keyring::pick_backend() {
        Ok(b) => b,
        Err(_) => return crate::cmd_overview_summary(),
    };

    let (cols, rows) = match size() {
        Ok(s) => s,
        Err(_) => return crate::cmd_overview_summary(),
    };
    if cols < MIN_COLS || rows < MIN_ROWS {
        println!(
            "terminal too small for the interactive view ({cols}x{rows}, need at least \
             {MIN_COLS}x{MIN_ROWS}) — showing the summary instead:\n"
        );
        return crate::cmd_overview_summary();
    }

    let mut entries = match backend.list_meta() {
        Ok(e) => e,
        Err(_) => return crate::cmd_overview_summary(),
    };
    entries.sort_by(|a, b| a.0.cmp(&b.0));

    run_interactive(backend.as_ref(), entries, &mut ArboardClipboard::new())
}

// ---------------------------------------------------------------------------
// Clipboard abstraction: real runs talk to arboard, tests talk to an
// in-memory fake, so the state machine's copy/clear/timeout logic never needs
// a display server to exercise.

trait ClipboardPort {
    fn get_text(&mut self) -> Option<String>;
    /// `true` on success. Failure (no display server reachable, Wayland
    /// compositor without the data-control protocol, ...) is reported to the
    /// user as a status line, never as a crash.
    fn set_text(&mut self, text: &str) -> bool;
    fn clear(&mut self) -> bool;
}

struct ArboardClipboard(Option<arboard::Clipboard>);

impl ArboardClipboard {
    fn new() -> Self {
        Self(arboard::Clipboard::new().ok())
    }
}

// KDE's Klipper (and, per the same convention, some GNOME/other Wayland
// history tools) skips an item tagged with this mime type instead of writing
// it to its persistent history. arboard exposes it as `exclude_from_history`
// on all three platforms (`SetExtLinux`/`SetExtApple`/`SetExtWindows`) via
// its own extension traits — investigated by reading
// arboard-3.6.1/src/platform/{linux,osx,windows}.rs directly, since the docs
// don't surface it under "custom mime type" search terms. It is a hint, not
// an enforcement: a history tool that doesn't recognise the tag (GNOME's,
// most non-KDE Wayland setups) still records the value, which is exactly why
// the auto-clear timeout below exists regardless.
#[cfg(target_os = "linux")]
fn set_text_excluding_history(cb: &mut arboard::Clipboard, text: String) -> bool {
    use arboard::SetExtLinux;
    cb.set().exclude_from_history().text(text).is_ok()
}
#[cfg(target_os = "macos")]
fn set_text_excluding_history(cb: &mut arboard::Clipboard, text: String) -> bool {
    use arboard::SetExtApple;
    cb.set().exclude_from_history().text(text).is_ok()
}
#[cfg(target_os = "windows")]
fn set_text_excluding_history(cb: &mut arboard::Clipboard, text: String) -> bool {
    use arboard::SetExtWindows;
    cb.set().exclude_from_history().text(text).is_ok()
}

impl ClipboardPort for ArboardClipboard {
    fn get_text(&mut self) -> Option<String> {
        self.0.as_mut()?.get_text().ok()
    }
    fn set_text(&mut self, text: &str) -> bool {
        let Some(cb) = self.0.as_mut() else {
            return false;
        };
        set_text_excluding_history(cb, text.to_string())
    }
    fn clear(&mut self) -> bool {
        let Some(cb) = self.0.as_mut() else {
            return false;
        };
        cb.clear().is_ok()
    }
}

// Mirrors what the panic hook needs to know, updated once per main-loop
// iteration from `App.clip`. `profile.release.panic = "abort"` (Cargo.toml)
// means a release-build panic never unwinds, so `TerminalGuard::drop` never
// runs there — this static plus the hook below is the only clipboard cleanup
// that exists on that path, not a backup for it.
static CLIP_RESTORE: Mutex<Option<Option<String>>> = Mutex::new(None);

fn clear_clipboard_best_effort() {
    let Ok(mut guard) = CLIP_RESTORE.lock() else {
        return;
    };
    let Some(restore) = guard.take() else {
        return;
    };
    drop(guard);
    if let Ok(mut cb) = arboard::Clipboard::new() {
        let _ = match restore {
            Some(prev) => cb.set_text(prev),
            None => cb.clear(),
        };
    }
}

// ---------------------------------------------------------------------------
// Pure state: grouping, navigation, and the effects of every key. No I/O.

struct Group {
    label: Option<String>,
    rows: Vec<usize>,
}

// Mirrors main.rs's `print_grouped`: platforms alphabetical (entries arrive
// pre-sorted by name), "(no platform)" last.
fn group_entries(entries: &[(String, SecretMeta)]) -> Vec<Group> {
    use std::collections::BTreeMap;
    let mut groups: BTreeMap<&str, Vec<usize>> = BTreeMap::new();
    let mut unlabelled = Vec::new();
    for (i, (_, meta)) in entries.iter().enumerate() {
        match meta.platform.as_deref() {
            Some(p) => groups.entry(p).or_default().push(i),
            None => unlabelled.push(i),
        }
    }
    let mut out: Vec<Group> = groups
        .into_iter()
        .map(|(label, rows)| Group {
            label: Some(label.to_string()),
            rows,
        })
        .collect();
    if !unlabelled.is_empty() {
        out.push(Group {
            label: None,
            rows: unlabelled,
        });
    }
    out
}

#[derive(PartialEq, Eq)]
enum Focus {
    Platforms,
    Secrets,
}

struct ClipState {
    started: Instant,
    previous: Option<String>,
}

impl ClipState {
    fn remaining(&self) -> Duration {
        CLIP_TIMEOUT.saturating_sub(self.started.elapsed())
    }
    fn expired(&self) -> bool {
        self.started.elapsed() >= CLIP_TIMEOUT
    }
}

struct App {
    entries: Vec<(String, SecretMeta)>,
    groups: Vec<Group>,
    focus: Focus,
    sel_group: usize,
    sel_row: usize,
    revealed: Option<Zeroizing<String>>,
    status: Option<String>,
    confirm_delete: bool,
    clip: Option<ClipState>,
    quit: bool,
}

impl App {
    fn new(entries: Vec<(String, SecretMeta)>) -> Self {
        let groups = group_entries(&entries);
        App {
            entries,
            groups,
            focus: Focus::Platforms,
            sel_group: 0,
            sel_row: 0,
            revealed: None,
            status: None,
            confirm_delete: false,
            clip: None,
            quit: false,
        }
    }

    fn current_index(&self) -> Option<usize> {
        self.groups
            .get(self.sel_group)
            .and_then(|g| g.rows.get(self.sel_row))
            .copied()
    }

    fn current(&self) -> Option<&(String, SecretMeta)> {
        self.current_index().map(|i| &self.entries[i])
    }

    fn clear_transients(&mut self) {
        self.revealed = None;
        self.status = None;
        self.confirm_delete = false;
    }

    fn move_platform(&mut self, delta: i32) {
        if self.groups.is_empty() {
            return;
        }
        let last = self.groups.len() - 1;
        self.sel_group = clamp_move(self.sel_group, delta, last);
        self.sel_row = 0;
        self.clear_transients();
    }

    fn move_secret(&mut self, delta: i32) {
        let Some(group) = self.groups.get(self.sel_group) else {
            return;
        };
        if group.rows.is_empty() {
            return;
        }
        self.sel_row = clamp_move(self.sel_row, delta, group.rows.len() - 1);
        self.clear_transients();
    }

    fn enter(&mut self) {
        if self
            .groups
            .get(self.sel_group)
            .is_some_and(|g| !g.rows.is_empty())
        {
            self.focus = Focus::Secrets;
        }
    }

    fn back(&mut self) {
        self.focus = Focus::Platforms;
        self.confirm_delete = false;
    }

    // Re-reads the keyring after a mutation (edit/delete) and keeps the
    // cursor on the same secret when it still exists, instead of resetting
    // to the top of the list every time.
    fn refresh(&mut self, backend: &dyn Backend) -> Result<()> {
        let previous = self.current().map(|(n, _)| n.clone());
        let mut entries = backend.list_meta()?;
        entries.sort_by(|a, b| a.0.cmp(&b.0));
        self.groups = group_entries(&entries);
        self.entries = entries;
        self.revealed = None;

        if let Some(name) = previous {
            for (gi, group) in self.groups.iter().enumerate() {
                if let Some(ri) = group.rows.iter().position(|&i| self.entries[i].0 == name) {
                    self.sel_group = gi;
                    self.sel_row = ri;
                    return Ok(());
                }
            }
        }
        self.sel_group = self.sel_group.min(self.groups.len().saturating_sub(1));
        self.sel_row = 0;
        self.focus = Focus::Platforms;
        Ok(())
    }

    fn toggle_reveal(&mut self, backend: &dyn Backend) {
        if self.revealed.is_some() {
            self.revealed = None;
            return;
        }
        let Some((name, _)) = self.current().cloned() else {
            self.status = Some("nothing selected".into());
            return;
        };
        match backend.get(&name) {
            Ok(Some(value)) => self.revealed = Some(Zeroizing::new(value)),
            Ok(None) => self.status = Some(format!("no such secret: {name}")),
            Err(e) => self.status = Some(format!("error: {e}")),
        }
    }

    fn start_delete(&mut self) {
        if self.current().is_some() {
            self.confirm_delete = true;
        } else {
            self.status = Some("nothing selected".into());
        }
    }

    fn confirm_delete(&mut self, confirmed: bool, backend: &dyn Backend) -> Result<()> {
        self.confirm_delete = false;
        let Some((name, _)) = self.current().cloned() else {
            return Ok(());
        };
        if !confirmed {
            self.status = Some("cancelled".into());
            return Ok(());
        }
        backend.delete(&name)?;
        self.status = Some(format!("deleted: {name}"));
        self.refresh(backend)
    }

    fn copy(&mut self, backend: &dyn Backend, clip: &mut dyn ClipboardPort) {
        let Some((name, _)) = self.current().cloned() else {
            self.status = Some("nothing to copy".into());
            return;
        };
        let value = match backend.get(&name) {
            Ok(Some(v)) => Zeroizing::new(v),
            Ok(None) => {
                self.status = Some(format!("no such secret: {name}"));
                return;
            }
            Err(e) => {
                self.status = Some(format!("error: {e}"));
                return;
            }
        };
        let previous = clip.get_text();
        if clip.set_text(&value) {
            self.clip = Some(ClipState {
                started: Instant::now(),
                previous,
            });
            self.status = Some(format!("copied {name} to the clipboard"));
        } else {
            self.status = Some("could not reach the clipboard".into());
        }
    }

    fn clear_clipboard(&mut self, clip: &mut dyn ClipboardPort, reason: &str) {
        let Some(state) = self.clip.take() else {
            return;
        };
        let ok = match state.previous {
            Some(prev) => clip.set_text(&prev),
            None => clip.clear(),
        };
        self.status = Some(if ok {
            reason.to_string()
        } else {
            "could not clear the clipboard".to_string()
        });
    }

    fn tick_clipboard(&mut self, clip: &mut dyn ClipboardPort) {
        if self.clip.as_ref().is_some_and(ClipState::expired) {
            self.clear_clipboard(clip, "clipboard auto-cleared");
        }
    }
}

fn clamp_move(current: usize, delta: i32, max: usize) -> usize {
    let next = current as i64 + delta as i64;
    next.clamp(0, max as i64) as usize
}

// One key press -> one state change. Delete confirmation is handled first
// and swallows every other binding so a stray keystroke during "delete
// this? y/N" can never fall through to, say, 'e'.
fn handle_key(
    app: &mut App,
    key: KeyCode,
    backend: &dyn Backend,
    clip: &mut dyn ClipboardPort,
) -> Result<()> {
    if app.confirm_delete {
        let confirmed = matches!(key, KeyCode::Char('y') | KeyCode::Char('Y'));
        return app.confirm_delete(confirmed, backend);
    }

    match key {
        KeyCode::Char('q') => app.quit = true,
        KeyCode::Up => match app.focus {
            Focus::Platforms => app.move_platform(-1),
            Focus::Secrets => app.move_secret(-1),
        },
        KeyCode::Down => match app.focus {
            Focus::Platforms => app.move_platform(1),
            Focus::Secrets => app.move_secret(1),
        },
        KeyCode::Right | KeyCode::Enter => app.enter(),
        KeyCode::Left | KeyCode::Esc => app.back(),
        KeyCode::Char('v') => app.toggle_reveal(backend),
        KeyCode::Char('d') => app.start_delete(),
        KeyCode::Char('y') => app.copy(backend, clip),
        KeyCode::Char('c') => app.clear_clipboard(clip, "clipboard cleared"),
        _ => {}
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Rendering: a pure `Vec<String>` frame, so content can be asserted on
// without a terminal, plus a thin crossterm writer for the real run.

fn mask_or_value(app: &App) -> String {
    match &app.revealed {
        Some(v) => v.to_string(),
        None => MASKED_PLACEHOLDER.to_string(),
    }
}

fn render(app: &App, cols: u16, rows: u16) -> Vec<String> {
    let cols = cols as usize;
    let rows = rows as usize;
    let mut lines = Vec::new();

    let header_right = format!(
        "{} platform · {} secret",
        app.groups.len(),
        app.entries.len()
    );
    lines.push(pad_between(" maskrun", &header_right, cols));
    lines.push(String::new());

    if app.entries.is_empty() {
        lines.push(" no secrets yet — run: maskrun put <name>".to_string());
    } else {
        let detail_height = 4; // separator + platform + note + value
        let body_rows = rows.saturating_sub(lines.len() + 3 /* blank + hints + status */);
        let list_height = body_rows.saturating_sub(detail_height).max(1);

        let mut left = Vec::new();
        for (i, g) in app.groups.iter().enumerate() {
            let marker = if i == app.sel_group { "▸" } else { " " };
            let label = g.label.as_deref().unwrap_or("(no platform)");
            left.push(format!(
                " {marker} {:<width$}{}",
                label,
                g.rows.len(),
                width = LEFT_PANE_WIDTH - 6
            ));
        }

        let mut right = Vec::new();
        if let Some(group) = app.groups.get(app.sel_group) {
            for (ri, &idx) in group.rows.iter().enumerate() {
                let marker = if app.focus == Focus::Secrets && ri == app.sel_row {
                    "▸"
                } else {
                    " "
                };
                right.push(format!(" {marker} {}", app.entries[idx].0));
            }
        }

        for i in 0..list_height {
            let l = left.get(i).cloned().unwrap_or_default();
            let r = right.get(i).cloned().unwrap_or_default();
            lines.push(format!("{:<width$}{}", l, r, width = LEFT_PANE_WIDTH));
        }

        lines.push("─".repeat(cols.min(80)));
        match app.current() {
            Some((_, meta)) => {
                lines.push(format!(
                    " platform  {}",
                    meta.platform.as_deref().unwrap_or("(none)")
                ));
                lines.push(format!(
                    " note      {}",
                    meta.note.as_deref().unwrap_or("(none)")
                ));
                lines.push(format!(" value     {}", mask_or_value(app)));
            }
            None => {
                lines.push(" (nothing selected)".to_string());
            }
        }
    }

    lines.push(String::new());
    let mut hint =
        " ↑↓ move   → enter   ← back   e edit   d delete   v reveal   y copy".to_string();
    if app.clip.is_some() {
        hint.push_str("   c clear");
    }
    hint.push_str("   q quit");
    lines.push(hint);

    let status_line = if app.confirm_delete {
        let name = app.current().map(|(n, _)| n.as_str()).unwrap_or("?");
        format!(" delete '{name}'? [y/N]")
    } else if let Some(clip) = &app.clip {
        let secs = clip.remaining().as_secs();
        match &app.status {
            Some(s) => format!(" {s} — clipboard clears in {secs}s"),
            None => format!(" clipboard clears in {secs}s"),
        }
    } else {
        match &app.status {
            Some(s) => format!(" {s}"),
            None => String::new(),
        }
    };
    lines.push(status_line);

    lines
        .into_iter()
        .map(|l| l.chars().take(cols).collect())
        .collect()
}

fn pad_between(left: &str, right: &str, cols: usize) -> String {
    let gap = cols.saturating_sub(left.chars().count() + right.chars().count() + 1);
    format!("{left}{}{right}", " ".repeat(gap.max(1)))
}

// A full clear rather than a per-line one: the cooked-mode detour for `e`
// (see `edit_secret`) prints a variable number of lines outside this
// function's control, and a per-line clear would leave its leftovers
// showing below whatever this frame's own (shorter) content overwrites.
fn draw(stdout: &mut impl Write, app: &App, cols: u16, rows: u16) -> Result<()> {
    let lines = render(app, cols, rows);
    queue!(stdout, Clear(ClearType::All), MoveTo(0, 0))?;
    for (i, line) in lines.iter().take(rows as usize).enumerate() {
        queue!(stdout, MoveTo(0, i as u16))?;
        if i == 0 {
            queue!(
                stdout,
                SetAttribute(Attribute::Bold),
                Print(line),
                SetAttribute(Attribute::Reset)
            )?;
        } else {
            queue!(stdout, Print(line))?;
        }
    }
    stdout.flush()?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Terminal lifecycle. `TerminalGuard::drop` covers a panic that unwinds
// (debug/test builds); `install_panic_hook` covers `panic = "abort"`
// (release, per Cargo.toml), which never unwinds and so never runs `drop` at
// all — the hook is not a backup for the guard there, it is the only thing
// that runs.

struct TerminalGuard;

impl TerminalGuard {
    fn enter() -> Result<Self> {
        enable_raw_mode().map_err(|e| Error::msg(format!("could not enter raw mode: {e}")))?;
        if let Err(e) = execute!(std::io::stdout(), EnterAlternateScreen, Hide) {
            let _ = disable_raw_mode();
            return Err(Error::msg(format!(
                "could not enter the alternate screen: {e}"
            )));
        }
        Ok(TerminalGuard)
    }
}

impl Drop for TerminalGuard {
    fn drop(&mut self) {
        restore_terminal();
    }
}

fn restore_terminal() {
    let _ = execute!(std::io::stdout(), Show, LeaveAlternateScreen);
    let _ = disable_raw_mode();
}

fn install_panic_hook() {
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        restore_terminal();
        clear_clipboard_best_effort();
        previous(info);
    }));
}

// ---------------------------------------------------------------------------
// Cooked-mode detour for `e`: alternate screen stays up (nothing it prints
// reaches scrollback either), only raw mode toggles off so `read_line` and
// `rpassword` get normal line editing and their own echo handling.

fn edit_secret(app: &mut App, backend: &dyn Backend) -> Result<()> {
    let Some((name, meta)) = app.current().cloned() else {
        app.status = Some("nothing selected".into());
        return Ok(());
    };

    let result = (|| -> Result<()> {
        disable_raw_mode().map_err(|e| Error::msg(format!("could not leave raw mode: {e}")))?;
        execute!(std::io::stdout(), Clear(ClearType::All), MoveTo(0, 0), Show)?;
        println!("editing {name}\n");

        let platform_prompt = format!(
            "New platform (Enter to keep {}): ",
            meta.platform.as_deref().unwrap_or("none")
        );
        let platform_in = crate::prompt_line(&platform_prompt)?;
        let note_prompt = format!(
            "New note (Enter to keep {}): ",
            meta.note.as_deref().unwrap_or("none")
        );
        let note_in = crate::prompt_line(&note_prompt)?;
        let value_in = prompt_new_value(&name)?;

        let meta_update = MetaUpdate {
            platform: crate::platform_update(platform_in)?,
            note: crate::note_update(note_in)?,
        };
        match value_in {
            Some(value) => backend.put(&name, &value, &meta_update)?,
            None if meta_update.platform != FieldUpdate::Keep
                || meta_update.note != FieldUpdate::Keep =>
            {
                backend.set_meta(&name, &meta_update)?
            }
            None => {}
        }
        Ok(())
    })();

    let _ = enable_raw_mode();
    let _ = execute!(std::io::stdout(), Hide);

    match result {
        Ok(()) => {
            app.status = Some(format!("updated: {name}"));
            app.refresh(backend)
        }
        Err(e) => {
            app.status = Some(format!("edit cancelled: {e}"));
            Ok(())
        }
    }
}

// `Enter` alone keeps the current value untouched — the same "blank means
// don't change this field" rule `prompt_line` already uses for platform/note
// (see main.rs's `platform_update`/`note_update`), so a relabel-only edit
// never has to touch the keyring's stored value.
fn prompt_new_value(secret: &str) -> Result<Option<Zeroizing<String>>> {
    let prompt = format!("New value for {secret} (Enter to keep current, not echoed): ");
    let first = Zeroizing::new(
        rpassword::prompt_password(prompt)
            .map_err(|e| Error::msg(format!("could not read a password: {e}")))?,
    );
    if first.is_empty() {
        return Ok(None);
    }
    let second = Zeroizing::new(
        rpassword::prompt_password("Confirm (not echoed): ")
            .map_err(|e| Error::msg(format!("could not read a password: {e}")))?,
    );
    if *first != *second {
        return Err(Error::msg("values did not match, nothing changed"));
    }
    Ok(Some(first))
}

// ---------------------------------------------------------------------------
// Main loop.

fn run_interactive(
    backend: &dyn Backend,
    entries: Vec<(String, SecretMeta)>,
    clip: &mut dyn ClipboardPort,
) -> Result<i32> {
    let _guard = TerminalGuard::enter()?;
    install_panic_hook();

    let mut app = App::new(entries);
    let mut iterations: u32 = 0;

    loop {
        let (cols, rows) = size().unwrap_or((MIN_COLS, MIN_ROWS));
        draw(&mut std::io::stdout(), &app, cols, rows)?;

        {
            let mut restore = CLIP_RESTORE.lock().unwrap();
            *restore = app.clip.as_ref().map(|c| c.previous.clone());
        }

        // Test-only escape hatch: an external pty-driven test can force the
        // panic-safety path without depending on a real bug to trigger it.
        // Never set by anything other than a deliberate test invocation.
        if std::env::var_os("MASKRUN_TUI_TEST_PANIC").is_some() && iterations == 1 {
            panic!("maskrun tui: forced test panic (MASKRUN_TUI_TEST_PANIC)");
        }
        iterations += 1;

        if event::poll(POLL_INTERVAL)? {
            match event::read()? {
                Event::Key(KeyEvent {
                    code,
                    kind: KeyEventKind::Press,
                    modifiers,
                    ..
                }) => {
                    if modifiers.contains(KeyModifiers::CONTROL) && code == KeyCode::Char('c') {
                        app.quit = true;
                    } else if code == KeyCode::Char('e') && !app.confirm_delete {
                        edit_secret(&mut app, backend)?;
                    } else {
                        handle_key(&mut app, code, backend, clip)?;
                    }
                }
                Event::Key(_) | Event::Resize(_, _) | Event::Mouse(_) | Event::Paste(_) => {}
                Event::FocusGained | Event::FocusLost => {}
            }
        } else {
            app.tick_clipboard(clip);
        }

        if app.quit {
            break;
        }
    }

    app.clear_clipboard(clip, "clipboard cleared");
    {
        let mut restore = CLIP_RESTORE.lock().unwrap();
        *restore = None;
    }
    Ok(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;
    use std::collections::HashMap;

    fn meta(platform: Option<&str>, note: Option<&str>) -> SecretMeta {
        SecretMeta {
            platform: platform.map(String::from),
            note: note.map(String::from),
        }
    }

    fn apply(update: &FieldUpdate, existing: Option<String>) -> Option<String> {
        match update {
            FieldUpdate::Keep => existing,
            FieldUpdate::Clear => None,
            FieldUpdate::Set(v) => Some(v.clone()),
        }
    }

    #[derive(Default)]
    struct MockBackend {
        store: RefCell<HashMap<String, (String, SecretMeta)>>,
    }

    impl MockBackend {
        fn with(entries: &[(&str, &str, Option<&str>, Option<&str>)]) -> Self {
            let mut store = HashMap::new();
            for (name, value, platform, note) in entries {
                store.insert(
                    name.to_string(),
                    (value.to_string(), meta(*platform, *note)),
                );
            }
            MockBackend {
                store: RefCell::new(store),
            }
        }
    }

    impl Backend for MockBackend {
        fn name(&self) -> &'static str {
            "mock"
        }
        fn put(&self, secret: &str, value: &str, update: &MetaUpdate) -> Result<()> {
            let mut store = self.store.borrow_mut();
            let existing = store
                .get(secret)
                .map(|(_, m)| m.clone())
                .unwrap_or_default();
            let merged = SecretMeta {
                platform: apply(&update.platform, existing.platform),
                note: apply(&update.note, existing.note),
            };
            store.insert(secret.to_string(), (value.to_string(), merged));
            Ok(())
        }
        fn get(&self, secret: &str) -> Result<Option<String>> {
            Ok(self.store.borrow().get(secret).map(|(v, _)| v.clone()))
        }
        fn get_meta(&self, secret: &str) -> Result<SecretMeta> {
            Ok(self
                .store
                .borrow()
                .get(secret)
                .map(|(_, m)| m.clone())
                .unwrap_or_default())
        }
        fn set_meta(&self, secret: &str, update: &MetaUpdate) -> Result<()> {
            let mut store = self.store.borrow_mut();
            let Some((value, existing)) = store.get(secret).cloned() else {
                return Err(Error::msg(format!("no such secret: {secret}")));
            };
            let merged = SecretMeta {
                platform: apply(&update.platform, existing.platform),
                note: apply(&update.note, existing.note),
            };
            store.insert(secret.to_string(), (value, merged));
            Ok(())
        }
        fn delete(&self, secret: &str) -> Result<()> {
            self.store.borrow_mut().remove(secret);
            Ok(())
        }
        fn list(&self) -> Result<Vec<String>> {
            Ok(self.store.borrow().keys().cloned().collect())
        }
        fn list_meta(&self) -> Result<Vec<(String, SecretMeta)>> {
            Ok(self
                .store
                .borrow()
                .iter()
                .map(|(k, (_, m))| (k.clone(), m.clone()))
                .collect())
        }
    }

    #[derive(Default)]
    struct FakeClipboard {
        content: Option<String>,
        reachable: bool,
        sets: u32,
    }

    impl FakeClipboard {
        fn reachable() -> Self {
            FakeClipboard {
                content: None,
                reachable: true,
                sets: 0,
            }
        }
        fn unreachable() -> Self {
            FakeClipboard {
                content: None,
                reachable: false,
                sets: 0,
            }
        }
    }

    impl ClipboardPort for FakeClipboard {
        fn get_text(&mut self) -> Option<String> {
            self.content.clone()
        }
        fn set_text(&mut self, text: &str) -> bool {
            if !self.reachable {
                return false;
            }
            self.sets += 1;
            self.content = Some(text.to_string());
            true
        }
        fn clear(&mut self) -> bool {
            if !self.reachable {
                return false;
            }
            self.content = None;
            true
        }
    }

    fn sorted(mut entries: Vec<(String, SecretMeta)>) -> Vec<(String, SecretMeta)> {
        entries.sort_by(|a, b| a.0.cmp(&b.0));
        entries
    }

    fn sample_entries() -> Vec<(String, SecretMeta)> {
        sorted(vec![
            ("gh-a".into(), meta(Some("github"), Some("first"))),
            ("gh-b".into(), meta(Some("github"), None)),
            ("vercel-x".into(), meta(Some("vercel"), None)),
            ("orphan".into(), meta(None, None)),
        ])
    }

    #[test]
    fn groups_platforms_alphabetically_with_no_platform_last() {
        let groups = group_entries(&sample_entries());
        let labels: Vec<Option<&str>> = groups.iter().map(|g| g.label.as_deref()).collect();
        assert_eq!(labels, vec![Some("github"), Some("vercel"), None]);
        assert_eq!(groups[0].rows.len(), 2);
    }

    #[test]
    fn empty_vault_groups_to_nothing() {
        assert!(group_entries(&[]).is_empty());
    }

    #[test]
    fn move_platform_clamps_instead_of_wrapping() {
        let mut app = App::new(sample_entries());
        app.move_platform(-5);
        assert_eq!(app.sel_group, 0);
        app.move_platform(5);
        assert_eq!(app.sel_group, app.groups.len() - 1);
    }

    #[test]
    fn move_platform_resets_secret_selection_and_reveal() {
        let mut app = App::new(sample_entries());
        app.enter();
        app.move_secret(1);
        app.revealed = Some(Zeroizing::new("x".into()));
        app.move_platform(1);
        assert_eq!(app.sel_row, 0);
        assert!(app.revealed.is_none());
    }

    #[test]
    fn enter_moves_focus_only_when_group_has_secrets() {
        let mut app = App::new(sample_entries());
        app.enter();
        assert!(app.focus == Focus::Secrets);
        app.back();
        assert!(app.focus == Focus::Platforms);
    }

    #[test]
    fn current_index_tracks_selection() {
        let app = App::new(sample_entries());
        assert_eq!(app.current().unwrap().0, "gh-a");
    }

    #[test]
    fn reveal_does_not_read_the_backend_until_v_is_pressed() {
        let backend = MockBackend::with(&[("gh-a", "secretvalue", Some("github"), None)]);
        let mut app = App::new(sample_entries());
        assert!(app.revealed.is_none());
        app.toggle_reveal(&backend);
        assert_eq!(
            app.revealed.as_ref().map(|v| v.as_str()),
            Some("secretvalue")
        );
        app.toggle_reveal(&backend);
        assert!(app.revealed.is_none());
    }

    #[test]
    fn navigating_away_hides_a_revealed_value() {
        let backend = MockBackend::with(&[("gh-a", "v", Some("github"), None)]);
        let mut app = App::new(sample_entries());
        app.toggle_reveal(&backend);
        assert!(app.revealed.is_some());
        app.enter();
        app.move_secret(1);
        assert!(app.revealed.is_none());
    }

    #[test]
    fn delete_requires_confirmation_and_a_wrong_key_cancels() {
        let backend = MockBackend::with(&[
            ("gh-a", "v1", Some("github"), None),
            ("gh-b", "v2", Some("github"), None),
        ]);
        let mut app = App::new(sample_entries());
        let mut clip = FakeClipboard::reachable();
        handle_key(&mut app, KeyCode::Char('d'), &backend, &mut clip).unwrap();
        assert!(app.confirm_delete);
        handle_key(&mut app, KeyCode::Char('n'), &backend, &mut clip).unwrap();
        assert!(!app.confirm_delete);
        assert!(backend.get("gh-a").unwrap().is_some());
    }

    // Locks down "a blind key sequence can never delete something": every
    // key except a literal 'y'/'Y' must cancel, including Enter, Esc, the
    // arrows, and 'd' pressed again — a second incident (see
    // Sessions/2026-09-21-tui.md) came from a driver script firing keys
    // without checking the screen first, so this is regression coverage for
    // "default is no", not just for the happy path.
    #[test]
    fn delete_confirmation_defaults_to_no_for_every_key_but_y() {
        let backend = MockBackend::with(&[("gh-a", "v1", Some("github"), None)]);
        let mut clip = FakeClipboard::reachable();
        for key in [
            KeyCode::Enter,
            KeyCode::Esc,
            KeyCode::Up,
            KeyCode::Down,
            KeyCode::Left,
            KeyCode::Right,
            KeyCode::Char('d'),
            KeyCode::Char('v'),
            KeyCode::Char('n'),
            KeyCode::Char('q'),
            KeyCode::Char(' '),
        ] {
            let mut app = App::new(sample_entries());
            handle_key(&mut app, KeyCode::Char('d'), &backend, &mut clip).unwrap();
            assert!(app.confirm_delete);
            handle_key(&mut app, key, &backend, &mut clip).unwrap();
            assert!(!app.confirm_delete, "key {key:?} should not confirm delete");
            assert!(
                backend.get("gh-a").unwrap().is_some(),
                "key {key:?} deleted gh-a"
            );
        }
    }

    #[test]
    fn delete_confirmation_prompt_names_the_exact_secret() {
        let mut app = App::new(sample_entries());
        app.confirm_delete = true;
        let lines = render(&app, 80, 20).join("\n");
        let (name, _) = app.current().unwrap();
        assert!(lines.contains(&format!("delete '{name}'? [y/N]")));
    }

    #[test]
    fn delete_confirmed_removes_the_secret_and_refreshes() {
        let backend = MockBackend::with(&[
            ("gh-a", "v1", Some("github"), None),
            ("gh-b", "v2", Some("github"), None),
        ]);
        let mut app = App::new(sample_entries());
        let mut clip = FakeClipboard::reachable();
        handle_key(&mut app, KeyCode::Char('d'), &backend, &mut clip).unwrap();
        handle_key(&mut app, KeyCode::Char('y'), &backend, &mut clip).unwrap();
        assert!(backend.get("gh-a").unwrap().is_none());
        assert!(!app.entries.iter().any(|(n, _)| n == "gh-a"));
    }

    #[test]
    fn copy_captures_previous_clipboard_content() {
        let backend = MockBackend::with(&[("gh-a", "secretvalue", Some("github"), None)]);
        let mut app = App::new(sample_entries());
        let mut clip = FakeClipboard::reachable();
        clip.content = Some("previous-thing".into());
        app.copy(&backend, &mut clip);
        assert_eq!(clip.content.as_deref(), Some("secretvalue"));
        assert_eq!(clip.sets, 1);
        assert!(app.clip.is_some());
        assert_eq!(
            app.clip.as_ref().unwrap().previous.as_deref(),
            Some("previous-thing")
        );
    }

    #[test]
    fn manual_clear_restores_previous_content() {
        let backend = MockBackend::with(&[("gh-a", "secretvalue", Some("github"), None)]);
        let mut app = App::new(sample_entries());
        let mut clip = FakeClipboard::reachable();
        clip.content = Some("previous-thing".into());
        app.copy(&backend, &mut clip);
        app.clear_clipboard(&mut clip, "clipboard cleared");
        assert_eq!(clip.content.as_deref(), Some("previous-thing"));
        assert!(app.clip.is_none());
    }

    #[test]
    fn manual_clear_empties_when_there_was_nothing_before() {
        let backend = MockBackend::with(&[("gh-a", "secretvalue", Some("github"), None)]);
        let mut app = App::new(sample_entries());
        let mut clip = FakeClipboard::reachable();
        app.copy(&backend, &mut clip);
        app.clear_clipboard(&mut clip, "clipboard cleared");
        assert!(clip.content.is_none());
    }

    #[test]
    fn tick_does_nothing_before_the_timeout() {
        let backend = MockBackend::with(&[("gh-a", "secretvalue", Some("github"), None)]);
        let mut app = App::new(sample_entries());
        let mut clip = FakeClipboard::reachable();
        app.copy(&backend, &mut clip);
        app.tick_clipboard(&mut clip);
        assert!(app.clip.is_some());
        assert_eq!(clip.content.as_deref(), Some("secretvalue"));
    }

    #[test]
    fn tick_clears_once_the_timeout_has_elapsed() {
        let mut app = App::new(sample_entries());
        let mut clip = FakeClipboard::reachable();
        clip.content = Some("secretvalue".into());
        app.clip = Some(ClipState {
            started: Instant::now() - CLIP_TIMEOUT - Duration::from_secs(1),
            previous: Some("previous-thing".into()),
        });
        app.tick_clipboard(&mut clip);
        assert!(app.clip.is_none());
        assert_eq!(clip.content.as_deref(), Some("previous-thing"));
    }

    #[test]
    fn copy_reports_status_when_clipboard_is_unreachable() {
        let backend = MockBackend::with(&[("gh-a", "secretvalue", Some("github"), None)]);
        let mut app = App::new(sample_entries());
        let mut clip = FakeClipboard::unreachable();
        app.copy(&backend, &mut clip);
        assert!(app.clip.is_none());
        assert!(app.status.as_deref().unwrap().contains("could not reach"));
    }

    #[test]
    fn copy_on_empty_vault_is_a_safe_no_op() {
        let backend = MockBackend::default();
        let mut app = App::new(Vec::new());
        let mut clip = FakeClipboard::reachable();
        app.copy(&backend, &mut clip);
        assert!(app.clip.is_none());
        assert_eq!(app.status.as_deref(), Some("nothing to copy"));
    }

    #[test]
    fn edit_updates_platform_note_and_value() {
        let backend = MockBackend::with(&[("gh-a", "old-value", Some("github"), None)]);
        let mut app = App::new(sample_entries());
        let update = MetaUpdate {
            platform: FieldUpdate::Keep,
            note: FieldUpdate::Set("retagged".into()),
        };
        backend.set_meta("gh-a", &update).unwrap();
        app.refresh(&backend).unwrap();
        assert_eq!(app.current().unwrap().1.note.as_deref(), Some("retagged"));
        assert_eq!(backend.get("gh-a").unwrap().as_deref(), Some("old-value"));
    }

    #[test]
    fn render_shows_masked_placeholder_before_reveal() {
        let app = App::new(sample_entries());
        let lines = render(&app, 80, 20).join("\n");
        assert!(lines.contains(MASKED_PLACEHOLDER));
        assert!(!lines.contains("gh-a-value-should-never-appear"));
    }

    #[test]
    fn render_shows_the_real_value_only_after_reveal() {
        let backend = MockBackend::with(&[("gh-a", "shown-after-v", Some("github"), None)]);
        let mut app = App::new(sample_entries());
        app.toggle_reveal(&backend);
        let lines = render(&app, 80, 20).join("\n");
        assert!(lines.contains("shown-after-v"));
    }

    #[test]
    fn render_empty_vault_tells_the_user_what_to_do() {
        let app = App::new(Vec::new());
        let lines = render(&app, 80, 20).join("\n");
        assert!(lines.contains("maskrun put"));
    }

    #[test]
    fn render_shows_delete_confirmation_prompt() {
        let mut app = App::new(sample_entries());
        app.confirm_delete = true;
        let lines = render(&app, 80, 20).join("\n");
        assert!(lines.contains("delete 'gh-a'?"));
    }

    #[test]
    fn render_shows_clipboard_countdown() {
        let mut app = App::new(sample_entries());
        app.clip = Some(ClipState {
            started: Instant::now(),
            previous: None,
        });
        let lines = render(&app, 80, 20).join("\n");
        assert!(lines.contains("clipboard clears in"));
    }

    #[test]
    fn render_never_panics_at_the_minimum_supported_size() {
        let app = App::new(sample_entries());
        let lines = render(&app, MIN_COLS, MIN_ROWS);
        assert!(!lines.is_empty());
        for line in &lines {
            assert!(line.chars().count() <= MIN_COLS as usize);
        }
    }

    #[test]
    fn render_handles_many_secrets_without_index_panics() {
        let mut entries = Vec::new();
        for i in 0..200 {
            entries.push((format!("secret-{i:03}"), meta(Some("github"), None)));
        }
        let app = App::new(sorted(entries));
        let lines = render(&app, 80, 20);
        assert!(!lines.is_empty());
    }

    #[test]
    fn handle_key_quit_sets_the_flag() {
        let backend = MockBackend::default();
        let mut app = App::new(Vec::new());
        let mut clip = FakeClipboard::reachable();
        handle_key(&mut app, KeyCode::Char('q'), &backend, &mut clip).unwrap();
        assert!(app.quit);
    }

    #[test]
    fn handle_key_on_empty_vault_never_panics() {
        let backend = MockBackend::default();
        let mut app = App::new(Vec::new());
        let mut clip = FakeClipboard::reachable();
        for key in [
            KeyCode::Up,
            KeyCode::Down,
            KeyCode::Right,
            KeyCode::Left,
            KeyCode::Char('v'),
            KeyCode::Char('d'),
            KeyCode::Char('y'),
            KeyCode::Char('c'),
        ] {
            handle_key(&mut app, key, &backend, &mut clip).unwrap();
        }
    }

    #[test]
    fn clip_state_remaining_never_underflows_past_zero() {
        let state = ClipState {
            started: Instant::now() - CLIP_TIMEOUT - Duration::from_secs(30),
            previous: None,
        };
        assert_eq!(state.remaining(), Duration::ZERO);
        assert!(state.expired());
    }
}
