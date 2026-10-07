//! thc-tui: the terminal UI. `thc` with no arguments (in a terminal) or `thc tui`.

mod about;
mod app;
mod doc;
mod doc_app;
mod doc_keys;
mod doc_ui;
mod derived;
mod binding_snapshot;
mod presentation_snapshot;
mod overlay_snapshot;
mod row_snapshot;
mod detail_snapshot;
mod clock_snapshot;
mod input_snapshot;
mod update;
mod runtime_effects;
mod data_snapshot;
mod node_row;
mod quiet;
mod snapshot_fmt;
mod input;
mod keymap;
pub mod keys_edit;
mod images;
mod motion;
mod recover;
mod text;
pub mod history;
#[cfg(test)]
mod fuzz;
#[cfg(test)]
mod goldens;
#[cfg(test)]
mod overlays;
#[cfg(test)]
mod lists;
#[cfg(test)]
mod kinds;
mod live;
mod theme;
mod ui;

use anyhow::Result;
use app::App;
use ratatui::crossterm::event::{self, Event, KeyEventKind};
use std::time::{Duration, Instant};
use thc_core::vault::Vault;

pub use theme::Theme;

thread_local! {
    /// Rendering a snapshot: nothing reaches the real terminal or clipboard.
    pub(crate) static SNAPSHOT: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// Run the TUI until the user quits.
/// `start`: `review` opens Log with the review lane on, `log` opens Log.
/// `thc keys`: the effective keymap (keymap.md §7) as text, JSON or the guide's Markdown, and
/// what §8.3 would refuse.
pub fn keys_text() -> String {
    keymap::to_text()
}

pub fn keys_json() -> serde_json::Value {
    keymap::to_json()
}

pub fn keys_markdown() -> String {
    keymap::to_markdown()
}

pub fn keys_conflicts() -> Vec<String> {
    keymap::conflicts()
}

pub fn run(vault: Vault, focus: Option<&str>, start: Option<&str>) -> Result<()> {
    let mut app = if start.is_some() { App::new_deferred(vault)? } else { App::new(vault)? };
    app.live_rx = Some(live::spawn(app.vault.paths.clone()));
    if let Some(id) = focus {
        app.focus_node(id);
    }
    apply_start(&mut app, start);
    about::on_start(&mut app);
    resume(&mut app);
    // ratatui::init sets up the terminal (raw mode, alternate screen, panic hook); frames go
    // through the quiet backend.
    let _ = ratatui::init();
    let mut terminal = ratatui::Terminal::new(quiet::Quiet::new(std::io::stdout()))?;
    // Bracketed paste (a paste is one event, read as Markdown) and focus events (save on blur).
    let _ = ratatui::crossterm::execute!(std::io::stdout(), ratatui::crossterm::event::EnableBracketedPaste, ratatui::crossterm::event::EnableFocusChange);
    // The kitty keyboard protocol where the terminal has it, flag 1 only (disambiguate): Esc
    // with no delay, ⌘ passthrough, ⇧Enter told from Enter (in WezTerm only with
    // `enable_kitty_keyboard = true`; by default it sends ⇧Enter as Enter), and every printable key
    // still arrives as plain text. "Report all keys" (8) turned capitals into lowercase in
    // WezTerm 20240203: its encoder takes Shift from the raw OS event, likely missing there, so
    // ⇧a went out as `CSI 97:65;1u`, which crossterm reads as `a` (0.8.9).
    let kitty = ratatui::crossterm::terminal::supports_keyboard_enhancement().unwrap_or(false);
    if kitty {
        push_keys();
    }
    // The mouse (mouse.md §2): 1000 + 1002 + 1006, and 1003 only with hover.
    let mouse = app.tui_prefs.mouse.then_some(app.tui_prefs.hover);
    if let Some(hover) = mouse {
        mouse_on(hover);
    }
    // The caret wears the vault's accent (OSC 12; vaults.md §10). Home keeps the terminal's own.
    let caret = (!app.theme.is_ansi() && app.theme.accent != theme::Accent::Ember).then(|| app.theme.accent.caret_hex(app.theme.dark()));
    if let Some(hex) = caret {
        use std::io::Write;
        let mut out = std::io::stdout();
        let _ = write!(out, "\x1b]12;{hex}\x07");
        let _ = out.flush();
    }
    // A panic leaves the keyboard and the mouse as the shell expects them (writing.md §6).
    {
        let prev = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            // What's typed and not saved yet goes to recovery.json first (recover.rs): the
            // release build aborts after this hook, so there's no later chance.
            recover::write_on_panic();
            // The terminal as the shell expects it: the cursor shape, paste and focus reports
            // (ratatui's own hook, `prev`, leaves raw mode and the alternate screen).
            {
                use std::io::Write;
                let mut out = std::io::stdout();
                let _ = out.write_all(b"\x1b[0 q\x1b[?2004l\x1b[?1004l");
                if caret.is_some() {
                    let _ = out.write_all(b"\x1b]112\x07");
                }
                let _ = out.flush();
            }
            if kitty {
                pop_keys();
            }
            // Always: `:mouse on` may have turned capture on after start. Releasing modes that
            // were never set is harmless.
            mouse_off();
            prev(info);
        }));
    }
    app.kitty = kitty;
    catch_hangup();
    if let Some(where_) = recover::waiting(&app) {
        app.notice(format!("unsaved lines from a crash wait in {where_} · open it to get them back"));
    }
    tmux_escape_note(&mut app);
    let result = event_loop(&mut terminal, &mut app);
    let _ = terminal.backend_mut().cursor_bar(false);
    if kitty {
        pop_keys();
    }
    if app.tui_prefs.mouse {
        mouse_off();
    }
    app.save_doc(true);
    if caret.is_some() {
        use std::io::Write;
        let mut out = std::io::stdout();
        let _ = out.write_all(b"\x1b]112\x07");
        let _ = out.flush();
    }
    let _ = ratatui::crossterm::execute!(std::io::stdout(), ratatui::crossterm::event::DisableBracketedPaste, ratatui::crossterm::event::DisableFocusChange);
    ratatui::restore();
    if result.is_ok() && app.reexec {
        return reexec(&app);
    }
    result
}

