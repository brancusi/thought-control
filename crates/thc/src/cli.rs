use crate::team_cmd::TeamCmd;
use clap::{Args, Parser, Subcommand};
use std::path::PathBuf;

/// thc: thought control. A CLI-first, local-first stash for notes, todos and dates.
///
/// Data lives in an append-only event log (the source of truth); a local SQLite view makes
/// queries fast. Every command supports --json for agents.
#[derive(Parser, Debug)]
#[command(name = "thc", version, about, long_about = None, after_help = AFTER_HELP)]
pub struct Cli {
    /// Machine-readable JSON output (and JSON errors on stderr).
    #[arg(long, global = true)]
    pub json: bool,

    /// Vault directory (default: nearest .thc.toml, then ~/.config/thought/config.toml).
    #[arg(long, global = true, env = "THC_VAULT", value_name = "DIR")]
    pub vault: Option<PathBuf>,

    /// Project work board, independent of the capture vault: <vault>[:¶ Page].
    #[arg(long, global = true, value_name = "BOARD")]
    pub board: Option<String>,

    /// Who is acting: "human", or an agent name like "claude" (recorded on every write).
    #[arg(long, global = true, env = "THC_ACTOR", value_name = "NAME")]
    pub actor: Option<String>,

    /// Print the ops a write would emit, without writing.
    #[arg(long, global = true)]
    pub dry_run: bool,

    /// Maximum items in listings.
    #[arg(long, global = true, default_value_t = 50)]
    pub limit: usize,

    /// Only these node fields in JSON output (implies --json). With no value, list the fields.
    #[arg(long, global = true, value_name = "a,b,c", num_args = 0..=1, default_missing_value = "")]
    pub fields: Option<String>,

    /// Write only if the node is unchanged since this rev (every node's JSON has `rev`); else exit 4.
    #[arg(long, global = true, value_name = "REV")]
    pub if_match: Option<String>,

    /// Write only if the node's field has this value, e.g. status=todo (repeatable); else exit 4.
    #[arg(long, global = true, value_name = "FIELD=VALUE")]
    pub expect: Vec<String>,

    /// Apply this context to this command (`none` turns one off). JSON and agents only get a
    /// context when they ask: this flag or THC_CONTEXT.
    #[arg(long, global = true, value_name = "NAME")]
    pub context: Option<String>,
    /// Refuse every write in this process, whoever the actor is (also THC_READONLY=1).
    #[arg(long, global = true)]
    pub readonly: bool,
    /// Confirm: skip prompts, and run a verb your policy marks `confirm`. For an agent it
    /// means "I asked the human and they said yes".
    #[arg(long, short = 'y', global = true)]
    pub yes: bool,

    #[command(subcommand)]
    pub cmd: Option<Cmd>,
}

const AFTER_HELP: &str = "\
Capture syntax (add/todo/remind): due:fri  sched:mon  at:\"tue 2pm\"  every:2w  !high  #tag  [[Page]]
Query syntax (q):  status:open  due<=+3d  #work  -#someday  is:overdue  under:<id>  sort:due  (a or b)
Exit codes: 0 ok, 1 error, 2 usage, 3 not found, 4 conflict, 5 ambiguous id, 6 validation";

