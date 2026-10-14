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

用法:
  w1 [w1选项] <agent命令> [agent参数...]

示例:
  w1 codex --yolo
  w1 gemini
  w1 opencode
  w1 claude

选项:
  --list-agents   列出已识别的 Coding Agent 及其 OSC 7501 支持状态
  --force-wrap    即使目标应用已原生支持 OSC 7501 也强制使用 PTY 包装
  -h, --help      显示帮助信息
  -V, --version   显示版本信息",
        env!("CARGO_PKG_VERSION")
    );
}

fn print_agents() {
    println!(
        "{:<16} {:<22} {:<14} 说明",
        "APP ID", "名称", "模式"
    );
    println!("{}", "-".repeat(76));
    for &kind in AgentKind::ALL {
        let mode = match kind.native_support() {
            NativeSupport::Full => "原生直通 (Passthrough)",
            NativeSupport::Partial => "原生 + w1 补全",
            NativeSupport::None => "w1 包装 (Wrap7501)",
        };
        let note = match kind {
            AgentKind::Claude => "Claude Code >= 2.1.295 原生支持 OSC 7501",
            AgentKind::Pi => "Pi Coding Agent >= 1.1.0 原生支持 OSC 7501",
            AgentKind::Omp => "原生只报状态：w1 补 working/标题/摘要/进度（会话 JSONL），并去掉标题 spinner 动画",
            AgentKind::Codex => "终端 + ~/.codex/state_5.sqlite 线程（按提交时刻认领）+ rollout JSONL",
            AgentKind::Gemini => "终端/OSC 标题 + $GEMINI_CLI_HOME/.gemini/tmp/<项目>/chats JSONL",
            AgentKind::OpenCode => "终端 + ~/.local/share/opencode/opencode.db（会话/消息/todo/子会话）",
            AgentKind::Kilo => "终端 + ~/.local/share/kilo（与 OpenCode 同构的会话库，未实测）",
            AgentKind::Grok => "原生 + w1 补全：active_sessions.json（按 pid）+ events/updates JSONL",
            AgentKind::Devin => "终端 + ~/.local/share/devin/cli（session_locks 按 pid）sessions.db",
            AgentKind::Cursor => "终端 + ~/.cursor chats meta.json + agent-transcripts（未登录，未实测回合）",
            AgentKind::Antigravity => "终端 + agy 日志（按 pid）+ conversation 数据库",
            AgentKind::Droid => "原生 + w1 补全：~/.factory/sessions/<cwd> JSONL（标题/摘要/todo/错误）",
            AgentKind::GithubCopilot => "终端 + ~/.copilot/session-state（inuse.<pid>.lock）events.jsonl",
            AgentKind::Amp => "原生 + w1 补全：终端画面（线程在服务端，本地无会话记录）",
            AgentKind::Kimi => "终端 + $KIMI_CODE_HOME/sessions/wd_*/ state.json & wire.jsonl",
            AgentKind::Qwen => "终端/OSC 标题 + ~/.qwen/sessions/<pid>.json + projects/*/chats JSONL",
            AgentKind::Crush => "原生 + w1 补全：<数据目录>/crush.db（标题/摘要/todo/错误）",
            _ => "终端画面 + OSC 0/2/9 实时状态与摘要推断",
        };
        println!(
            "{:<16} {:<22} {:<22} {}",
            kind.app_id(),
            kind.display_name(),
            mode,
            note
        );
    }
}