/// The vault picker chose another vault: save here, then the TUI reopens on it, with that
/// vault's settings, keys and colour (vaults.md §8). The session's own state (the terminal, the
/// mouse, the kitty flags) carries over; a snapshot switches into a scratch copy, as it started.
pub(crate) fn switch_vault(app: &mut App, path: &std::path::Path) {
    app.save_doc(true);
    app.remember_caret();
    // Where we were is the last word before the new session reads the history.
    app.history_tick(false);
    app.history.save(&app.vault.paths.cache);
    app.drain_saves(true);
    let real = thc_core::vault::Paths { vault: path.to_path_buf(), cache: thc_core::vault::default_cache(path) };
    let snapshot = SNAPSHOT.with(|s| s.get());
    let (paths, origin) = if snapshot && app.vault.origin.is_some() {
        match thc_core::vault::scratch_copy(&real) {
            Ok(p) => (p, Some(real.clone())),
            Err(e) => return app.error(format!("couldn't open {}: {e:#}", thc_core::vault::tilde(path))),
        }
    } else {
        (real.clone(), None)
    };
    let v = match Vault::open(paths, app.vault.actor.clone(), "tui") {
        Ok(mut v) => {
            v.origin = origin;
            v
        }
        Err(e) => return app.error(format!("couldn't open {}: {e:#}", thc_core::vault::tilde(path))),
    };
    thc_core::settings::init(Some(path));
    keymap::reset();
    let mut next = match App::new(v) {
        Ok(a) => a,
        Err(e) => return app.error(format!("couldn't open {}: {e:#}", thc_core::vault::tilde(path))),
    };
    next.screen_width = app.screen_width;
    next.kitty = app.kitty;
    next.daemon_live = false;
    if !snapshot {
        next.live_rx = Some(live::spawn(next.vault.paths.clone()));
    }
    let name = next.vault_name.clone();
    let focus = app.switch_focus.take();
    let back = app.switch_return.take();
    let place = app.hist_pending.take();
    let back_name = back.as_ref().map(|p| thc_core::registry::Registry::load().name_for(p));
    *app = next;
    match (focus, back) {
        // Opened a row from another vault: it's open here, and Esc returns (vaults.md §3.5).
        (Some(id), back) => {
            app.focus_node(&id);
            app.return_vault = back;
            app.doc_origin = None;
            match back_name {
                Some(b) => app.info(format!("in {name} · Esc returns to {b}")),
                None => app.info(format!("now in {name}")),
            }
        }
        _ => app.info(format!("now in {name}")),
    }
    // A history step into this vault: the rest of the place (§7.2). The history itself is the
    // same file, read again by the new session.
    if let Some(p) = place {
        app.restore_place(p);
    }
}

/// Back from an in-place update: the same view, page or day, selection and filter.
fn resume(app: &mut App) {
    if let Some(p) = std::env::var_os("THC_TUI_RESUME") {
        if let Ok(r) = std::fs::read(&p).map_err(|_| ()).and_then(|b| serde_json::from_slice::<app::Resume>(&b).map_err(|_| ())) {
            app.apply_resume(r);
        }
        let _ = std::fs::remove_file(&p);
    }
}

/// Hand over to the new binary in place: save where we are, then exec `thc tui` on it. Only
/// returns if the exec failed.
fn reexec(app: &App) -> Result<()> {
    use std::os::unix::process::CommandExt;
    let version = match &app.update_state {
        app::UpdateState::Downloading { version } => Some(version.clone()),
        _ => None,
    };
    let state = app.vault.paths.cache.join("tui-resume.json");
    std::fs::write(&state, serde_json::to_vec(&app.resume_state(version))?)?;
    let exe = std::env::current_exe()?;
    let err = std::process::Command::new(&exe).arg("tui").env("THC_TUI_RESUME", &state).exec();
    Err(anyhow::anyhow!("couldn't start the new thc ({}): {err} · run thc again", exe.display()))
}

/// The kitty keyboard flags thc asks for: 1 (disambiguate) only. See `run`.
fn push_keys() {
    use ratatui::crossterm::event::{KeyboardEnhancementFlags as F, PushKeyboardEnhancementFlags};
    let _ = ratatui::crossterm::execute!(std::io::stdout(), PushKeyboardEnhancementFlags(F::DISAMBIGUATE_ESCAPE_CODES));
}

/// Mouse reporting: press/release (1000), drag (1002), SGR coordinates (1006), all motion (1003)
/// for hover only.
fn mouse_on(hover: bool) {
    use std::io::Write;
    let mut out = std::io::stdout();
    let _ = out.write_all(if hover { b"\x1b[?1000h\x1b[?1002h\x1b[?1003h\x1b[?1006h" } else { b"\x1b[?1000h\x1b[?1002h\x1b[?1006h" });
    let _ = out.flush();
}

static HANGUP: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

extern "C" fn on_hangup(_: libc::c_int) {
    HANGUP.store(true, std::sync::atomic::Ordering::SeqCst);
}

/// The terminal is gone without a SIGHUP reaching us (the shell died first, the window was
/// force-closed): stdout is no longer a terminal, or the tty was revoked under it. A TUI left
/// like that once spun at 100% CPU for 8 hours. One cheap ioctl per frame.
fn terminal_gone() -> bool {
    let mut t: libc::termios = unsafe { std::mem::zeroed() };
    unsafe { libc::tcgetattr(libc::STDOUT_FILENO, &mut t) != 0 }
}

/// SIGHUP / SIGTERM / SIGINT set a flag the loop checks (a handler can't save safely itself).
/// Without SA_RESTART, so a read blocked in the terminal returns and the loop sees the flag.
fn catch_hangup() {
    unsafe {
        for sig in [libc::SIGHUP, libc::SIGTERM, libc::SIGINT] {
            let mut sa: libc::sigaction = std::mem::zeroed();
            sa.sa_sigaction = on_hangup as *const () as libc::sighandler_t;
            libc::sigemptyset(&mut sa.sa_mask);
            libc::sigaction(sig, &sa, std::ptr::null_mut());
        }
    }
    // The terminal can vanish with no signal at all (a revoked tty): crossterm then sits in its
    // read and never returns to the loop. Once a second, look; if it's gone, raise the flag and
    // interrupt the main thread, which saves and leaves as for a hangup.
    // And a hard deadline: once asked to leave, thc is gone within a second whatever the loop is
    // doing (a plain `kill` once didn't stop a spinning one). If the loop hasn't saved by
    // then, the unsaved lines go to recovery.json, as after a crash, and the process exits.
    let main = unsafe { libc::pthread_self() } as usize;
    std::thread::spawn(move || {
        let mut since: Option<std::time::Instant> = None;
        let mut gone = false;
        loop {
            std::thread::sleep(std::time::Duration::from_millis(100));
            if HANGUP.load(std::sync::atomic::Ordering::SeqCst) {
                let t = *since.get_or_insert_with(std::time::Instant::now);
                if t.elapsed() >= std::time::Duration::from_millis(1000) {
                    recover::write_on_panic();
                    if !gone {
                        let _ = ratatui::crossterm::terminal::disable_raw_mode();
                        let _ = ratatui::crossterm::execute!(std::io::stdout(), ratatui::crossterm::terminal::LeaveAlternateScreen);
                    }
                    std::process::exit(0);
                }
            }
            if gone || !terminal_gone() {
                continue;
            }
            gone = true;
            HANGUP.store(true, std::sync::atomic::Ordering::SeqCst);
            // crossterm reads a dead tty forever (0 bytes means "read again"): put an empty,
            // non-blocking pipe where it reads, so its poll times out and the loop runs.
            unsafe {
                let mut p = [0; 2];
                if libc::pipe(p.as_mut_ptr()) == 0 {
                    libc::fcntl(p[0], libc::F_SETFL, libc::O_NONBLOCK);
                    libc::dup2(p[0], libc::STDIN_FILENO);
                }
                libc::pthread_kill(main as libc::pthread_t, libc::SIGTERM);
            }
        }
    });
}