#[derive(Subcommand, Debug)]
pub enum Cmd {
    /// A command from the registry (registry.rs): main runs it through its Spec.
    #[command(skip)]
    Registered,
    /// Session briefing for an agent: current state, then the rules (~15 lines).
    Prime {
        /// Brief this team role on its board, card and next tasks.
        #[arg(long)]
        role: Option<String>,
    },
    /// Start, inspect, or stop this project's team.
    #[command(subcommand)]
    Team(TeamCmd),
    /// Move a node.
    Mv(MvArgs),
    /// Edit a node and its children in $EDITOR (default: today's journal).
    Edit { id: Option<String> },
    /// Pages.
    #[command(subcommand)]
    Page(PageCmd),
    /// Query: thc q 'status:open due<=+3d #work sort:due'
    Q {
        expr: Vec<String>,
        /// Say what the query means instead of listing results (`--explain=sql` adds the SQL).
        #[arg(long, value_name = "sql", num_args = 0..=1, require_equals = true, default_missing_value = "words", value_parser = ["words", "sql"])]
        explain: Option<String>,
        /// Evaluate against the state at a past time ("last tue", "2026-09-30", "-2d").
        #[arg(long, value_name = "WHEN", allow_hyphen_values = true)]
        as_of: Option<String>,
    },
    /// Recent changes, grouped by transaction.
    Log {
        /// Only changes by this actor (agent name, or human). (The global --actor sets who *you* are.)
        #[arg(long)]
        by: Option<String>,
        /// e.g. 1d, 12h, 30min
        #[arg(long, allow_hyphen_values = true)]
        since: Option<String>,
    },
    /// Undo a transaction (default: the most recent one), or bulk-revert an agent's changes.
    Undo {
        #[arg(long, conflicts_with_all = ["by", "since"])]
        tx: Option<String>,
        /// Revert every change by this agent (with --since): a preview, then a confirmation.
        #[arg(long, requires = "since")]
        by: Option<String>,
        /// e.g. 2h, 1d (with --by)
        #[arg(long, requires = "by", allow_hyphen_values = true)]
        since: Option<String>,
        /// Revert fields that were changed afterwards too.
        #[arg(long)]
        force: bool,
    },
    /// Apply a JSONL batch of operations as one transaction (all or nothing). `-` reads stdin.
    Apply {
        #[arg(default_value = "-")]
        file: String,
    },
    /// Import an outline (`- item`, indented children; capture syntax works) as one transaction.
    Import {
        /// File, or - for stdin.
        file: String,
        /// Under this node (default: today's journal).
        #[arg(long)]
        under: Option<String>,
        /// Into this day's journal.
        #[arg(long, allow_hyphen_values = true)]
        journal: Option<String>,
        /// Under this page (created if missing).
        #[arg(long)]
        page: Option<String>,
    },
    /// A view applied by default to your listings and captures (local, never synced).
    /// `thc context work`, `thc context none`, or no argument for the status.
    Context { name: Option<String> },
    /// Review what agents changed: accept or revert, oldest first.
    Review {
        #[command(subcommand)]
        cmd: Option<ReviewCmd>,
        /// Only changes by this agent.
        #[arg(long, global = true)]
        by: Option<String>,
        /// e.g. 2h, 1d
        #[arg(long, global = true, allow_hyphen_values = true)]
        since: Option<String>,
    },
    /// Set a node back to how it was at a time or right after a transaction (`<tx>^`: before it).
    Rewind {
        id: String,
        #[arg(long, value_name = "TIME|TX", allow_hyphen_values = true)]
        to: String,
    },
    /// Unresolved concurrent-edit conflicts.
    #[command(subcommand)]
    Conflict(ConflictCmd),
    /// Write the Markdown export (vault/export), or print a JSON snapshot with --json.
    Export,
    /// Turn files in vault/drop/ into inbox items.
    Ingest,
    /// Check the vault and local store. `--fix` previews repairs (days or tags made twice on
    /// two devices, empty tags); add `--yes` to write them as one transaction.
    Doctor {
        #[arg(long)]
        fix: bool,
    },
    /// Rebuild the local store by replaying the event log.
    Rebuild,
    /// Write in a journal day (today by default): the TUI, caret on a fresh line at the end.
    /// `thc j yesterday`, `thc j fri`, `thc j 2026-10-02`.
    J {
        date: Option<String>,
        /// Open in focus: only your text on screen ([tui] journal sets the default).
        #[arg(long, conflicts_with = "no_focus")]
        focus: bool,
        /// Open with the full screen (tabs, header, bar), whatever [tui] journal says.
        #[arg(long)]
        no_focus: bool,
    },
    /// Write in a page (by title or ID): the TUI, caret at the start of its first line.
    P {
        page: String,
        /// Open in focus ([tui] pages sets the default).
        #[arg(long, conflicts_with = "no_focus")]
        focus: bool,
        #[arg(long)]
        no_focus: bool,
    },
    /// Open the terminal UI (also the default when `thc` runs with no arguments in a terminal).
    Tui {
        /// Open on this node (its page or journal day), e.g. from a notification.
        #[arg(long, value_name = "ID")]
        focus: Option<String>,
        /// Open Log with the review lane on (agent changes waiting for review).
        #[arg(long, conflicts_with = "focus")]
        review: bool,
        /// Open on Log (recent changes).
        #[arg(long, conflicts_with_all = ["focus", "review"])]
        log: bool,
        /// Record the UI trace (every key, click, tick and pushed state, as JSON lines) to FILE,
        /// for `thc ui replay`.
        #[arg(long, value_name = "FILE")]
        trace: Option<std::path::PathBuf>,
    },
}

#[derive(Subcommand, Debug)]
pub enum DaemonCmd {
    /// Run in the foreground, printing one line per event (for debugging and service managers).
    Run,
    /// Is it running? Exit 0 when live, 3 when not.
    Status,
    /// Start it in the background.
    Start,
    /// Stop it (alerts pause until it runs again).
    Stop,
    /// Stop and start (after an upgrade).
    Restart,
    /// Run it at every login (launchd on macOS, systemd --user on Linux).
    Install,
    /// Remove the login item and stop it.
    Uninstall,
}

