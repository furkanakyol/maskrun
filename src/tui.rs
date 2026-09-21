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
use crossterm::event::{
    self, DisableBracketedPaste, EnableBracketedPaste, Event, KeyCode, KeyEvent, KeyEventKind,
    KeyModifiers,
};
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

    fn reload(&mut self, backend: &dyn Backend) -> Result<()> {
        let mut entries = backend.list_meta()?;
        entries.sort_by(|a, b| a.0.cmp(&b.0));
        self.groups = group_entries(&entries);
        self.entries = entries;
        self.revealed = None;
        Ok(())
    }

    // Finds `name` and puts the cursor on it; if absent (or `name` is
    // `None`), clamps the group cursor and resets the row instead. Returns
    // whether it actually found the secret.
    fn select(&mut self, name: Option<&str>) -> bool {
        if let Some(name) = name {
            for (gi, group) in self.groups.iter().enumerate() {
                if let Some(ri) = group.rows.iter().position(|&i| self.entries[i].0 == name) {
                    self.sel_group = gi;
                    self.sel_row = ri;
                    return true;
                }
            }
        }
        self.sel_group = self.sel_group.min(self.groups.len().saturating_sub(1));
        self.sel_row = 0;
        false
    }

    // Re-reads the keyring after a mutation (edit/delete) and keeps the
    // cursor on the same secret when it still exists, instead of resetting
    // to the top of the list every time.
    fn refresh(&mut self, backend: &dyn Backend) -> Result<()> {
        let previous = self.current().map(|(n, _)| n.clone());
        self.reload(backend)?;
        if !self.select(previous.as_deref()) {
            self.focus = Focus::Platforms;
        }
        Ok(())
    }

    // After adding a secret there's no "previous selection" to preserve —
    // the one just written is what the user wants to see, highlighted.
    fn refresh_selecting(&mut self, backend: &dyn Backend, name: &str) -> Result<()> {
        self.reload(backend)?;
        self.select(Some(name));
        self.focus = Focus::Secrets;
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
        let body_rows =
            rows.saturating_sub(lines.len() + 4 /* blank + hints + clip + status */);
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

        // Anchored at the selected group's own left-pane row: starting at
        // row 0 printed the highlighted secret beside whatever unrelated
        // group happened to sit at that absolute row on the left.
        let mut right: Vec<String> = vec![String::new(); app.sel_group];
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
    lines.push(hint_line(app, cols));
    lines.push(clip_line(app));
    lines.push(status_line(app));

    lines
        .into_iter()
        .map(|l| l.chars().take(cols).collect())
        .collect()
}

// The full labels when they fit; a bare-keys fallback otherwise, instead of
// cutting the long form mid-word (e.g. "q quit" -> "q qu" at 84 columns
// once "c clear" is in the mix).
fn hint_line(app: &App, cols: usize) -> String {
    let mut full =
        " ↑↓ move   → enter   ← back   a add   e edit   d delete   v reveal   y copy".to_string();
    if app.clip.is_some() {
        full.push_str("   c clear");
    }
    full.push_str("   q quit");
    if full.chars().count() <= cols {
        return full;
    }

    let mut short = " ↑↓ → ← a e d v y".to_string();
    if app.clip.is_some() {
        short.push_str(" c");
    }
    short.push_str(" q");
    short.chars().take(cols).collect()
}

fn clip_line(app: &App) -> String {
    match &app.clip {
        Some(clip) => format!(" clipboard clears in {}s", clip.remaining().as_secs()),
        None => String::new(),
    }
}

// Kept on its own line and apart from `clip_line`: a cancelled delete and an
// active clipboard countdown used to share this line and get concatenated
// into one confusing message.
fn status_line(app: &App) -> String {
    if app.confirm_delete {
        let name = app.current().map(|(n, _)| n.as_str()).unwrap_or("?");
        return format!(" delete '{name}'? [y/N]");
    }
    match &app.status {
        Some(s) => format!(" {s}"),
        None => String::new(),
    }
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
    draw_lines(stdout, &lines, rows)
}