fn mouse_off() {
    use std::io::Write;
    let mut out = std::io::stdout();
    let _ = out.write_all(b"\x1b[?1006l\x1b[?1003l\x1b[?1002l\x1b[?1000l");
    let _ = out.flush();
}

fn pop_keys() {
    let _ = ratatui::crossterm::execute!(std::io::stdout(), ratatui::crossterm::event::PopKeyboardEnhancementFlags);
}

/// Inside tmux, Esc waits for tmux's `escape-time` (500 ms by default) before thc sees it:
/// say once how to shorten it (jank B3). Never under THC_TEST (no real tools).
fn tmux_escape_note(app: &mut App) {
    if std::env::var_os("TMUX").is_none() || thc_core::sandbox::active() {
        return;
    }
    let ms = std::process::Command::new("tmux").args(["show-options", "-sv", "escape-time"]).output().ok().and_then(|o| String::from_utf8_lossy(&o.stdout).trim().parse::<u32>().ok());
    if let Some(ms) = ms.filter(|m| *m > 50) {
        app.info(format!("tmux holds Esc for {ms} ms · set -sg escape-time 10 in ~/.tmux.conf"));
    }
}

/// A snapshot's mouse token, dispatched as the event loop would (clicks counted explicitly).
fn mouse_token(app: &mut App, t: &str) -> bool {
    use event::{KeyModifiers, MouseButton as B, MouseEvent, MouseEventKind as K};
    let ev = |kind: K, x: u16, y: u16, modifiers: KeyModifiers| MouseEvent { kind, column: x, row: y, modifiers };
    let xy = |s: &str| -> Option<(u16, u16)> {
        let (x, y) = s.split_once(',')?;
        Some((x.trim().parse().ok()?, y.trim().parse().ok()?))
    };
    let Some((name, arg)) = t.split_once(':') else { return false };
    // With capture off the terminal sends nothing: the token is swallowed.
    if !app.tui_prefs.mouse && matches!(name, "click" | "dclick" | "tclick" | "sclick" | "cclick" | "aclick" | "mclick" | "drag" | "wheel" | "hover") {
        return true;
    }
    let press = |app: &mut App, button: B, x: u16, y: u16, mods: KeyModifiers, clicks: u8| {
        input::handle_mouse(app, ev(K::Down(button), x, y, mods), clicks);
        input::handle_mouse(app, ev(K::Up(button), x, y, mods), clicks);
    };
    match name {
        "click" | "dclick" | "tclick" | "sclick" | "cclick" | "aclick" | "mclick" => {
            let Some((x, y)) = xy(arg) else { return false };
            let (button, mods, n) = match name {
                "dclick" => (B::Left, KeyModifiers::NONE, 2),
                "tclick" => (B::Left, KeyModifiers::NONE, 3),
                "sclick" => (B::Left, KeyModifiers::SHIFT, 1),
                "cclick" => (B::Left, KeyModifiers::CONTROL, 1),
                "aclick" => (B::Left, KeyModifiers::ALT, 1),
                "mclick" => (B::Middle, KeyModifiers::NONE, 1),
                _ => (B::Left, KeyModifiers::NONE, 1),
            };
            if n > 1 {
                for k in 1..n {
                    press(app, button, x, y, mods, k);
                }
            }
            press(app, button, x, y, mods, n);
            true
        }
        "drag" => {
            let v: Vec<u16> = arg.split(',').filter_map(|p| p.trim().parse().ok()).collect();
            let [x1, y1, x2, y2] = v[..] else { return false };
            input::handle_mouse(app, ev(K::Down(B::Left), x1, y1, KeyModifiers::NONE), 1);
            let steps = x1.abs_diff(x2).max(y1.abs_diff(y2)).max(1);
            for i in 1..=steps {
                let f = |a: u16, b: u16| (a as i32 + (b as i32 - a as i32) * i as i32 / steps as i32) as u16;
                input::handle_mouse(app, ev(K::Drag(B::Left), f(x1, x2), f(y1, y2), KeyModifiers::NONE), 1);
            }
            input::handle_mouse(app, ev(K::Up(B::Left), x2, y2, KeyModifiers::NONE), 1);
            true
        }
        "wheel" => {
            let (spec, at) = arg.split_once('@').map_or((arg, None), |(a, b)| (a, xy(b)));
            let (dir, n) = spec.split_once(':').map_or((spec, 1), |(d, n)| (d, n.parse().unwrap_or(1)));
            let kind = if dir == "up" { K::ScrollUp } else { K::ScrollDown };
            let (x, y) = at.unwrap_or((app.screen_width / 2, 10));
            for _ in 0..n {
                input::handle_mouse(app, ev(kind, x, y, KeyModifiers::NONE), 1);
            }
            true
        }
        "hover" => {
            let Some((x, y)) = xy(arg) else { return false };
            input::handle_mouse(app, ev(K::Moved, x, y, KeyModifiers::NONE), 1);
            true
        }
        _ => false,
    }
}

/// Double and triple clicks, detected here (SGR reports single presses): another left press
/// within 400 ms on the same cell ±1 counts up, to 3 (mouse.md §8).
fn count_clicks(app: &mut App, m: &event::MouseEvent) -> u8 {
    use event::{MouseButton, MouseEventKind};
    if m.kind != MouseEventKind::Down(MouseButton::Left) {
        return 1;
    }
    let n = match app.last_click {
        Some((at, x, y, n)) if at.elapsed() < Duration::from_millis(400) && x.abs_diff(m.column) <= 1 && y.abs_diff(m.row) <= 1 => (n % 3) + 1,
        _ => 1,
    };
    app.last_click = Some((Instant::now(), m.column, m.row, n));
    n
}

