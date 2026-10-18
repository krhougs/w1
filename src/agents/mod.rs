//! Agent identification and the registry of agent profiles. Each agent's knowledge lives in
//! its own module: `<agent>/mod.rs` (profile and hooks), `rules.toml` (screen rules) and
//! `tracker.rs` (session store, when it has one).

mod profile;

mod agy;
mod aider;
mod amp;
mod claude;
mod cline;
mod codex;
mod copilot;
mod crush;
mod cursor;
mod devin;
mod droid;
mod gemini;
mod goose;
mod grok;
mod hermes;
mod kilo;
mod kimi;
mod kiro;
mod letta;
mod maki;
mod mastracode;
mod muse;
mod omp;
mod opencode;
mod pi;
mod qodercli;
mod qwen;
mod zcode;

use std::ffi::OsString;
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};

pub use profile::NativeSupport;
pub(crate) use profile::{AgentProfile, NativeQuirks};

/// Screen rules for unknown programs and agents without rules of their own.
pub(crate) const GENERIC_RULES: &str = include_str!("generic.toml");

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum AgentKind {
    Claude,
    Pi,
    Omp,
    Codex,
    Gemini,
    OpenCode,
    Cursor,
    Devin,
    Antigravity,
    Cline,
    Mastracode,
    GithubCopilot,
    Kimi,
    Kiro,
    Droid,
    Amp,
    Grok,
    Hermes,
    Kilo,
    Qodercli,
    Qwen,
    Letta,
    Maki,
    Muse,
    Aider,
    Goose,
    Crush,
    Zcode,
}

impl AgentKind {
    pub const ALL: &'static [Self] = &[
        Self::Claude,
        Self::Pi,
        Self::Omp,
        Self::Codex,
        Self::Gemini,
        Self::OpenCode,
        Self::Cursor,
        Self::Devin,
        Self::Antigravity,
        Self::Cline,
        Self::Mastracode,
        Self::GithubCopilot,
        Self::Kimi,
        Self::Kiro,
        Self::Droid,
        Self::Amp,
        Self::Grok,
        Self::Hermes,
        Self::Kilo,
        Self::Qodercli,
        Self::Qwen,
        Self::Letta,
        Self::Maki,
        Self::Muse,
        Self::Aider,
        Self::Goose,
        Self::Crush,
        Self::Zcode,
    ];

    pub(crate) fn profile(self) -> &'static AgentProfile {
        match self {
            Self::Claude => &claude::PROFILE,
            Self::Pi => &pi::PROFILE,
            Self::Omp => &omp::PROFILE,
            Self::Codex => &codex::PROFILE,
            Self::Gemini => &gemini::PROFILE,
            Self::OpenCode => &opencode::PROFILE,
            Self::Cursor => &cursor::PROFILE,
            Self::Devin => &devin::PROFILE,
            Self::Antigravity => &agy::PROFILE,
            Self::Cline => &cline::PROFILE,
            Self::Mastracode => &mastracode::PROFILE,
            Self::GithubCopilot => &copilot::PROFILE,
            Self::Kimi => &kimi::PROFILE,
            Self::Kiro => &kiro::PROFILE,
            Self::Droid => &droid::PROFILE,
            Self::Amp => &amp::PROFILE,
            Self::Grok => &grok::PROFILE,
            Self::Hermes => &hermes::PROFILE,
            Self::Kilo => &kilo::PROFILE,
            Self::Qodercli => &qodercli::PROFILE,
            Self::Qwen => &qwen::PROFILE,
            Self::Letta => &letta::PROFILE,
            Self::Maki => &maki::PROFILE,
            Self::Muse => &muse::PROFILE,
            Self::Aider => &aider::PROFILE,
            Self::Goose => &goose::PROFILE,
            Self::Crush => &crush::PROFILE,
            Self::Zcode => &zcode::PROFILE,
        }
    }

    /// Canonical OSC 7501 `app` identifier (`[A-Za-z0-9_.+-]{1,32}`).
    pub fn app_id(self) -> &'static str {
        self.profile().app_id
    }

    /// Human-readable display name.
    pub fn display_name(self) -> &'static str {
        self.profile().display_name
    }

    /// OSC 7501 support of the latest official release of this agent.
    pub fn native_support(self) -> NativeSupport {
        self.profile().native
    }

    /// How `w1` handles this agent, as `--list-agents` shows it.
    pub fn summary(self) -> &'static str {
        self.profile().summary
    }
}

#[derive(Clone, Debug)]
pub struct ResolvedTarget {
    pub program: OsString,
    pub args: Vec<OsString>,
    pub resolved_executable: Option<PathBuf>,
    pub agent: Option<AgentKind>,
    pub app_id: String,
    pub display_name: String,
    pub native: NativeSupport,
    /// argv as lossy strings (program first), used for agent-specific argument inspection.
    pub argv: Vec<String>,
}

