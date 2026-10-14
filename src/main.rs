pub mod agents;
pub mod detect;
pub mod inspect;
pub mod osc7501;
pub mod procs;
pub mod pty;
pub mod report;
pub mod screen;
pub mod stream;

use std::ffi::OsString;
use std::process::ExitCode;

use agents::{AgentKind, NativeSupport, ResolvedTarget};

fn main() -> ExitCode {
    let mut args = std::env::args_os().skip(1).peekable();
    let mut force_wrap = false;

    while let Some(arg) = args.peek() {
        let s = arg.to_string_lossy();
        match s.as_ref() {
            "-h" | "--help" => {
                print_help();
                return ExitCode::SUCCESS;
            }
            "-V" | "--version" => {
                println!("w1 (Wrap7501) {}", env!("CARGO_PKG_VERSION"));
                return ExitCode::SUCCESS;
            }
            "--list-agents" => {
                print_agents();
                return ExitCode::SUCCESS;
            }
            "--force-wrap" => {
                force_wrap = true;
                let _ = args.next();
            }
            "--" => {
                let _ = args.next();
                break;
            }
            _ => break,
        }
    }

    let Some(program) = args.next() else {
        print_help();
        return ExitCode::from(2);
    };
    let rest_args: Vec<OsString> = args.collect();

    let target = ResolvedTarget::resolve(program, rest_args);
    let code = pty::run_target(target, force_wrap);
    ExitCode::from(code)
}

fn print_help() {
    println!(
        "\
w1 (Wrap7501) {} — OSC 7501 (Program Status Protocol) Coding Agent Wrapper

Usage:
  w1 [w1 options] <agent command> [agent args...]

Examples:
  w1 codex --yolo
  w1 gemini
  w1 opencode
  w1 claude

Options:
  --list-agents   List recognised coding agents and how each is handled
  --force-wrap    Wrap in a PTY even if the program fully supports OSC 7501
  -h, --help      Show this help
  -V, --version   Show version",
        env!("CARGO_PKG_VERSION")
    );
}

fn print_agents() {
    println!(
        "{:<16} {:<22} {:<12} Notes",
        "APP ID", "Name", "Mode"
    );
    println!("{}", "-".repeat(76));
    for &kind in AgentKind::ALL {
        let mode = match kind.native_support() {
            NativeSupport::Full => "Passthrough",
            NativeSupport::Partial => "Native + w1",
            NativeSupport::None => "Wrapped",
        };
        let note = match kind {
            AgentKind::Claude => "Claude Code >= 2.1.295 supports OSC 7501 natively",
            AgentKind::Pi => "Pi Coding Agent >= 1.1.0 supports OSC 7501 natively",
            AgentKind::Omp => "Native state only; w1 adds working/title/summary/progress (session JSONL) and strips the title spinner",
            AgentKind::Codex => "Screen + ~/.codex/state_5.sqlite threads (claimed by prompt time) + rollout JSONL",
            AgentKind::Gemini => "Screen/OSC title + $GEMINI_CLI_HOME/.gemini/tmp/<project>/chats JSONL",
            AgentKind::OpenCode => "Screen + ~/.local/share/opencode/opencode.db (sessions/messages/todos/subagents)",
            AgentKind::Kilo => "Screen + ~/.local/share/kilo (same store as OpenCode; untested)",
            AgentKind::Grok => "Native + w1: active_sessions.json (by pid) + events/updates JSONL",
            AgentKind::Devin => "Screen + ~/.local/share/devin/cli sessions.db (session_locks by pid)",
            AgentKind::Cursor => "Screen + ~/.cursor chats meta.json + agent-transcripts (turns untested)",
            AgentKind::Antigravity => "Screen + agy log (by pid) + conversation databases",
            AgentKind::Droid => "Native + w1: ~/.factory/sessions/<cwd> JSONL (title/summary/todos/errors)",
            AgentKind::GithubCopilot => "Screen + ~/.copilot/session-state events.jsonl (inuse.<pid>.lock)",
            AgentKind::Amp => "Native + w1: screen only (threads live server-side)",
            AgentKind::Kimi => "Screen + $KIMI_CODE_HOME/sessions/wd_*/ state.json & wire.jsonl",
            AgentKind::Qwen => "Screen/OSC title + ~/.qwen/sessions/<pid>.json + projects/*/chats JSONL",
            AgentKind::Crush => "Native + w1: <data dir>/crush.db (title/summary/todos/errors)",
            _ => "Screen + OSC 0/2/9 heuristics",
        };
        println!(
            "{:<16} {:<22} {:<12} {}",
            kind.app_id(),
            kind.display_name(),
            mode,
            note
        );
    }
}