/// The document's inline images (attachments.md §3), drawn after the frame over the rows the
/// layout reserved. Only when what's on screen changed: old images are cleared first (a full
/// redraw for the iTerm2 protocol, a delete for kitty's), and an overlay hides them.
fn draw_images(terminal: &mut ratatui::Terminal<quiet::Quiet>, app: &mut App) -> Result<()> {
    let Some(proto) = images::proto() else { return Ok(()) };
    let covered = app.overlay.is_some() || app.prompt.is_some();
    let want: Vec<images::Place> = if covered { Vec::new() } else { app.render.image_places.clone() };
    if (want.clone(), covered) == app.images_drawn {
        return Ok(());
    }
    let had = !app.images_drawn.0.is_empty();
    if had {
        match proto {
            images::Proto::Kitty => images::clear_kitty(terminal.backend_mut().raw())?,
            images::Proto::Iterm => {
                terminal.clear()?;
                terminal.draw(|f| ui::draw_app(f, app))?;
            }
        }
    }
    images::draw(terminal.backend_mut().raw(), proto, &want)?;
    app.images_drawn = (want, covered);
    Ok(())
}

/// Text entry shows a bar cursor: writing in a document, a prompt, the palette, move, capture.
fn wants_bar(app: &App) -> bool {
    (app.doc.is_some() && app.doc_write && app.overlay.is_none())
        || app.prompt.is_some()
        || matches!(app.overlay, Some(app::Overlay::Palette { .. } | app::Overlay::Finder { .. } | app::Overlay::Move { .. } | app::Overlay::Capture { .. }))
}

fn event_loop(terminal: &mut ratatui::Terminal<quiet::Quiet>, app: &mut App) -> Result<()> {
    let mut last_poll = Instant::now();
    let mut last_keys_poll = Instant::now();
    let mut keys_watch = keys_edit::Watch::default();
    keys_watch.changed(&app.vault.paths.vault);
    // THC_TUI_TRACE=1: per-frame timings (key read → frame written), for the budgets in
    // tui-editor.md §10. Written to the cache dir as tui-trace.log; p50/p99 on exit.
    let trace = std::env::var("THC_TUI_TRACE").is_ok_and(|v| v == "1");
    let mut key_at: Option<Instant> = None;
    let mut samples: Vec<f64> = Vec::new();
    let result = (|| -> Result<()> { loop {
        // The terminal went away: save and leave before drawing into it (a draw would fail
        // first and lose the line being typed).
        if HANGUP.load(std::sync::atomic::Ordering::SeqCst) {
            app.save_doc(true);
            app.remember_caret();
            app.drain_saves(true);
            app.history_tick(false);
            app.history.save(&app.vault.paths.cache);
            return Ok(());
        }
        if let Some(path) = app.switch_to.take() {
            switch_vault(app, &path);
        }
        if last_keys_poll.elapsed() >= Duration::from_millis(500) {
            last_keys_poll = Instant::now();
            if keys_watch.changed(&app.vault.paths.vault) {
                reload_keys(app);
            }
        }
        app.drain_update();
        // What a crash now would lose, for the panic hook (recover.rs).
        recover::note(app);
        terminal.draw(|f| ui::draw_app(f, app))?;
        draw_images(terminal, app)?;
        terminal.backend_mut().cursor_bar(wants_bar(app))?;
        if let Some(t) = key_at.take() {
            samples.push(t.elapsed().as_secs_f64() * 1000.0);
        }
        app.after_frame();
        // The terminal went away (SIGHUP: its window closed) or we were asked to stop (SIGTERM):
        // leave as ⌃Q does, every line saved, instead of dying mid-line.
        if HANGUP.load(std::sync::atomic::Ordering::SeqCst) || terminal_gone() {
            app.quit = true;
        }
        if app.quit || app.reexec {
            app.save_doc(true);
            app.remember_caret();
            app.drain_saves(true);
            app.history_tick(false);
            app.history.save(&app.vault.paths.cache);
            if app.reexec {
                // One frame of "updating… · back in a moment" before the terminal is handed over.
                std::thread::sleep(Duration::from_millis(250));
            }
            return Ok(());
        }
        if let Some(on) = app.mouse_request.take() {
            if on != app.tui_prefs.mouse {
                if on {
                    mouse_on(app.tui_prefs.hover);
                } else {
                    mouse_off();
                    app.hover = None;
                }
                app.tui_prefs.mouse = on;
            }
            app.info(if on { "mouse on · :mouse off for your terminal's own selection" } else { "mouse off · :mouse on to click again" });
        }
        if let Some(id) = app.editor_request.take() {
            // $EDITOR gets the keyboard as it was before thc (no kitty flags), then thc's again.
            if app.kitty {
                pop_keys();
            }
            if app.tui_prefs.mouse {
                mouse_off();
            }
            ratatui::restore();
            let r = run_editor(app, &id);
            if app.kitty {
                push_keys();
            }
            if app.tui_prefs.mouse {
                mouse_on(app.tui_prefs.hover);
            }
            let _ = ratatui::init();
            *terminal = ratatui::Terminal::new(quiet::Quiet::new(std::io::stdout()))?;
            terminal.clear()?;
            if id == "@keys" {
                runtime_effects::dispatch(app, update::Msg::KeysEdited {
                    result: r.map(|msg| msg.unwrap_or_default()).map_err(|e| format!("{e:#}")),
                    at: Instant::now(),
                });
                keys_watch.changed(&app.vault.paths.vault);
            } else {
                match r {
                    Ok(Some(msg)) => app.confirm("edited", None, msg),
                    Ok(None) => app.info("no changes"),
                    Err(e) => app.error(format!("{e:#}")),
                }
            }
            let _ = app.reload();
            continue;
        }
        // Every queued event before the next frame: a held key or a fast typist costs one
        // frame, not one per key (jank: lag on held keys).
        // A which-key popup due soon wakes the loop for it (keymap.md §5.1).
        let wait = crate::keymap::popup_wait(app).map_or(Duration::from_millis(250), |w| w.min(Duration::from_millis(250)));
        // (Interrupted because the terminal went away: the check above, next frame.)
        let ready = match event::poll(wait) {
            Err(_) if HANGUP.load(std::sync::atomic::Ordering::SeqCst) => false,
            r => r?,
        };
        if ready {
            let mut first = true;
            while first || event::poll(Duration::ZERO)? {
                first = false;
                let ev = event::read()?;
                if trace && key_at.is_none() && matches!(ev, Event::Key(_) | Event::Paste(_)) {
                    key_at = Some(Instant::now());
                }
                match ev {
                    Event::Key(k) if k.kind == KeyEventKind::Press => {
                        input::handle_key(app, k);
                        // Keys arrive in batches: keep the crash snapshot current per key.
                        recover::note(app);
                        // Tests of the hard deadline: a loop that never comes back (THC_TEST only).
                        if k.code == ratatui::crossterm::event::KeyCode::Char('!') && std::env::var_os("THC_TEST").is_some() && std::env::var_os("THC_TEST_WEDGE").is_some() {
                            loop {
                                std::thread::sleep(Duration::from_secs(60));
                            }
                        }
                    }
                    Event::Paste(text) if app.doc.is_some() => crate::doc_keys::paste(app, &text),
                    Event::Mouse(m) => {
                        let clicks = count_clicks(app, &m);
                        input::handle_mouse(app, m, clicks);
                    }
                    Event::FocusLost => app.save_doc(true),
                    Event::FocusGained => app.check_installed(true),
                    Event::Resize(..) => {
                        app.render = ui::RenderOutput::default();
                        break; // redraw before dispatching input against geometry from the old size
                    }
                    _ => {}
                }
                if app.quit || app.reexec || app.editor_request.is_some() {
                    break;
                }
            }
        }
        app.doc_tick();
        app.drain_live();
        app.check_installed(false);
        app.check_registry();
        // With the daemon live, changes arrive as events; polling is the offline fallback.
        if last_poll.elapsed() >= Duration::from_millis(if app.daemon_live { 5000 } else { 500 }) {
            last_poll = Instant::now();
            if let Err(e) = app.poll_external() {
                app.error(format!("sync: {e:#}"));
            }
        }
    } })();
    if trace && !samples.is_empty() {
        samples.sort_by(|a, b| a.partial_cmp(b).unwrap());
        let p = |q: f64| samples[((samples.len() as f64 - 1.0) * q) as usize];
        let line = format!("{} keys · p50 {:.2} ms · p99 {:.2} ms · max {:.2} ms\n", samples.len(), p(0.5), p(0.99), p(1.0));
        let path = app.vault.paths.cache.join("tui-trace.log");
        let _ = std::fs::OpenOptions::new().create(true).append(true).open(&path).and_then(|mut f| std::io::Write::write_all(&mut f, line.as_bytes()));
        eprintln!("thc tui trace: {}", line.trim());
    }
    result
}