#[derive(Args, Debug)]
pub struct AddArgs {
    #[arg(required = true)]
    pub text: Vec<String>,
    /// Add to the inbox instead of today's journal.
    #[arg(long, conflicts_with_all = ["under", "journal"])]
    pub inbox: bool,
    /// Add as a child of this node.
    #[arg(long, value_name = "ID")]
    pub under: Option<String>,
    /// Add to this journal day instead of today.
    #[arg(long, allow_hyphen_values = true, value_name = "DATE")]
    pub journal: Option<String>,
    /// Use this exact id (idempotent: repeating the command returns the existing node).
    #[arg(long, conflicts_with = "key")]
    pub id: Option<String>,
    /// Idempotency key: the same key always maps to the same node, on every device.
    #[arg(long, value_name = "KEY")]
    pub key: Option<String>,
    /// Keep the text exactly as written: no capture tokens (due:, !high, [ ]) and no #tags.
    /// For notes and findings that mention the syntax ([[links]] still link; quote them to keep them as text).
    #[arg(long)]
    pub plain: bool,
}

#[derive(Args, Debug)]
pub struct TodoArgs {
    #[command(flatten)]
    pub add: AddArgs,
    #[arg(long, allow_hyphen_values = true)]
    pub due: Option<String>,
    #[arg(long, allow_hyphen_values = true, alias = "sched")]
    pub scheduled: Option<String>,
    #[arg(long, short = 'p')]
    pub priority: Option<String>,
    #[arg(long, short = 't')]
    pub tag: Vec<String>,
    /// e.g. "every 2w", "every weekday", "every! month" (from completion)
    #[arg(long)]
    pub repeat: Option<String>,
}

#[derive(Args, Debug)]
pub struct RemindArgs {
    #[arg(required = true)]
    pub text: Vec<String>,
    /// When to alert, e.g. "nov 1 9am", "tomorrow 14:00".
    #[arg(long, allow_hyphen_values = true, required = true)]
    pub at: String,
    #[arg(long)]
    pub repeat: Option<String>,
    #[arg(long, value_name = "ID")]
    pub under: Option<String>,
    #[arg(long)]
    pub inbox: bool,
    /// Create a plain dated note instead of a task.
    #[arg(long)]
    pub no_task: bool,
    /// Idempotency key: the same key always maps to the same node, on every device.
    #[arg(long, value_name = "KEY")]
    pub key: Option<String>,
}

#[derive(Args, Debug)]
pub struct MvArgs {
    pub id: String,
    #[arg(long, value_name = "ID", conflicts_with_all = ["root", "journal"])]
    pub under: Option<String>,
    /// Move to the top level (inbox, unless it has a title).
    #[arg(long)]
    pub root: bool,
    /// Move into a journal day.
    #[arg(long, allow_hyphen_values = true, value_name = "DATE")]
    pub journal: Option<String>,
    #[arg(long, value_name = "ID", conflicts_with = "before")]
    pub after: Option<String>,
    #[arg(long, value_name = "ID")]
    pub before: Option<String>,
}

#[derive(Subcommand, Debug)]
pub enum PageCmd {
    /// Create a page.
    New {
        title: Vec<String>,
        #[arg(long, short = 't')]
        tag: Vec<String>,
    },
    /// List pages.
    Ls,
}

#[derive(Subcommand, Debug)]
pub enum AlertCmd {
    /// Attach an alert: --at <datetime>, or --before <dur> relative to due (or --anchor scheduled).
    Add {
        id: String,
        #[arg(long, allow_hyphen_values = true, conflicts_with = "before")]
        at: Option<String>,
        #[arg(long, allow_hyphen_values = true, value_name = "DURATION")]
        before: Option<String>,
        #[arg(long, default_value = "due")]
        anchor: String,
    },
    /// What alerts would deliver if they fired at a time (default now): the daemon's own plan
    /// (singles, 3-in-60-s summaries, missed-while-away, silent). Writes nothing.
    Preview {
        #[arg(long, allow_hyphen_values = true, value_name = "WHEN")]
        at: Option<String>,
    },
    /// Upcoming alerts.
    Ls {
        /// Include acknowledged and past alerts.
        #[arg(long)]
        all: bool,
    },
    Ack { id: String },
    Snooze {
        id: String,
        #[arg(long, default_value = "+1h", value_name = "WHEN")]
        until: String,
    },
    Rm { id: String },
}

#[derive(Subcommand, Debug)]
pub enum ConflictCmd {
    /// List unresolved conflicts.
    Ls,
    /// Resolve: keep the current text (winner) or restore the other version (loser).
    /// Text: keep current, other or both. Move: dismiss (the node stays where it is). Moved here:
    /// --keep here or --delete.
    Resolve {
        id: String,
        /// Text: current, other or both. A line moved here (its parent was deleted elsewhere): here.
        #[arg(long, value_parser = ["current", "other", "both", "winner", "loser", "here"])]
        keep: Option<String>,
        /// A line moved here: delete it, as the other device meant.
        #[arg(long)]
        delete: bool,
    },
}

