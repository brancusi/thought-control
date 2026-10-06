//! `[tui]` in ~/.config/thought/config.toml: how the terminal UI opens and behaves. Every
//! setting has a built-in default, so a missing key (or file) behaves as documented; `thc setup`
//! writes the commented blocks below so all of them are visible. A command-line flag
//! (`thc j --focus` / `--no-focus`) beats the file, and THC_TUI_* variables beat both.
//!
//! `[tui.focus]` (tui-editor.md §8) says what Focus shows: a preset, then per-element overrides.

/// The `[tui]` block `thc setup` adds to config.toml when it has none.
pub const TUI_BLOCK: &str = r#"
[tui]
# How `thc j` opens a journal day: "focus" (just your writing) or "normal".
journal = "focus"
# How `thc p` opens a page: "focus" or "normal".
pages = "normal"
# Keep the line you're on at 45% of the screen outside focus.
typewriter = false
# The mouse: click to place the cursor, drag to select, the wheel scrolls (false: your terminal's
# own selection, with no capture at all).
mouse = true
# Hover highlights: "auto" (on here, off over SSH), "on" or "off".
hover = "auto"
# Rows the wheel scrolls per step.
wheel_rows = 1
"#;

/// The `[tui.focus]` block `thc setup` adds when there's none (`:focus` in the TUI changes it).
pub const FOCUS_BLOCK: &str = r#"
[tui.focus]
# What focus shows: "bare" (only your text), "writer" (adds the date, the day strip, the keys
# footer and a word count) or "planner" (adds dates and priorities, a month, today's tasks,
# links and a clock).
preset = "writer"
# The text column's width in focus (50-100). Unset, it's 72 (64 for planner, so the month fits).
# width = 72
# Then turn single elements on or off on top of the preset (remove the # to use one):
# footer = true        # the keys footer
# header = true        # the date or the page title
# strip = true         # the 7-day strip
# month = false        # a month calendar beside the text (110+ columns wide)
# meta = false         # dates, priority and repeat on the right
# also_today = false   # "also today" under a journal day
# linked_from = false  # "linked from" under a page
# tabs = false         # the view tabs at the top
# wordcount = true     # the word count
# clock = false        # the time
# dim = false          # dim every line but the one you're on
# typewriter = false   # keep the line you're on at 45% of the screen's height
"#;

/// The blocks a config file is missing, to append (empty when it has both).
pub fn missing_blocks(have: &str) -> String {
    let has = |h: &str| have.lines().any(|l| l.trim() == h);
    let mut out = String::new();
    if !has("[tui]") {
        out.push_str(TUI_BLOCK);
    }
    if !has("[tui.focus]") {
        out.push_str(FOCUS_BLOCK);
    }
    out
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    Focus,
    Normal,
}

/// One thing Focus can show (tui-editor.md §8.1).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum El {
    Footer,
    Header,
    Strip,
    Month,
    Meta,
    AlsoToday,
    LinkedFrom,
    Tabs,
    Wordcount,
    Clock,
    Dim,
    Typewriter,
    /// The page or day rail beside the document, or the crumb when it doesn't fit (navigation.md §3).
    Nav,
}

/// Every element: its config key, its letter in the `:focus` overlay and its label there.
pub const ELEMENTS: [(El, &str, char, &str); 13] = [
    (El::Footer, "footer", 'f', "footer"),
    (El::Header, "header", 'h', "header"),
    (El::Strip, "strip", 's', "day strip"),
    (El::Month, "month", 'c', "month"),
    (El::Meta, "meta", 'm', "meta"),
    (El::AlsoToday, "also_today", 'a', "also today"),
    (El::LinkedFrom, "linked_from", 'l', "linked from"),
    (El::Tabs, "tabs", 't', "tabs"),
    (El::Wordcount, "wordcount", 'w', "word count"),
    (El::Clock, "clock", 'k', "clock"),
    (El::Dim, "dim", 'd', "dim others"),
    (El::Typewriter, "typewriter", 'y', "typewriter"),
    (El::Nav, "nav", 'n', "page rail"),
];

impl El {
    fn bit(self) -> u16 {
        1 << ELEMENTS.iter().position(|(e, ..)| *e == self).unwrap()
    }