/// `$EDITOR` round-trip on a subtree (same format as `thc edit`). The terminal is restored first.
fn run_editor(app: &mut App, id: &str) -> Result<Option<String>> {
    if id == "@keys" {
        let result = keys_edit::edit(None);
        reload_keys(app);
        return result.map(Some);
    }
    if let Some(name) = id.strip_prefix("@view:") {
        return run_view_editor(app, name);
    }
    // `e` on About: the device config in $EDITOR (nothing to apply: thc reads it as it is).
    if id == "@config" {
        let path = thc_core::policy::config_dir().join("config.toml");
        if let Some(d) = path.parent() {
            std::fs::create_dir_all(d)?;
        }
        let editor = std::env::var("VISUAL").or_else(|_| std::env::var("EDITOR")).unwrap_or_else(|_| "vi".into());
        let status = std::process::Command::new("sh").arg("-c").arg(format!("{editor} \"$1\"")).arg("thc-edit").arg(&path).status()?;
        if !status.success() {
            anyhow::bail!("editor exited with {status}");
        }
        return Ok(Some(format!("{} saved · thc keys --conflicts checks remaps", thc_core::policy::config_path_display())));
    }
    let rendered = thc_core::edit::render(&app.vault.store, id)?;
    let path = app.vault.paths.cache.join(format!("edit-{}.md", &id[..5]));
    std::fs::write(&path, &rendered.text)?;
    let editor = std::env::var("VISUAL").or_else(|_| std::env::var("EDITOR")).unwrap_or_else(|_| "vi".into());
    let mut attempt = 0;
    loop {
        let status = std::process::Command::new("sh").arg("-c").arg(format!("{editor} \"$1\"")).arg("thc-edit").arg(&path).status()?;
        if !status.success() {
            anyhow::bail!("editor exited with {status}; nothing changed");
        }
        let edited = std::fs::read_to_string(&path)?;
        if edited == rendered.text {
            let _ = std::fs::remove_file(&path);
            return Ok(None);
        }
        let mut summary = None;
        let out = &mut summary;
        let r = app.write(|b| {
            *out = Some(thc_core::edit::apply(b, &rendered, &edited)?);
            Ok(())
        });
        match r {
            Ok(_) => {
                let _ = std::fs::remove_file(&path);
                let s = summary.unwrap_or_default();
                return Ok(Some(format!("saved: {} new, {} changed, {} moved, {} deleted", s.created, s.updated, s.moved, s.deleted)));
            }
            Err(e) if attempt < 3 => {
                attempt += 1;
                let body: String = edited.lines().filter(|l| !l.starts_with("<!-- thc error:")).collect::<Vec<_>>().join("\n");
                std::fs::write(&path, format!("<!-- thc error: {e:#} -->\n{body}\n"))?;
            }
            Err(e) => return Err(e.context(format!("edit not applied; text kept at {}", path.display()))),
        }
    }
}

fn reload_keys(app: &mut App) {
    let settings = thc_core::settings::init(Some(&app.vault.paths.vault));
    keymap::reset();
    app.pending_keys.clear();
    app.pending_since = None;
    let problems = keymap::remap_problems();
    if !problems.is_empty() {
        app.error(format!("keys: {} · :remap to fix", problems.join(" · ")));
    } else if let Some(problem) = settings.notices.first() {
        app.error(problem.clone());
    }
}