impl ResolvedTarget {
    pub fn resolve(program: OsString, args: Vec<OsString>) -> Self {
        let program_str = program.to_string_lossy();
        let resolved_executable = resolve_in_path(Path::new(program_str.as_ref()));

        let arg_strings: Vec<String> = std::iter::once(program_str.into_owned())
            .chain(args.iter().map(|a| a.to_string_lossy().into_owned()))
            .collect();

        let agent = identify_from_command(&arg_strings, resolved_executable.as_deref());

        let (app_id, display_name) = match agent {
            Some(kind) => (kind.app_id().to_owned(), kind.display_name().to_owned()),
            None => {
                let raw_name = path_basename(arg_strings.first().map(String::as_str).unwrap_or_default());
                let app = crate::osc7501::sanitize_app(raw_name).unwrap_or_else(|| "cli".to_owned());
                (app.clone(), raw_name.to_owned())
            }
        };

        // A known agent that has started emitting OSC 7501 so far always reports less than w1
        // can add, so it is merged rather than passed through; unknown programs are trusted.
        let has_marker = || {
            resolved_executable
                .as_deref()
                .is_some_and(binary_contains_osc7501_marker)
        };
        let native = match agent.map(AgentKind::native_support) {
            Some(NativeSupport::None) if has_marker() => NativeSupport::Partial,
            None if has_marker() => NativeSupport::Full,
            Some(support) => support,
            None => NativeSupport::None,
        };

        Self {
            program,
            args,
            resolved_executable,
            agent,
            app_id,
            display_name,
            native,
            argv: arg_strings,
        }
    }
}

pub fn identify_from_command(argv: &[String], resolved_exe: Option<&Path>) -> Option<AgentKind> {
    let first = argv.first()?;
    let base = normalized_lookup_name(path_basename(first));

    if is_generic_runtime(&base) {
        if let Some(wrapped) = identify_wrapped_runtime_arg(&base, argv) {
            return Some(wrapped);
        }
    }

    if let Some(kind) = lookup_agent_name(&base) {
        return Some(kind);
    }

    if let Some(kind) = lookup_from_package_path(first) {
        return Some(kind);
    }

    if let Some(exe) = resolved_exe {
        if let Some(name) = exe.file_name().and_then(|n| n.to_str()) {
            let norm = normalized_lookup_name(name);
            if let Some(kind) = lookup_agent_name(&norm) {
                return Some(kind);
            }
        }
        if let Ok(canon) = fs::canonicalize(exe) {
            if let Some(name) = canon.file_name().and_then(|n| n.to_str()) {
                let norm = normalized_lookup_name(name);
                if let Some(kind) = lookup_agent_name(&norm) {
                    return Some(kind);
                }
            }
            if let Some(kind) = lookup_from_package_path(&canon.to_string_lossy()) {
                return Some(kind);
            }
        }
    }

    None
}

fn identify_wrapped_runtime_arg(runtime: &str, argv: &[String]) -> Option<AgentKind> {
    match runtime {
        "node" | "bun" | "deno" => {
            let mut iter = argv.iter().skip(1);
            while let Some(arg) = iter.next() {
                if arg == "--" {
                    return iter.next().and_then(|a| identify_token(a));
                }
                if matches!(arg.as_str(), "run" | "x") {
                    continue;
                }
                if arg.starts_with('-') {
                    if matches!(
                        arg.as_str(),
                        "-r" | "--require"
                            | "--loader"
                            | "--import"
                            | "--experimental-loader"
                            | "--inspect-port"
                            | "-e"
                            | "--eval"
                            | "-p"
                            | "--print"
                    ) {
                        let _ = iter.next();
                    }
                    continue;
                }
                return identify_token(arg);
            }
            None
        }
        "npx" | "bunx" | "pnpx" | "uvx" | "pipx" => {
            let mut iter = argv.iter().skip(1);
            while let Some(arg) = iter.next() {
                if arg == "--" {
                    return iter.next().and_then(|a| identify_token(a));
                }
                if arg == "run" {
                    continue;
                }
                if arg.starts_with('-') {
                    if matches!(arg.as_str(), "-p" | "--package" | "--from") {
                        if let Some(pkg) = iter.next() {
                            if let Some(k) = identify_token(pkg) {
                                return Some(k);
                            }
                        }
                    }
                    continue;
                }
                return identify_token(arg);
            }
            None
        }
        "python" | "python3" => {
            let mut iter = argv.iter().skip(1);
            while let Some(arg) = iter.next() {
                if arg == "-m" {
                    return iter.next().and_then(|a| identify_token(a));
                }
                if arg == "-c" {
                    return None;
                }
                if arg.starts_with('-') {
                    continue;
                }
                return identify_token(arg);
            }
            None
        }
        "sh" | "bash" | "zsh" | "fish" => {
            let mut iter = argv.iter().skip(1);
            while let Some(arg) = iter.next() {
                if arg == "-c" {
                    if let Some(cmd) = iter.next() {
                        if let Some(first_word) = cmd.split_whitespace().next() {
                            return identify_token(first_word);
                        }
                    }
                    return None;
                }
                if arg.starts_with('-') {
                    continue;
                }
                return identify_token(arg);
            }
            None
        }
        _ => None,
    }
}