    pub fn key(self) -> &'static str {
        ELEMENTS.iter().find(|(e, ..)| *e == self).unwrap().1
    }

    pub fn from_key(k: &str) -> Option<El> {
        ELEMENTS.iter().find(|(_, key, ..)| *key == k).map(|(e, ..)| *e)
    }

    pub fn from_letter(c: char) -> Option<El> {
        ELEMENTS.iter().find(|(_, _, l, _)| *l == c).map(|(e, ..)| *e)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Preset {
    Bare,
    Writer,
    Planner,
}

pub const PRESETS: [Preset; 3] = [Preset::Bare, Preset::Writer, Preset::Planner];

impl Preset {
    pub fn name(self) -> &'static str {
        match self {
            Preset::Bare => "bare",
            Preset::Writer => "writer",
            Preset::Planner => "planner",
        }
    }

    pub fn parse(s: &str) -> Option<Preset> {
        PRESETS.into_iter().find(|p| p.name() == s)
    }

    /// The text column's width when `[tui.focus] width` doesn't say: planner is narrower, so
    /// the meta and the month fit beside it at 120 columns (6 + 64 + 2 + 24 + 22 = 118).
    pub fn width(self) -> u16 {
        match self {
            Preset::Planner => 64,
            _ => FOCUS_WIDTH,
        }
    }

    pub fn set(self) -> FocusSet {
        use El::*;
        let on: &[El] = match self {
            Preset::Bare => &[],
            Preset::Writer => &[Header, Strip, Footer, Wordcount],
            Preset::Planner => &[Header, Strip, Footer, Meta, Month, AlsoToday, LinkedFrom, Clock],
        };
        on.iter().fold(FocusSet::default(), |s, e| s.with(*e, true))
    }
}

/// Which elements are on.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct FocusSet(u16);

impl FocusSet {
    pub fn has(self, e: El) -> bool {
        self.0 & e.bit() != 0
    }

    pub fn with(self, e: El, on: bool) -> FocusSet {
        FocusSet(if on { self.0 | e.bit() } else { self.0 & !e.bit() })
    }
}

/// What Focus shows: a preset, the elements as they stand (preset plus overrides) and a
/// text column width when one was set (else the preset's).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Focus {
    pub preset: Preset,
    pub set: FocusSet,
    pub custom_width: Option<u16>,
}

pub const FOCUS_WIDTH: u16 = 72;

impl Default for Focus {
    fn default() -> Self {
        Focus { preset: Preset::Writer, set: Preset::Writer.set(), custom_width: None }
    }
}

impl Focus {
    pub fn has(&self, e: El) -> bool {
        self.set.has(e)
    }

    /// The text column's width: `[tui.focus] width`, else the preset's.
    pub fn width(&self) -> u16 {
        self.custom_width.unwrap_or(self.preset.width())
    }

    pub fn toggle(&mut self, e: El) {
        self.set = self.set.with(e, !self.set.has(e));
    }

    pub fn use_preset(&mut self, p: Preset) {
        self.preset = p;
        self.set = p.set();
    }