/// `e` in a view's recipe (view-explain.md §3): the view in $EDITOR, a section per line
/// (`Title · query`), saved as your version when you leave. A line that doesn't read is named,
/// and the editor opens again with the error on top (three tries, as `thc edit`).
fn run_view_editor(app: &mut App, name: &str) -> Result<Option<String>> {
    let Some(rec) = app.recipe(name) else { anyhow::bail!("no view @{name}") };
    let v = rec.view;
    let head = format!("# @{name}: a section per line, Title · query. Lines starting with # are ignored.\n# Scope: {} (* in the view changes it).\n", v.scope.clone().unwrap_or_else(|| "this vault".into()));
    let body: String = v.sections.iter().map(|s| format!("{} · {}\n", s.title, s.query)).collect();
    let text = format!("{head}{body}");
    let path = app.vault.paths.cache.join(format!("view-{name}.txt"));
    std::fs::write(&path, &text)?;
    let editor = std::env::var("VISUAL").or_else(|_| std::env::var("EDITOR")).unwrap_or_else(|_| "vi".into());
    for attempt in 0..4 {
        let status = std::process::Command::new("sh").arg("-c").arg(format!("{editor} \"$1\"")).arg("thc-view").arg(&path).status()?;
        if !status.success() {
            anyhow::bail!("editor exited with {status}; nothing changed");
        }
        let edited = std::fs::read_to_string(&path)?;
        if edited == text {
            let _ = std::fs::remove_file(&path);
            return Ok(None);
        }
        let mut sections = Vec::new();
        let mut bad: Option<String> = None;
        for (i, l) in edited.lines().enumerate() {
            let t = l.trim();
            if t.is_empty() || t.starts_with('#') {
                continue;
            }
            match t.split_once(" · ").or_else(|| t.split_once(" - ")) {
                Some((title, q)) if !title.trim().is_empty() => sections.push(thc_core::views::Section { title: title.trim().into(), query: q.trim().into() }),
                _ => {
                    bad = Some(format!("line {}: wants Title · query", i + 1));
                    break;
                }
            }
        }
        let today = app.today;
        let scope = v.scope.clone();
        let r = match bad {
            Some(b) => Err(anyhow::anyhow!(b)),
            None => app.write_in_public(|b| thc_core::views::set_sectioned(b, name, Some(scope.as_deref()), &sections, today).map(|_| ())),
        };
        match r {
            Ok(_) => {
                let _ = std::fs::remove_file(&path);
                return Ok(Some(format!("@{name} saved · {} section{}", sections.len(), if sections.len() == 1 { "" } else { "s" })));
            }
            Err(e) if attempt < 3 => {
                // The line a section's error is about, when the message names its title.
                let msg = format!("{e:#}");
                let line = sections.iter().position(|s| msg.contains(&format!("{:?}", s.title))).and_then(|k| edited.lines().enumerate().filter(|(_, l)| !l.trim().is_empty() && !l.trim().starts_with('#')).nth(k).map(|(i, _)| i + 1));
                let at = line.map(|n| format!("line {n}: ")).unwrap_or_default();
                let rest: String = edited.lines().filter(|l| !l.starts_with("# error:")).map(|l| format!("{l}\n")).collect();
                std::fs::write(&path, format!("# error: {at}{msg}\n{rest}"))?;
            }
            // The reason first (the bar is short), then where the text is.
            Err(e) => anyhow::bail!("{e:#} · view not saved, text kept at {}", path.display()),
        }
    }
    Ok(None)
}

/// Render one frame headlessly (for tests, docs and design review): replay `keys`
/// (vim-style string; `<cr>`, `<esc>`, `<tab>`, `<c-x>` supported) then dump the screen as text.
fn apply_start(app: &mut App, start: Option<&str>) {
    // A problem in [tui] / [tui.focus] (an unknown key): one line, once.
    if let Some(w) = app.tui_prefs.warnings.first().cloned() {
        app.info(w);
    }
    // A refused remap ([keys.<context>], keymap.md §8.3): the first, and how many more.
    let problems = keymap::remap_problems();
    if !problems.is_empty() {
        let n = problems.len();
        app.notice(format!("keys: {n} problem{} · thc keys --conflicts", if n == 1 { "" } else { "s" }));
    }
    // A vault setting that was ignored (vaults.md §9.3): the first, and how many more.
    let notices = thc_core::settings::current().notices.clone();
    if let Some(first) = notices.first() {
        let more = if notices.len() > 1 { format!(" · {} more", notices.len() - 1) } else { String::new() };
        app.notice(format!("{first}{more}"));
    }
    // A project .thc.toml chose the vault: say which, once, so a stray one can't hide.
    if let Some(from) = app.vault.source_note.clone() {
        let v = app.vault.origin.as_ref().map_or(&app.vault.paths.vault, |o| &o.vault);
        app.info(format!("vault {} ({from})", thc_core::vault::tilde(v)));
    }
    match start {
        Some("review") => {
            app.set_view(app::View::Log);
            if !app.review_lane {
                app.toggle_review_lane();
            }
        }
        Some("log") => app.set_view(app::View::Log),
        // `thc j [date]`: that day, in Write, caret on a fresh line at the end; `|focus` /
        // `|normal` (the flag or [tui] journal) picks the mode.
        Some(s) if s.contains('|') => {
            let (head, mode) = s.split_once('|').unwrap();
            apply_start(app, Some(head));
            app.set_focus_mode(mode == "focus");
        }
        Some(s) if s.starts_with("journal:") => match thc_core::dates::parse(&s["journal:".len()..], app.today) {
            Ok(d) => {
                app.journal_date = d.date();
                app.selected = None;
                app.set_view(app::View::Journal);
            }
            Err(e) => app.error(format!("{e:#}")),
        },
        // `thc p <page>`: the page by ID or title, in Write.
        Some(s) if s.starts_with("page:") => {
            let q = &s["page:".len()..];
            let st = &app.vault.store;
            let id = st.resolve(q).ok().or_else(|| st.find_root_by_title(q, false).ok().flatten());
            match id {
                Some(id) => {
                    app.page_open = Some(id);
                    app.prompt = None;
                    app.set_view(app::View::Pages);
                    app.prompt = None;
                }
                None => {
                    app.pages_filter = q.to_string();
                    app.set_view(app::View::Pages);
                    app.error(format!("no page \"{q}\" · Enter on + new page creates it"));
                }
            }
        }
        // Anything else (or nothing): the view as it is, loaded.
        _ => {
            let _ = app.reload();
        }
    }
}