fn identify_token(token: &str) -> Option<AgentKind> {
    let trimmed = token.trim_matches(|c| matches!(c, '"' | '\''));
    if trimmed.is_empty() || trimmed.starts_with('-') {
        return None;
    }
    let without_version = trimmed.split('@').next().unwrap_or(trimmed);
    let base = normalized_lookup_name(path_basename(without_version));
    lookup_agent_name(&base).or_else(|| lookup_from_package_path(trimmed))
}

pub fn lookup_agent_name(name: &str) -> Option<AgentKind> {
    let norm = normalized_lookup_name(path_basename(name));
    AgentKind::ALL.iter().copied().find(|kind| {
        let profile = kind.profile();
        profile.commands.contains(&norm.as_str())
            || profile.versioned_command.is_some_and(|prefix| {
                norm.strip_prefix(prefix)
                    .is_some_and(|version| version.starts_with(|c: char| c.is_ascii_digit()))
            })
    })
}

fn lookup_from_package_path(path: &str) -> Option<AgentKind> {
    let lower = path.to_lowercase();
    AgentKind::ALL.iter().copied().find(|kind| {
        kind.profile()
            .install_paths
            .iter()
            .any(|p| lower.contains(p))
    })
}

fn is_generic_runtime(name: &str) -> bool {
    matches!(
        name,
        "sh" | "bash"
            | "zsh"
            | "fish"
            | "node"
            | "bun"
            | "deno"
            | "npx"
            | "bunx"
            | "pnpx"
            | "uvx"
            | "pipx"
            | "python"
            | "python3"
    )
}

pub fn normalized_lookup_name(name: &str) -> String {
    let mut out = name.trim().to_lowercase();
    for suffix in [".exe", ".cmd", ".bat", ".ps1", ".js", ".mjs", ".cjs", ".py", ".sh"] {
        if out.ends_with(suffix) {
            out.truncate(out.len() - suffix.len());
            break;
        }
    }
    out
}

pub fn path_basename(path: &str) -> &str {
    path.rsplit(['/', '\\'])
        .find(|comp| !comp.is_empty())
        .unwrap_or(path)
}

pub fn resolve_in_path(program: &Path) -> Option<PathBuf> {
    if program.components().count() > 1 {
        return executable_file(program).then(|| program.to_path_buf());
    }
    let path_var = std::env::var_os("PATH")?;
    for dir in std::env::split_paths(&path_var) {
        let candidate = dir.join(program);
        if executable_file(&candidate) {
            return Some(candidate);
        }
    }
    None
}

fn executable_file(path: &Path) -> bool {
    let Ok(meta) = fs::metadata(path) else {
        return false;
    };
    if !meta.is_file() {
        return false;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        (meta.permissions().mode() & 0o111) != 0
    }
    #[cfg(not(unix))]
    {
        true
    }
}

/// Checks if an executable file or its resolved target contains the OSC 7501 emission marker
/// (`]7501;state=` or `7501;state=`). Bounded to 256 MiB to stay fast and safe.
fn binary_contains_osc7501_marker(path: &Path) -> bool {
    let target = fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    let Ok(meta) = fs::metadata(&target) else {
        return false;
    };
    if !meta.is_file() || meta.len() > 256 * 1024 * 1024 {
        return false;
    }
    let Ok(mut file) = fs::File::open(&target) else {
        return false;
    };
    let needles: &[&[u8]] = &[b"]7501;state=", b"]7501;?"];
    let max_needle = 12;
    let mut buf = vec![0u8; 256 * 1024];
    let mut tail = Vec::with_capacity(max_needle);

    loop {
        let Ok(read) = file.read(&mut buf) else {
            return false;
        };
        if read == 0 {
            break;
        }
        let mut window = Vec::with_capacity(tail.len() + read);
        window.extend_from_slice(&tail);
        window.extend_from_slice(&buf[..read]);
        for needle in needles {
            if window.windows(needle.len()).any(|w| w == *needle) {
                return true;
            }
        }
        tail.clear();
        let keep = window.len().saturating_sub(max_needle);
        tail.extend_from_slice(&window[keep..]);
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identifies_common_agents_and_wrappers() {
        assert_eq!(
            identify_from_command(&["codex".into(), "--yolo".into()], None),
            Some(AgentKind::Codex)
        );
        assert_eq!(
            identify_from_command(&["claude".into()], None),
            Some(AgentKind::Claude)
        );
        assert_eq!(
            identify_from_command(&["npx".into(), "@openai/codex".into()], None),
            Some(AgentKind::Codex)
        );
        assert_eq!(
            identify_from_command(&["node".into(), "/opt/homebrew/bin/gemini".into()], None),
            Some(AgentKind::Gemini)
        );
        assert_eq!(AgentKind::Claude.native_support(), NativeSupport::Full);
        assert_eq!(AgentKind::Pi.native_support(), NativeSupport::Full);
        assert_eq!(AgentKind::Omp.native_support(), NativeSupport::Partial);
        assert_eq!(AgentKind::Codex.native_support(), NativeSupport::None);
    }
}