    /// The preset's name, or `custom` once anything differs from it.
    pub fn label(&self) -> &'static str {
        if self.set == self.preset.set() { self.preset.name() } else { "custom" }
    }

    /// The elements that differ from the preset, as (key, on).
    pub fn diffs(&self) -> Vec<(&'static str, bool)> {
        let base = self.preset.set();
        ELEMENTS.iter().filter(|(e, ..)| base.has(*e) != self.set.has(*e)).map(|(e, k, ..)| (*k, self.set.has(*e))).collect()
    }

    /// `planner,+clock,-footer` / `writer` / `+month -footer` (THC_TUI_FOCUS and `:focus`).
    pub fn apply(&mut self, spec: &str) -> Result<(), String> {
        for tok in spec.split([',', ' ']).map(str::trim).filter(|t| !t.is_empty()) {
            if let Some(p) = Preset::parse(tok) {
                self.use_preset(p);
                continue;
            }
            let (on, name) = match tok.as_bytes()[0] {
                b'+' => (true, &tok[1..]),
                b'-' => (false, &tok[1..]),
                _ => (true, tok),
            };
            match El::from_key(name) {
                Some(e) => self.set = self.set.with(e, on),
                None => {
                    let hint = suggest(name, PRESETS.iter().map(|p| p.name()).chain(ELEMENTS.iter().map(|x| x.1)));
                    return Err(format!("focus: unknown \"{name}\"{}", hint.map(|h| format!(" · did you mean {h}?")).unwrap_or_default()));
                }
            }
        }
        Ok(())
    }

    /// Read `[tui.focus]`, with `[tui] focus_dim` / `focus_typewriter` as aliases (the
    /// section wins). Unknown keys and values become warnings.
    fn from_tables(tui: Option<&toml::Table>, warnings: &mut Vec<String>) -> Focus {
        let mut f = Focus::default();
        let sec = tui.and_then(|t| t.get("focus")).and_then(|v| v.as_table());
        if let Some(p) = sec.and_then(|s| s.get("preset")) {
            match p.as_str().and_then(Preset::parse) {
                Some(p) => f.use_preset(p),
                None => warnings.push(format!("[tui.focus] preset {p} isn't bare, writer or planner · using writer")),
            }
        }
        for (alias, e) in [("focus_dim", El::Dim), ("focus_typewriter", El::Typewriter)] {
            if let Some(b) = tui.and_then(|t| t.get(alias)).and_then(|v| v.as_bool()) {
                f.set = f.set.with(e, b);
            }
        }
        for (k, v) in sec.into_iter().flatten() {
            match (k.as_str(), El::from_key(k)) {
                ("preset", _) => {}
                ("width", _) => match v.as_integer() {
                    Some(w) => f.custom_width = Some(w.clamp(50, 100) as u16),
                    None => warnings.push("[tui.focus] width should be a number (50-100)".into()),
                },
                (_, Some(e)) => match v.as_bool() {
                    Some(b) => f.set = f.set.with(e, b),
                    None => warnings.push(format!("[tui.focus] {k} should be true or false")),
                },
                (_, None) => {
                    let hint = suggest(k, ELEMENTS.iter().map(|x| x.1).chain(["preset", "width"]));
                    warnings.push(format!("[tui.focus] unknown key \"{k}\"{}", hint.map(|h| format!(" · did you mean {h}?")).unwrap_or_default()));
                }
            }
        }
        f
    }
}

/// The closest name within two edits, for "did you mean".
fn suggest<'a>(typed: &str, names: impl Iterator<Item = &'a str>) -> Option<&'a str> {
    names.map(|n| (strsim(typed, n), n)).filter(|(d, _)| *d <= 2).min_by_key(|(d, _)| *d).map(|(_, n)| n)
}