pub fn snapshot(vault: Vault, width: u16, height: u16, keys: &str, focus: Option<&str>, start: Option<&str>) -> Result<String> {
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyEventState, KeyModifiers};
    SNAPSHOT.with(|s| s.set(true));
    let started = Instant::now();
    let mut app = if start.is_some() { App::new_deferred(vault)? } else { App::new(vault)? };
    app.daemon_live = thc_core::proto::Client::connect(app.vault.daemon_paths()).is_some();
    if let Some(id) = focus {
        app.focus_node(id);
    }
    apply_start(&mut app, start);
    about::on_start(&mut app);
    resume(&mut app);
    // Review fixture: the frame shown while an update applies (just before the re-exec).
    if std::env::var_os("THC_TUI_FAKE_APPLYING").is_some() {
        app.update_state = app::UpdateState::Downloading { version: "0.8.1".into() };
        app.reexec = true;
    }
    let mut term = Terminal::new(quiet::Snap { inner: TestBackend::new(width, height), visible: false })?;
    term.draw(|f| ui::draw_app(f, &mut app))?;
    // THC_TUI_TRACE=1: the first frame, from opening the vault (the open budget, §10).
    if std::env::var("THC_TUI_TRACE").is_ok_and(|v| v == "1" || v == "2") {
        eprintln!("thc tui trace: first frame {:.1} ms", started.elapsed().as_secs_f64() * 1000.0);
    }
    let mut rest = keys;
    let mut key_ms: Vec<f64> = Vec::new();
    while !rest.is_empty() {
        // A paste token's text may hold `>`: it ends at a `>` followed by `<` or the end.
        let token_end = |r: &str| -> Option<usize> {
            if r.starts_with("paste:") {
                let b = r.as_bytes();
                (0..b.len()).find(|&i| b[i] == b'>' && (i + 1 == b.len() || b[i + 1] == b'<'))
            } else {
                r.find('>')
            }
        };
        let (code, mods, used) = if let Some(end) = rest.strip_prefix('<').and_then(token_end) {
            let tok = &rest[1..end + 1];
            let (code, mods) = match tok {
                "cr" => (KeyCode::Enter, KeyModifiers::NONE),
                "f1" => (KeyCode::F(1), KeyModifiers::NONE),
                "esc" => (KeyCode::Esc, KeyModifiers::NONE),
                "tab" => (KeyCode::Tab, KeyModifiers::NONE),
                "space" => (KeyCode::Char(' '), KeyModifiers::NONE),
                "bs" => (KeyCode::Backspace, KeyModifiers::NONE),
                "up" => (KeyCode::Up, KeyModifiers::NONE),
                "down" => (KeyCode::Down, KeyModifiers::NONE),
                "left" => (KeyCode::Left, KeyModifiers::NONE),
                "right" => (KeyCode::Right, KeyModifiers::NONE),
                "s-tab" => (KeyCode::BackTab, KeyModifiers::SHIFT),
                "s-left" => (KeyCode::Left, KeyModifiers::SHIFT),
                "s-right" => (KeyCode::Right, KeyModifiers::SHIFT),
                "s-up" => (KeyCode::Up, KeyModifiers::SHIFT),
                "s-down" => (KeyCode::Down, KeyModifiers::SHIFT),
                "m-up" => (KeyCode::Up, KeyModifiers::ALT),
                "m-down" => (KeyCode::Down, KeyModifiers::ALT),
                "m-left" => (KeyCode::Left, KeyModifiers::ALT),
                "m-right" => (KeyCode::Right, KeyModifiers::ALT),
                "s-cr" => (KeyCode::Enter, KeyModifiers::SHIFT),
                "pgdn" => (KeyCode::PageDown, KeyModifiers::NONE),
                "pgup" => (KeyCode::PageUp, KeyModifiers::NONE),
                "home" => (KeyCode::Home, KeyModifiers::NONE),
                "end" => (KeyCode::End, KeyModifiers::NONE),
                "del" => (KeyCode::Delete, KeyModifiers::NONE),
                t if t.starts_with("paste:") => {
                    // A bracketed paste; `\n` in the token is a line break.
                    let text = t["paste:".len()..].replace("\\n", "\n");
                    let t0 = Instant::now();
                    crate::doc_keys::paste(&mut app, &text);
                    // THC_TUI_TRACE=1: a paste's time, its save included (the paste budget).
                    if std::env::var("THC_TUI_TRACE").is_ok_and(|v| v == "1" || v == "2") {
                        app.drain_saves(true);
                        term.draw(|f| ui::draw_app(f, &mut app))?;
                        eprintln!("thc tui trace: paste {} lines {:.1} ms", text.lines().count(), t0.elapsed().as_secs_f64() * 1000.0);
                    }
                    (KeyCode::Null, KeyModifiers::NONE)
                }
                t if t.starts_with("remote:") => {
                    // Fixture: another device changes a line's text (ID:TEXT), then the poll sees it.
                    if let Some((id, text)) = t["remote:".len()..].split_once(':') {
                        remote_write(&app, id, text)?;
                        let _ = app.poll_external();
                    }
                    (KeyCode::Null, KeyModifiers::NONE)
                }
                // The mouse (mouse.md §9): <click:x,y> <dclick:x,y> <tclick:x,y> <sclick:x,y>
                // <cclick:x,y> <mclick:x,y> <drag:x1,y1,x2,y2> <wheel:up|down[:n][@x,y]> <hover:x,y>.
                t if mouse_token(&mut app, t) => (KeyCode::Null, KeyModifiers::NONE),
                t if t.starts_with("m-") => (KeyCode::Char(t.chars().nth(2).unwrap_or(' ')), KeyModifiers::ALT),
                "alert" => {
                    // Fixture: the first pending alert fires as if the daemon pushed it.
                    app.simulate_alert();
                    (KeyCode::Null, KeyModifiers::NONE)
                }
                t if t.starts_with("agent:") => {
                    // Fixture: an agent (on its own device) adds a node to today's journal,
                    // then the TUI polls the log exactly as it does live.
                    agent_write(&app, &t["agent:".len()..])?;
                    let _ = app.poll_external();
                    (KeyCode::Null, KeyModifiers::NONE)
                }
                "c-cr" => (KeyCode::Enter, KeyModifiers::CONTROL),
                "c-up" => (KeyCode::Up, KeyModifiers::CONTROL),
                "c-down" => (KeyCode::Down, KeyModifiers::CONTROL),
                "c-left" => (KeyCode::Left, KeyModifiers::CONTROL),
                "c-m-left" => (KeyCode::Left, KeyModifiers::CONTROL | KeyModifiers::ALT),
                "c-m-right" => (KeyCode::Right, KeyModifiers::CONTROL | KeyModifiers::ALT),
                "d-up" => (KeyCode::Up, KeyModifiers::SUPER),
                "d-down" => (KeyCode::Down, KeyModifiers::SUPER),
                "c-right" => (KeyCode::Right, KeyModifiers::CONTROL),
                "c-home" => (KeyCode::Home, KeyModifiers::CONTROL),
                "c-end" => (KeyCode::End, KeyModifiers::CONTROL),
                // ⌘ (super), as the kitty protocol delivers it.
                t if t.starts_with("d-") => (KeyCode::Char(t.chars().nth(2).unwrap_or(' ')), KeyModifiers::SUPER),
                t if t.starts_with("c-") => (KeyCode::Char(t.chars().nth(2).unwrap_or(' ')), KeyModifiers::CONTROL),
                _ => (KeyCode::Null, KeyModifiers::NONE),
            };
            (code, mods, end + 2)
        } else {
            let c = rest.chars().next().unwrap();
            (KeyCode::Char(c), KeyModifiers::NONE, c.len_utf8())
        };
        let t0 = Instant::now();
        if code != KeyCode::Null {
            input::handle_key(&mut app, KeyEvent { code, modifiers: mods, kind: KeyEventKind::Press, state: KeyEventState::NONE });
        }
        if let Some(id) = app.editor_request.take() {
            // Tests of the view editor run their own (non-interactive) $VISUAL.
            if id.starts_with("@view:") && std::env::var_os("THC_TUI_SNAPSHOT_EDITOR").is_some() {
                match run_editor(&mut app, &id) {
                    Ok(Some(msg)) => app.info(msg),
                    Ok(None) => app.info("no changes"),
                    Err(e) => app.error(format!("{e:#}")),
                }
                let _ = app.reload();
            } else {
                app.info("($EDITOR skipped in snapshot)");
            }
        }
        if let Some(path) = app.switch_to.take() {
            let t = Instant::now();
            switch_vault(&mut app, &path);
            if std::env::var("THC_TUI_TRACE").is_ok_and(|v| v == "2") {
                eprintln!("switch: {:.1} ms", t.elapsed().as_secs_f64() * 1e3);
            }
        }
        if let Some(on) = app.mouse_request.take() {
            app.tui_prefs.mouse = on;
            app.info(if on { "mouse on · :mouse off for your terminal's own selection" } else { "mouse off · :mouse on to click again" });
        }
        term.draw(|f| ui::draw_app(f, &mut app))?;
        let ms_key = t0.elapsed().as_secs_f64() * 1000.0;
        app.after_frame();
        if code != KeyCode::Null {
            let ms = ms_key;
            key_ms.push(ms);
            if std::env::var("THC_TUI_TRACE").is_ok_and(|v| v == "2") {
                eprintln!("{ms:8.2} ms  {code:?} {mods:?}");
            }
        }
        rest = &rest[used..];
    }
    // A snapshot asked to keep caret memory leaves it as quitting does (tests of reopening).
    if std::env::var_os("THC_TUI_SNAPSHOT_CARETS").is_some() {
        app.remember_caret();
    }
    // Snapshots that save through the daemon (tests): everything lands before the frame is read.
    if std::env::var_os("THC_TUI_SNAPSHOT_DAEMON").is_some() {
        app.save_doc(true);
        app.drain_saves(true);
        term.draw(|f| ui::draw_app(f, &mut app))?;
    }
    // THC_TUI_TRACE=1: each replayed key's handle + frame time, to stderr (the §10 budgets).
    if std::env::var("THC_TUI_TRACE").is_ok_and(|v| v == "1" || v == "2") && !key_ms.is_empty() {
        let mut v = key_ms.clone();
        v.sort_by(|a, b| a.partial_cmp(b).unwrap());
        let p = |q: f64| v[((v.len() as f64 - 1.0) * q) as usize];
        eprintln!("thc tui trace: {} keys · p50 {:.2} ms · p99 {:.2} ms · max {:.2} ms", v.len(), p(0.5), p(0.99), p(1.0));
    }
    // A save waits for the frame after a key (as live): draw once more after it, so a snapshot
    // shows what a person sees a moment later (a line just left has its tokens folded).
    term.draw(|f| ui::draw_app(f, &mut app))?;
    let mut buf = term.backend().inner.buffer().clone();
    // THC_TUI_SNAPSHOT_CURSOR=1: the terminal cursor's cell drawn as `▮` (caret placement tests).
    if std::env::var("THC_TUI_SNAPSHOT_CURSOR").is_ok_and(|v| v == "1") {
        use ratatui::backend::Backend;
        let Ok(p) = term.backend_mut().get_cursor_position();
        if term.backend().visible && p.x < buf.area.width && p.y < buf.area.height {
            buf[(p.x, p.y)].set_symbol("▮");
        }
    }
    let format = std::env::var("THC_TUI_SNAPSHOT_FORMAT").unwrap_or_default();
    match format.as_str() {
        "ansi" => return Ok(snapshot_fmt::ansi(&buf)),
        "html" => return Ok(snapshot_fmt::html(&buf, &app.theme)),
        _ => {}
    }
    let mut out = String::new();
    for y in 0..buf.area.height {
        let mut line = String::new();
        let mut skip = 0;
        for x in 0..buf.area.width {
            if skip > 0 {
                skip -= 1;
                continue;
            }
            let sym = buf[(x, y)].symbol();
            skip = unicode_width::UnicodeWidthStr::width(sym).saturating_sub(1);
            line.push_str(sym);
        }
        out.push_str(line.trim_end());
        out.push('\n');
    }
    Ok(out)
}