#[derive(Subcommand, Debug)]
pub enum ReviewCmd {
    /// One entry with its raw ops.
    Show { item: String },
    /// Accept changes (numbers from the last listing, or tx suffixes). A person only.
    Accept {
        items: Vec<String>,
        /// Everything in the queue (narrow with --by / --since).
        #[arg(long)]
        all: bool,
    },
    /// Revert changes, keeping fields changed afterwards unless --force.
    Revert {
        #[arg(required = true)]
        items: Vec<String>,
        #[arg(long)]
        force: bool,
    },
}

#[derive(Subcommand, Debug)]
pub enum VaultCmd {
    /// Every registered vault: name, path, open and inbox counts, how it syncs.
    Ls,
    /// Create a vault (default ~/thought-vaults/<name>) and register it.
    New { name: String, path: Option<PathBuf> },
    /// Register an existing vault folder (how teammates join a shared vault).
    Add {
        path: PathBuf,
        /// A local name, when another vault here already has its name.
        #[arg(long = "as", value_name = "NAME")]
        as_name: Option<String>,
    },
    /// Make a vault this device's current one (`.thc.toml` files still win in their projects).
    Use { name: String },
    /// A vault's path, sync, creation and counts (default: the current vault).
    Info { name: Option<String> },
    /// Rename a vault (its synced name: everyone who has it sees the new one).
    Rename { old: String, new: String },
    /// Unregister a vault. Nothing is deleted.
    Rm { name: String },
}

#[derive(Subcommand, Debug)]
pub enum ViewCmd {
    /// Save a query as a view: thc view add work 'status:open #work sort:due'
    Add {
        name: String,
        query: String,
        #[arg(long)]
        title: Option<String>,
        /// Place it on the TUI Tasks saved row (1-9).
        #[arg(long, value_name = "N")]
        tasks: Option<u32>,
        /// Make it a ThoughtBar preset.
        #[arg(long)]
        bar: bool,
        /// Capture defaults when this view is the context, e.g. '#work'.
        #[arg(long, value_name = "TEXT")]
        capture: Option<String>,
    },
    /// Change a view (only what you pass changes).
    Set {
        name: String,
        query: Option<String>,
        #[arg(long)]
        title: Option<String>,
        /// Tasks saved-row slot (1-9); 0 takes it off the row.
        #[arg(long, value_name = "N")]
        tasks: Option<u32>,
        #[arg(long, conflicts_with = "no_bar")]
        bar: bool,
        #[arg(long)]
        no_bar: bool,
        /// Capture defaults; '' clears them.
        #[arg(long, value_name = "TEXT")]
        capture: Option<String>,
        /// A section of a sectioned view (repeat for each, in order): --section Overdue 'is:overdue'.
        /// Replaces the view's sections. `thc view set today --section …` edits Today.
        #[arg(long = "section", num_args = 2, value_names = ["TITLE", "QUERY"], action = clap::ArgAction::Append)]
        section: Vec<String>,
        /// The vaults every section reads: vault:*, vault:(a or b), vault:acme; '' for the current one.
        #[arg(long, value_name = "VAULTS", allow_hyphen_values = true)]
        scope: Option<String>,
    },
    /// Every view, with how many nodes match right now (and the built-ins: today, agenda, inbox, tasks).
    Ls,
    /// Remove a view (thc undo brings it back).
    Rm { name: String },
    /// A built-in (today, agenda, inbox, tasks) back to the shipped one.
    Reset { name: String },
    /// Duplicate a view under a new name (then thc view set NEW --scope 'vault:acme').
    Copy { from: String, to: String },
}

#[derive(Subcommand, Debug)]
pub enum ReleaseToolCmd {
    /// Make an ed25519 update key: the private seed goes to FILE (mode 600, never printed), the
    /// public half (hex) is printed.
    Keygen { file: std::path::PathBuf },
    /// Print the hex signature of `thc <version> <target> <sha256>` with the key in $CLI_UPDATE_KEY.
    Sign { version: String, target: String, sha256: String },
    /// Print the public half of $CLI_UPDATE_KEY (CI checks it matches the key built into thc).
    Public,
    /// Check every target in a latest.json against the tarballs in DIR and the built-in key.
    Verify { manifest: std::path::PathBuf, dir: std::path::PathBuf },
}

#[derive(Subcommand, Debug)]
pub enum RoleCmd {
    /// Print a built-in card (lead/pm, engineer, merger, designer, reviewer, or a custom role).
    Show { name: String },
}
