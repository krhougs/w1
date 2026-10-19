use std::time::Duration;

use crate::engine::store::Tracker;
use crate::stream::TitleFilter;

/// OSC 7501 support of an agent's latest official release.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NativeSupport {
    None,
    Partial,
    Full,
}

/// Everything `w1` knows about one agent. Each module under `agents/` declares one; the engine
/// reads agent differences only from here.
pub(crate) struct AgentProfile {
    /// OSC 7501 `app` identifier (`[A-Za-z0-9_.+-]{1,32}`).
    pub app_id: &'static str,
    pub display_name: &'static str,
    /// Executable names, lower-cased and without `.exe`/`.js`/… suffixes.
    pub commands: &'static [&'static str],
    /// An executable named by this prefix followed by a version (`muse-bin-1.2`).
    pub versioned_command: Option<&'static str>,
    /// Lower-case substrings of an install path (npm package, version directory).
    pub install_paths: &'static [&'static str],
    /// Use the latest release as the reference.
    pub native: NativeSupport,
    pub native_quirks: NativeQuirks,
    /// Screen rules, see `engine::rules`.
    pub rules: &'static str,
    pub titles: TitleFilter,
    pub resume: Resume,
    /// Reads the agent's session store.
    pub tracker: Option<fn() -> Box<dyn Tracker>>,
    pub hooks: Hooks,
    /// How `w1` handles the agent, for `--list-agents`.
    pub summary: &'static str,
}

impl AgentProfile {
    /// Defaults for the optional fields. Every profile sets `app_id`, `display_name`, `commands`
    /// and `rules`.
    pub const DEFAULTS: Self = Self {
        app_id: "",
        display_name: "",
        commands: &[],
        versioned_command: None,
        install_paths: &[],
        native: NativeSupport::None,
        native_quirks: NativeQuirks::NONE,
        rules: "",
        titles: TitleFilter::Despin,
        resume: Resume::DEFAULT,
        tracker: None,
        hooks: Hooks::NONE,
        summary: "Screen + OSC 0/2/9 heuristics",
    };
}

/// Native reports that must not be taken at face value.
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct NativeQuirks {
    /// A root record only counts once it has stood this long.
    pub settle: Option<Duration>,
    /// A native `blocked` only counts once its dialog has been on screen, until the record
    /// changes or the idle prompt is back.
    pub blocked_needs_screen: bool,
    /// Reports `idle` where a turn finished, so its `idle` does not undo a `done` w1 saw.
    pub idle_for_done: bool,
}

impl NativeQuirks {
    pub const NONE: Self = Self {
        settle: None,
        blocked_needs_screen: false,
        idle_for_done: false,
    };
}

/// How the user asks the agent to continue an earlier session, so a session created before
/// launch may be ours.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Resume {
    pub flags: &'static [&'static str],
    /// First positional arguments (`codex resume`).
    pub subcommands: &'static [&'static str],
}

impl Resume {
    pub const DEFAULT: Self = Self {
        flags: &["--continue", "--resume", "--session"],
        subcommands: &[],
    };

    pub const fn flags(flags: &'static [&'static str]) -> Self {
        Self {
            flags,
            subcommands: &[],
        }
    }
}

/// `msg` of a blocked screen, from the matched rule id and the bottom non-empty lines.
pub(crate) type BlockedDetail = fn(&str, &[&str]) -> Option<String>;

/// `msg` of a working screen, from the OSC title.
pub(crate) type WorkingDetail = fn(&str) -> Option<String>;

/// Agent-specific screen summaries, tried before the generic heuristics.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Hooks {
    pub blocked_detail: Option<BlockedDetail>,
    pub working_detail: Option<WorkingDetail>,
}

impl Hooks {
    pub const NONE: Self = Self {
        blocked_detail: None,
        working_detail: None,
    };
}
