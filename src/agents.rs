use std::ffi::OsString;
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};

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

    /// Canonical OSC 7501 `app` identifier (`[A-Za-z0-9_.+-]{1,32}`).
    pub fn app_id(self) -> &'static str {
        match self {
            Self::Claude => "claude-code",
            Self::Pi => "pi",
            Self::Omp => "omp",
            Self::Codex => "codex",
            Self::Gemini => "gemini",
            Self::OpenCode => "opencode",
            Self::Cursor => "cursor",
            Self::Devin => "devin",
            Self::Antigravity => "antigravity",
            Self::Cline => "cline",
            Self::Mastracode => "mastracode",
            Self::GithubCopilot => "copilot",
            Self::Kimi => "kimi",
            Self::Kiro => "kiro",
            Self::Droid => "droid",
            Self::Amp => "amp",
            Self::Grok => "grok-build",
            Self::Hermes => "hermes",
            Self::Kilo => "kilo",
            Self::Qodercli => "qodercli",
            Self::Qwen => "qwen",
            Self::Letta => "letta",
            Self::Maki => "maki",
            Self::Muse => "muse",
            Self::Aider => "aider",
            Self::Goose => "goose",
            Self::Crush => "crush",
            Self::Zcode => "zcode",
        }
    }

    /// Human-readable display name.
    pub fn display_name(self) -> &'static str {
        match self {
            Self::Claude => "Claude Code",
            Self::Pi => "Pi Coding Agent",
            Self::Omp => "OMP",
            Self::Codex => "OpenAI Codex CLI",
            Self::Gemini => "Gemini CLI",
            Self::OpenCode => "OpenCode",
            Self::Cursor => "Cursor Agent",
            Self::Devin => "Devin CLI",
            Self::Antigravity => "Antigravity CLI",
            Self::Cline => "Cline",
            Self::Mastracode => "Mastra Code",
            Self::GithubCopilot => "GitHub Copilot CLI",
            Self::Kimi => "Kimi Code",
            Self::Kiro => "Kiro CLI",
            Self::Droid => "Factory Droid",
            Self::Amp => "Sourcegraph Amp",
            Self::Grok => "Grok Build",
            Self::Hermes => "Hermes Agent",
            Self::Kilo => "Kilo Code",
            Self::Qodercli => "Qoder CLI",
            Self::Qwen => "Qwen Code",
            Self::Letta => "Letta Code",
            Self::Maki => "Maki",
            Self::Muse => "Muse Code",
            Self::Aider => "Aider",
            Self::Goose => "Goose",
            Self::Crush => "Crush",
            Self::Zcode => "ZCode",
        }
    }

    /// Manifest ID used for detection rules.
    pub fn manifest_id(self) -> &'static str {
        match self {
            Self::Claude => "claude",
            Self::Pi => "pi",
            Self::Omp => "omp",
            Self::Codex => "codex",
            Self::Gemini => "gemini",
            Self::OpenCode => "opencode",
            Self::Cursor => "cursor",
            Self::Devin => "devin",
            Self::Antigravity => "agy",
            Self::Cline => "cline",
            Self::Mastracode => "mastracode",
            Self::GithubCopilot => "copilot",
            Self::Kimi => "kimi",
            Self::Kiro => "kiro",
            Self::Droid => "droid",
            Self::Amp => "amp",
            Self::Grok => "grok",
            Self::Hermes => "hermes",
            Self::Kilo => "kilo",
            Self::Qodercli => "qodercli",
            Self::Qwen => "qwen",
            Self::Letta => "letta",
            Self::Maki => "maki",
            Self::Muse => "muse",
            Self::Aider => "aider",
            Self::Goose => "goose",
            Self::Crush => "crush",
            Self::Zcode => "zcode",
        }
    }

    /// OSC 7501 support of the latest official release of this agent.
    pub fn native_support(self) -> NativeSupport {
        match self {
            Self::Claude | Self::Pi => NativeSupport::Full,
            // omp reports state only: no `title`, no turn summary, and the working indicator lives
            // in its OSC 0 title spinner. Grok Build >= 1.0.52 reports natively but stays silent
            // inside tmux. Crush (>= 0.98), Droid (>= 0.236) and Amp report state/kind only.
            // agy (>= 1.4.0) reports no title, and `blocked` with no dialog on screen. `w1` wraps
            // them, answers their probe and completes the records.
            Self::Omp
            | Self::Grok
            | Self::Crush
            | Self::Droid
            | Self::Amp
            | Self::Antigravity => NativeSupport::Partial,
            _ => NativeSupport::None,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NativeSupport {
    None,
    Partial,
    Full,
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
    match norm.as_str() {
        "claude" | "claude-code" | "claudecode" => Some(AgentKind::Claude),
        "pi" | "pi-coding-agent" => Some(AgentKind::Pi),
        "omp" | "oh-my-pi" => Some(AgentKind::Omp),
        "codex" | "codex-cli" => Some(AgentKind::Codex),
        "gemini" | "gemini-cli" => Some(AgentKind::Gemini),
        "opencode" | "opencode2" | "open-code" => Some(AgentKind::OpenCode),
        "cursor" | "cursor-agent" | "agent" => Some(AgentKind::Cursor),
        "devin" | "devin-cli" => Some(AgentKind::Devin),
        "agy" | "antigravity" | "antigravity-cli" => Some(AgentKind::Antigravity),
        "cline" | ".cline" => Some(AgentKind::Cline),
        "mastracode" | "mastra-code" => Some(AgentKind::Mastracode),
        "copilot" | "github-copilot" | "ghcs" => Some(AgentKind::GithubCopilot),
        "kimi" | "kimi-code" | "kimi-cli" | "kimi-legacy" => Some(AgentKind::Kimi),
        "kiro" | "kiro-cli" => Some(AgentKind::Kiro),
        "droid" | "factory-droid" => Some(AgentKind::Droid),
        "amp" | "amp-local" => Some(AgentKind::Amp),
        "grok" | "grok-build" => Some(AgentKind::Grok),
        "hermes" | "hermes-agent" => Some(AgentKind::Hermes),
        "kilo" | "kilo-code" | "kilocode" => Some(AgentKind::Kilo),
        "qodercli" | "qoderclicn" | "qoder" | "qodercn" => Some(AgentKind::Qodercli),
        "qwen" | "qwen-code" => Some(AgentKind::Qwen),
        "letta" | "letta-code" => Some(AgentKind::Letta),
        "maki" => Some(AgentKind::Maki),
        "muse" | "muse-code" | "muse-cli" => Some(AgentKind::Muse),
        "aider" | "aider-chat" => Some(AgentKind::Aider),
        "goose" | "goose-cli" => Some(AgentKind::Goose),
        "crush" => Some(AgentKind::Crush),
        "zcode" | "ztui" | "zcode-app-cli" => Some(AgentKind::Zcode),
        _ if norm.starts_with("muse-bin-")
            && norm["muse-bin-".len()..].starts_with(|c: char| c.is_ascii_digit()) =>
        {
            Some(AgentKind::Muse)
        }
        _ => None,
    }
}

fn lookup_from_package_path(path: &str) -> Option<AgentKind> {
    let lower = path.to_lowercase();
    if lower.contains("@anthropic-ai/claude-code") || lower.contains("/claude/versions/") {
        return Some(AgentKind::Claude);
    }
    if lower.contains("@earendil-works/pi-coding-agent") {
        return Some(AgentKind::Pi);
    }
    if lower.contains("@openai/codex") {
        return Some(AgentKind::Codex);
    }
    if lower.contains("@google/gemini-cli") || lower.contains("/gemini-cli/") {
        return Some(AgentKind::Gemini);
    }
    if lower.contains("cursor-agent") {
        return Some(AgentKind::Cursor);
    }
    if lower.contains("/devin/cli/") {
        return Some(AgentKind::Devin);
    }
    if lower.contains("kimi-cli") || lower.contains("kimi-code") {
        return Some(AgentKind::Kimi);
    }
    if lower.contains("@qwen-code/qwen-code") || lower.contains("qwen-code") {
        return Some(AgentKind::Qwen);
    }
    if lower.contains("@sourcegraph/amp") {
        return Some(AgentKind::Amp);
    }
    if lower.contains("@github/copilot") {
        return Some(AgentKind::GithubCopilot);
    }
    None
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