/// Levenshtein distance (short strings only).
fn strsim(a: &str, b: &str) -> usize {
    let b: Vec<char> = b.chars().collect();
    let mut prev: Vec<usize> = (0..=b.len()).collect();
    for (i, ca) in a.chars().enumerate() {
        let mut cur = vec![i + 1];
        for (j, cb) in b.iter().enumerate() {
            cur.push((prev[j] + (ca != *cb) as usize).min(prev[j + 1] + 1).min(cur[j] + 1));
        }
        prev = cur;
    }
    prev[b.len()]
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LeaderPopup {
    Immediate,
    Delay,
    Off,
}

#[derive(Clone, Debug)]
pub struct TuiConfig {
    pub journal: Mode,
    pub pages: Mode,
    /// Capture the mouse (mouse.md §2).
    pub mouse: bool,
    /// Hover (1003 all-motion reporting): resolved from "auto" | "on" | "off".
    pub hover: bool,
    pub wheel_rows: usize,
    /// Typewriter scrolling outside focus (in focus it's the `typewriter` element).
    pub typewriter: bool,
    pub focus: Focus,
    /// Which-key (keymap.md §5.1): the prefix popup's delay in ms (`g` `p` `S`), -1 = off.
    pub which_key_ms: i64,
    /// The leader's popup: shown at once, after `which_key_ms`, or never (the footer breadcrumb
    /// always shows).
    pub leader_popup: LeaderPopup,
    /// The page/day rail outside Focus at 120 columns or more (navigation.md §3).
    pub page_rail: bool,
    /// One line per problem in the file (an unknown key, a bad value), for the TUI to show.
    pub warnings: Vec<String>,
}

impl Default for TuiConfig {
    fn default() -> Self {
        TuiConfig { journal: Mode::Focus, pages: Mode::Normal, typewriter: false, mouse: true, hover: std::env::var_os("SSH_CONNECTION").is_none(), wheel_rows: 1, focus: Focus::default(), which_key_ms: 200, leader_popup: LeaderPopup::Immediate, page_rail: true, warnings: vec![] }
    }
}

impl TuiConfig {
    pub fn load() -> TuiConfig {
        // Your config with the vault's settings layered on (vaults.md §9).
        let eff = crate::settings::current();
        Self::from_table(eff.table.get("tui").and_then(|x| x.as_table()))
    }

    pub fn from_table(t: Option<&toml::Table>) -> TuiConfig {
        let d = TuiConfig::default();
        let mode = |k: &str, def: Mode| match t.and_then(|t| t.get(k)).and_then(|v| v.as_str()) {
            Some("focus") => Mode::Focus,
            Some("normal") => Mode::Normal,
            _ => def,
        };
        let env_flag = |env: &str| std::env::var(env).ok().map(|v| v == "1");
        let mut warnings = vec![];
        let mut focus = Focus::from_tables(t, &mut warnings);
        // THC_TUI_* beat the file, for one run.
        if let Some(b) = env_flag("THC_TUI_FOCUS_DIM") {
            focus.set = focus.set.with(El::Dim, b);
        }
        if let Some(b) = env_flag("THC_TUI_FOCUS_TYPEWRITER") {
            focus.set = focus.set.with(El::Typewriter, b);
        }
        if let Ok(spec) = std::env::var("THC_TUI_FOCUS") {
            if let Err(e) = focus.apply(&spec) {
                warnings.push(format!("THC_TUI_FOCUS: {e}"));
            }
        }
        // hover = auto is on locally, off over SSH (all-motion reports cost a lot there).
        let hover = match t.and_then(|t| t.get("hover")).and_then(|v| v.as_str()) {
            Some("on") => true,
            Some("off") => false,
            _ => d.hover,
        };
        TuiConfig {
            journal: mode("journal", d.journal),
            pages: mode("pages", d.pages),
            mouse: env_flag("THC_TUI_MOUSE").or_else(|| t.and_then(|t| t.get("mouse")).and_then(|v| v.as_bool())).unwrap_or(d.mouse),
            hover,
            wheel_rows: t.and_then(|t| t.get("wheel_rows")).and_then(|v| v.as_integer()).map_or(d.wheel_rows, |n| n.clamp(1, 10) as usize),
            typewriter: env_flag("THC_TUI_TYPEWRITER").or_else(|| t.and_then(|t| t.get("typewriter")).and_then(|v| v.as_bool())).unwrap_or(d.typewriter),
            focus,
            which_key_ms: t.and_then(|t| t.get("which_key_ms")).and_then(|v| v.as_integer()).map_or(d.which_key_ms, |n| n.clamp(-1, 5000)),
            leader_popup: match std::env::var("THC_TUI_LEADER_POPUP").ok().or_else(|| t.and_then(|t| t.get("leader_popup")).and_then(|v| v.as_str()).map(str::to_string)).as_deref() {
                Some("delay") => LeaderPopup::Delay,
                Some("off") => LeaderPopup::Off,
                _ => d.leader_popup,
            },
            page_rail: env_flag("THC_TUI_PAGE_RAIL").or_else(|| t.and_then(|t| t.get("page_rail")).and_then(|v| v.as_bool())).unwrap_or(d.page_rail),
            warnings,
        }
    }
}

/// Save Focus to `[tui.focus]` in the config at `path`: `preset`, `width` when it isn't the
/// default, and only the elements that differ from the preset. The rest of the file (other
/// sections, their comments) is left as it was; the old `[tui.focus]` body is replaced.
pub fn save_focus(path: &std::path::Path, f: &Focus) -> std::io::Result<()> {
    let have = std::fs::read_to_string(path).unwrap_or_default();
    let mut body = format!("[tui.focus]\n# Saved by :focus in the TUI. Presets: bare, writer, planner.\npreset = \"{}\"\n", f.preset.name());
    if let Some(w) = f.custom_width {
        body.push_str(&format!("width = {w}\n"));
    }
    for (k, on) in f.diffs() {
        body.push_str(&format!("{k} = {on}\n"));
    }
    let lines: Vec<&str> = have.lines().collect();
    let out = match lines.iter().position(|l| l.trim() == "[tui.focus]") {
        Some(start) => {
            // The section runs to the next table header; comments just above that header
            // belong to it, so they stay.
            let mut end = lines[start + 1..].iter().position(|l| l.trim_start().starts_with('[')).map_or(lines.len(), |i| start + 1 + i);
            while end > start + 1 && (lines[end - 1].trim().is_empty() || lines[end - 1].trim_start().starts_with('#')) && end < lines.len() {
                end -= 1;
            }
            let mut s = lines[..start].join("\n");
            if !s.is_empty() {
                s.push('\n');
            }
            s.push_str(&body);
            if end < lines.len() {
                s.push('\n');
                s.push_str(&lines[end..].join("\n"));
                s.push('\n');
            }
            s
        }
        None => {
            let sep = if have.is_empty() || have.ends_with('\n') { "" } else { "\n" };
            format!("{have}{sep}\n{body}")
        }
    };
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    std::fs::write(path, out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg(s: &str) -> TuiConfig {
        let t: toml::Table = s.parse().unwrap();
        TuiConfig::from_table(t.get("tui").and_then(|x| x.as_table()))
    }

    #[test]
    fn the_default_blocks_are_the_defaults() {
        let c = cfg(&missing_blocks(""));
        let d = TuiConfig::default();
        assert_eq!((c.journal, c.pages, c.typewriter, c.focus, c.mouse, c.wheel_rows), (d.journal, d.pages, d.typewriter, d.focus, d.mouse, d.wheel_rows));
        assert!(c.warnings.is_empty(), "{:?}", c.warnings);
        assert_eq!(c.focus.label(), "writer");
        assert!(missing_blocks(&missing_blocks("")).is_empty());
    }

    #[test]
    fn values_override_and_bad_ones_fall_back() {
        let c = cfg("[tui]\njournal = \"normal\"\npages = \"bogus\"\nfocus_dim = true");
        assert_eq!(c.journal, Mode::Normal);
        assert_eq!(c.pages, Mode::Normal, "an unknown value keeps the default");
        assert!(c.focus.has(El::Dim), "focus_dim is an alias for dim");
    }

    #[test]
    fn a_preset_then_overrides() {
        let c = cfg("[tui]\nfocus_typewriter = true\n[tui.focus]\npreset = \"planner\"\nclock = false\nwordcount = true\ntypewriter = false\nwidth = 200\ncalender = true");
        assert!(c.focus.has(El::Month) && c.focus.has(El::Meta) && c.focus.has(El::Wordcount));
        assert!(!c.focus.has(El::Clock));
        assert!(!c.focus.has(El::Typewriter), "the section beats the alias");
        assert_eq!(c.focus.width(), 100, "a width in the file beats the preset's");
        assert_eq!(cfg("[tui.focus]\npreset = \"planner\"").focus.width(), 64);
        assert_eq!(c.focus.label(), "custom");
        assert_eq!(c.warnings, vec!["[tui.focus] unknown key \"calender\""]);
        let c = cfg("[tui.focus]\nfooterr = false");
        assert_eq!(c.warnings, vec!["[tui.focus] unknown key \"footerr\" · did you mean footer?"]);
    }

    #[test]
    fn the_focus_grammar() {
        let mut f = Focus::default();
        f.apply("planner,+wordcount,-footer").unwrap();
        assert_eq!(f.preset, Preset::Planner);
        assert!(f.has(El::Wordcount) && !f.has(El::Footer));
        assert_eq!(f.diffs(), vec![("footer", false), ("wordcount", true)]);
        f.apply("bare").unwrap();
        assert_eq!(f.label(), "bare");
        assert_eq!(f.apply("+mont").unwrap_err(), "focus: unknown \"mont\" · did you mean month?");
    }

    #[test]
    fn saving_writes_only_the_differences() {
        let dir = std::env::temp_dir().join(format!("thc-focus-save-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let p = dir.join("config.toml");
        let start = format!("vault = \"~/thought\"\n{}{}\n[other]\nx = 1\n", TUI_BLOCK, FOCUS_BLOCK);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(&p, &start).unwrap();
        let mut f = Focus::default();
        f.apply("planner,-clock").unwrap();
        save_focus(&p, &f).unwrap();
        let s = std::fs::read_to_string(&p).unwrap();
        assert!(s.contains("preset = \"planner\"\nclock = false\n"), "{s}");
        assert!(s.contains("journal = \"focus\"") && s.contains("[other]\nx = 1"), "the rest stays: {s}");
        let back = cfg(&s);
        assert_eq!(back.focus, f);
        // No section yet: it's appended.
        std::fs::write(&p, "vault = \"~/thought\"\n").unwrap();
        save_focus(&p, &Focus::default()).unwrap();
        assert_eq!(cfg(&std::fs::read_to_string(&p).unwrap()).focus, Focus::default());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