fn draw_lines(stdout: &mut impl Write, lines: &[String], rows: u16) -> Result<()> {
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

// Terminal lifecycle. `TerminalGuard::drop` covers a panic that unwinds
// (debug/test builds); `install_panic_hook` covers `panic = "abort"`
// (release, per Cargo.toml), which never unwinds and so never runs `drop` at
// all — the hook is not a backup for the guard there, it is the only thing
// that runs.

struct TerminalGuard;

impl TerminalGuard {
    fn enter() -> Result<Self> {
        enable_raw_mode().map_err(|e| Error::msg(format!("could not enter raw mode: {e}")))?;
        if let Err(e) = execute!(
            std::io::stdout(),
            EnterAlternateScreen,
            Hide,
            EnableBracketedPaste
        ) {
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
    let _ = execute!(
        std::io::stdout(),
        DisableBracketedPaste,
        Show,
        LeaveAlternateScreen
    );
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

// Full-screen add-secret form. Runs its own small event loop, like
// `run_interactive`'s minus the clipboard tick, instead of `edit_secret`'s
// cooked-mode detour: alternate screen and raw mode stay up throughout, so
// the value fields are masked by hand rather than through `rpassword`
// (which needs a real cooked terminal to hide input).

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum AddField {
    Name,
    Platform,
    Note,
    Value,
    Confirm,
}

impl AddField {
    fn next(self) -> Self {
        match self {
            AddField::Name => AddField::Platform,
            AddField::Platform => AddField::Note,
            AddField::Note => AddField::Value,
            AddField::Value => AddField::Confirm,
            AddField::Confirm => AddField::Confirm,
        }
    }
    fn prev(self) -> Self {
        match self {
            AddField::Name => AddField::Name,
            AddField::Platform => AddField::Name,
            AddField::Note => AddField::Platform,
            AddField::Value => AddField::Note,
            AddField::Confirm => AddField::Value,
        }
    }
}

struct AddForm {
    field: AddField,
    name: String,
    platform: String,
    note: String,
    value: Zeroizing<String>,
    confirm: Zeroizing<String>,
    error: Option<String>,
    reveal: bool,
}

impl AddForm {
    fn new() -> Self {
        AddForm {
            field: AddField::Name,
            name: String::new(),
            platform: String::new(),
            note: String::new(),
            value: Zeroizing::new(String::new()),
            confirm: Zeroizing::new(String::new()),
            error: None,
            reveal: false,
        }
    }

    fn push_char(&mut self, c: char) {
        match self.field {
            AddField::Name => self.name.push(c),
            AddField::Platform => self.platform.push(c),
            AddField::Note => self.note.push(c),
            AddField::Value => self.value.push(c),
            AddField::Confirm => self.confirm.push(c),
        }
    }

    // A pasted secret arrives as one Paste event, so it lands in the field
    // whole. Without this, a token copied with its trailing newline sent that
    // newline through as Enter and moved the cursor to the next field
    // mid-paste, leaving the user typing into `confirm` believing they were
    // still entering the value.
    fn push_paste(&mut self, text: &str) {
        let cleaned: String = text.chars().filter(|c| !c.is_control()).collect();
        match self.field {
            AddField::Name => self.name.push_str(&cleaned),
            AddField::Platform => self.platform.push_str(&cleaned),
            AddField::Note => self.note.push_str(&cleaned),
            AddField::Value => self.value.push_str(&cleaned),
            AddField::Confirm => self.confirm.push_str(&cleaned),
        }
    }

    fn backspace(&mut self) {
        match self.field {
            AddField::Name => {
                self.name.pop();
            }
            AddField::Platform => {
                self.platform.pop();
            }
            AddField::Note => {
                self.note.pop();
            }
            AddField::Value => {
                self.value.pop();
            }
            AddField::Confirm => {
                self.confirm.pop();
            }
        }
    }

    fn cycle_platform(&mut self, platforms: &[String], delta: i32) {
        if self.field != AddField::Platform {
            return;
        }
        let mut options = vec![String::new()];
        options.extend(platforms.iter().cloned());
        let current = options
            .iter()
            .position(|p| p == &self.platform)
            .unwrap_or(0);
        let next = clamp_move(current, delta, options.len() - 1);
        self.platform = options[next].clone();
    }

    fn back(&mut self) {
        self.error = None;
        self.field = self.field.prev();
    }

    // Enter/Tab: validate the field being left and, only on success, move
    // to the next one. Returns `true` once the confirm field matches the
    // value field — the caller's cue to actually store the secret.
    fn advance(&mut self, existing: &[(String, SecretMeta)]) -> bool {
        self.error = None;
        match self.field {
            AddField::Name => {
                if self.name.is_empty() {
                    self.error = Some("name is required".into());
                } else if let Err(e) = crate::keyring::check_name(&self.name) {
                    self.error = Some(e.to_string());
                } else if existing.iter().any(|(n, _)| n == &self.name) {
                    self.error = Some(format!("{} already exists", self.name));
                } else {
                    self.field = self.field.next();
                }
            }
            AddField::Platform => {
                if self.platform.is_empty() {
                    self.field = self.field.next();
                } else if let Err(e) = crate::keyring::check_name(&self.platform) {
                    self.error = Some(e.to_string());
                } else {
                    self.field = self.field.next();
                }
            }
            AddField::Note => {
                if self.note.is_empty() {
                    self.field = self.field.next();
                } else if let Err(e) = crate::keyring::check_note(&self.note) {
                    self.error = Some(e.to_string());
                } else {
                    self.field = self.field.next();
                }
            }
            AddField::Value => {
                if self.value.is_empty() {
                    self.error = Some("value is required".into());
                } else {
                    self.field = self.field.next();
                }
            }
            AddField::Confirm => {
                if *self.value == *self.confirm {
                    return true;
                }
                self.error = Some("values did not match".into());
                self.confirm = Zeroizing::new(String::new());
            }
        }
        false
    }
}

fn masked(value: &Zeroizing<String>) -> String {
    "•".repeat(value.chars().count())
}

fn field_line(label: &str, value: &str, active: bool) -> String {
    let marker = if active { "▸" } else { " " };
    format!(" {marker} {:<10}{}", format!("{label}:"), value)
}

fn render_add_form(form: &AddForm, platforms: &[String], cols: usize) -> Vec<String> {
    let mut lines = Vec::new();
    lines.push(pad_between(" maskrun", "add secret", cols));
    lines.push(String::new());

    lines.push(field_line("name", &form.name, form.field == AddField::Name));
    lines.push(field_line(
        "platform",
        &form.platform,
        form.field == AddField::Platform,
    ));
    lines.push(if platforms.is_empty() {
        String::new()
    } else {
        format!("   existing: {}", platforms.join(", "))
    });
    lines.push(field_line("note", &form.note, form.field == AddField::Note));
    let shown = |v: &Zeroizing<String>| {
        if form.reveal {
            v.to_string()
        } else {
            masked(v)
        }
    };
    lines.push(field_line(
        "value",
        &shown(&form.value),
        form.field == AddField::Value,
    ));
    lines.push(field_line(
        "confirm",
        &shown(&form.confirm),
        form.field == AddField::Confirm,
    ));

    lines.push(String::new());
    lines.push(match &form.error {
        Some(e) => format!(" ! {e}"),
        None => String::new(),
    });
    lines.push(" Enter/Tab next   Shift-Tab back   ^R reveal   Esc cancel".to_string());

    lines
        .into_iter()
        .map(|l| l.chars().take(cols).collect())
        .collect()
}

fn draw_add_form(
    stdout: &mut impl Write,
    form: &AddForm,
    platforms: &[String],
    cols: u16,
    rows: u16,
) -> Result<()> {
    let lines = render_add_form(form, platforms, cols as usize);
    draw_lines(stdout, &lines, rows)
}

// Esc drops `form` — including its two `Zeroizing` value buffers — without
// ever calling `backend.put`, so cancelling never leaves the typed value
// sitting in memory or in the vault.
fn run_add_form(app: &mut App, backend: &dyn Backend) -> Result<()> {
    let mut platforms: Vec<String> = app
        .entries
        .iter()
        .filter_map(|(_, m)| m.platform.clone())
        .collect();
    platforms.sort();
    platforms.dedup();

    let mut form = AddForm::new();
    loop {
        let (cols, rows) = size().unwrap_or((MIN_COLS, MIN_ROWS));
        draw_add_form(&mut std::io::stdout(), &form, &platforms, cols, rows)?;

        let (code, modifiers) = match event::read()? {
            Event::Paste(text) => {
                form.push_paste(&text);
                continue;
            }
            Event::Key(KeyEvent {
                code,
                kind: KeyEventKind::Press,
                modifiers,
                ..
            }) => (code, modifiers),
            _ => continue,
        };

        if modifiers.contains(KeyModifiers::CONTROL) && code == KeyCode::Char('c') {
            return Ok(());
        }
        if modifiers.contains(KeyModifiers::CONTROL) && code == KeyCode::Char('r') {
            form.reveal = !form.reveal;
            continue;
        }

        match code {
            KeyCode::Esc => return Ok(()),
            KeyCode::Up => form.cycle_platform(&platforms, -1),
            KeyCode::Down => form.cycle_platform(&platforms, 1),
            KeyCode::BackTab => form.back(),
            KeyCode::Backspace => form.backspace(),
            KeyCode::Tab | KeyCode::Enter => {
                if form.advance(&app.entries) {
                    let meta = MetaUpdate {
                        platform: crate::platform_update(
                            (!form.platform.is_empty()).then(|| form.platform.clone()),
                        )?,
                        note: crate::note_update(
                            (!form.note.is_empty()).then(|| form.note.clone()),
                        )?,
                    };
                    match backend.put(&form.name, &form.value, &meta) {
                        Ok(()) => {
                            let name = form.name.clone();
                            app.refresh_selecting(backend, &name)?;
                            app.status = Some(format!("added {name}"));
                            return Ok(());
                        }
                        Err(e) => form.error = Some(format!("error: {e}")),
                    }
                }
            }
            KeyCode::Char(c)
                if !modifiers.contains(KeyModifiers::CONTROL)
                    && !modifiers.contains(KeyModifiers::ALT) =>
            {
                form.push_char(c)
            }
            _ => {}
        }
    }
}

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
                    } else if code == KeyCode::Char('a') && !app.confirm_delete {
                        app.clear_transients();
                        run_add_form(&mut app, backend)?;
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

    #[test]
    fn secret_marker_lines_up_with_its_own_platform_row() {
        let mut app = App::new(sample_entries());
        app.move_platform(1); // github -> vercel
        app.enter();
        let lines = render(&app, 80, 20);
        let platform_row = lines.iter().position(|l| l.contains("vercel")).unwrap();
        let secret_row = lines.iter().position(|l| l.contains("▸ vercel-x")).unwrap();
        assert_eq!(
            platform_row, secret_row,
            "the selected secret should print beside its own platform, not row 0"
        );
    }

    #[test]
    fn cancel_message_and_clipboard_countdown_stay_on_separate_lines() {
        let backend = MockBackend::with(&[("gh-a", "v1", Some("github"), None)]);
        let mut app = App::new(sample_entries());
        let mut clip = FakeClipboard::reachable();
        app.copy(&backend, &mut clip);
        handle_key(&mut app, KeyCode::Char('d'), &backend, &mut clip).unwrap();
        handle_key(&mut app, KeyCode::Esc, &backend, &mut clip).unwrap();
        assert_eq!(app.status.as_deref(), Some("cancelled"));

        let lines = render(&app, 80, 20);
        let status = lines.last().unwrap();
        assert_eq!(status, " cancelled");
        assert!(!status.contains("clipboard"));
        assert!(lines.iter().any(|l| l.contains("clipboard clears in")));
    }

    #[test]
    fn hint_never_truncates_a_word_when_the_terminal_is_narrow() {
        let mut app = App::new(sample_entries());
        app.clip = Some(ClipState {
            started: Instant::now(),
            previous: None,
        });
        let hint = hint_line(&app, 84);
        assert!(hint.chars().count() <= 84);
        assert!(hint.ends_with('q') || hint.ends_with("quit"));
    }

    #[test]
    fn hint_shows_the_full_form_when_it_fits() {
        let app = App::new(sample_entries());
        let hint = hint_line(&app, 200);
        assert!(hint.contains("q quit"));
        assert!(hint.contains("a add"));
    }

    #[test]
    fn add_form_walks_through_fields_on_valid_input() {
        let existing = sample_entries();
        let mut form = AddForm::new();
        form.name = "new-secret".into();
        assert!(!form.advance(&existing));
        assert_eq!(form.field, AddField::Platform);
        assert!(!form.advance(&existing));
        assert_eq!(form.field, AddField::Note);
        assert!(!form.advance(&existing));
        assert_eq!(form.field, AddField::Value);
        form.value = Zeroizing::new("s3cr3t".into());
        assert!(!form.advance(&existing));
        assert_eq!(form.field, AddField::Confirm);
        form.confirm = Zeroizing::new("s3cr3t".into());
        assert!(form.advance(&existing));
    }

    #[test]
    fn add_form_blocks_on_empty_name() {
        let mut form = AddForm::new();
        assert!(!form.advance(&sample_entries()));
        assert_eq!(form.field, AddField::Name);
        assert_eq!(form.error.as_deref(), Some("name is required"));
    }

    #[test]
    fn add_form_blocks_on_invalid_name_charset() {
        let mut form = AddForm::new();
        form.name = "has space".into();
        assert!(!form.advance(&sample_entries()));
        assert_eq!(form.field, AddField::Name);
        assert!(form.error.is_some());
    }

    #[test]
    fn add_form_blocks_on_duplicate_name() {
        let mut form = AddForm::new();
        form.name = "gh-a".into();
        assert!(!form.advance(&sample_entries()));
        assert_eq!(form.field, AddField::Name);
        assert_eq!(form.error.as_deref(), Some("gh-a already exists"));
    }

    #[test]
    fn add_form_rejects_mismatched_confirmation_without_cancelling() {
        let mut form = AddForm::new();
        form.field = AddField::Confirm;
        form.value = Zeroizing::new("one".into());
        form.confirm = Zeroizing::new("two".into());
        assert!(!form.advance(&sample_entries()));
        assert_eq!(form.field, AddField::Confirm);
        assert_eq!(form.error.as_deref(), Some("values did not match"));
        assert!(form.confirm.is_empty());
    }

    #[test]
    fn add_form_platform_cycle_includes_blank_and_does_not_wrap() {
        let mut form = AddForm::new();
        form.field = AddField::Platform;
        let platforms = vec!["github".to_string(), "vercel".to_string()];
        form.cycle_platform(&platforms, -1);
        assert_eq!(form.platform, "");
        form.cycle_platform(&platforms, 1);
        assert_eq!(form.platform, "github");
        form.cycle_platform(&platforms, 1);
        assert_eq!(form.platform, "vercel");
        form.cycle_platform(&platforms, 1);
        assert_eq!(form.platform, "vercel");
    }

    #[test]
    fn add_form_back_returns_to_the_previous_field_and_clears_the_error() {
        let mut form = AddForm::new();
        form.field = AddField::Note;
        form.error = Some("stale".into());
        form.back();
        assert_eq!(form.field, AddField::Platform);
        assert!(form.error.is_none());
    }

    #[test]
    fn a_pasted_value_keeps_its_field_even_with_a_trailing_newline() {
        let mut form = AddForm::new();
        form.field = AddField::Value;
        form.push_paste("ghp_token_with_newline\n");
        assert_eq!(form.field, AddField::Value);
        assert_eq!(&*form.value, "ghp_token_with_newline");
    }

    #[test]
    fn revealing_shows_the_value_and_masking_hides_it() {
        let mut form = AddForm::new();
        form.field = AddField::Value;
        form.push_paste("plain-token");
        let hidden = render_add_form(&form, &[], 80).join("\n");
        assert!(!hidden.contains("plain-token"));
        form.reveal = true;
        let shown = render_add_form(&form, &[], 80).join("\n");
        assert!(shown.contains("plain-token"));
    }

    #[test]
    fn render_add_form_masks_the_value_fields() {
        let mut form = AddForm::new();
        form.name = "gh-c".into();
        form.value = Zeroizing::new("topsecretvalue".into());
        form.confirm = Zeroizing::new("top".into());
        let lines = render_add_form(&form, &[], 80).join("\n");
        assert!(!lines.contains("topsecretvalue"));
        assert!(!lines.contains("top"));
        assert!(lines.contains("••••••••••••••"));
    }
}