fn agent_write(app: &App, text: &str) -> Result<()> {
    use thc_core::event::Actor;
    let mut paths = app.vault.paths.clone();
    paths.cache = paths.cache.join("fixture-agent");
    let mut v = Vault::open(paths, Actor { kind: "agent".into(), name: Some("claude".into()) }, "cli")?;
    let today = thc_core::dates::today();
    let cap = thc_core::capture::parse(text, today)?;
    v.transact(|s| {
        let mut b = thc_core::builder::TxBuilder::new(s, today);
        let j = b.journal(today)?;
        b.create_from_capture(Some(j), &cap, None)?;
        Ok((b.finish(), ()))
    })?;
    Ok(())
}

/// Snapshot fixture: a text change to a node from another device (`<remote:ID:TEXT>`).
fn remote_write(app: &App, id_prefix: &str, text: &str) -> Result<()> {
    use thc_core::builder::TxBuilder;
    use thc_core::event::Actor;
    let mut paths = app.vault.paths.clone();
    paths.cache = paths.cache.join("fixture-remote");
    let mut v = Vault::open(paths, Actor { kind: "agent".into(), name: Some("claude".into()) }, "cli")?;
    let id = v.store.resolve(id_prefix)?;
    let today = thc_core::dates::today();
    v.transact(|s| {
        let mut b = TxBuilder::new(s, today);
        b.set_text(&id, text)?;
        Ok((b.finish(), ()))
    })?;
    Ok(())
}
