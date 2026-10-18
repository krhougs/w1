use std::ffi::OsString;
use std::process::ExitCode;

use w1::{AgentKind, NativeSupport, ResolvedTarget, RunOptions};

fn main() -> ExitCode {
    let mut args = std::env::args_os().skip(1).peekable();
    let mut options = RunOptions::from_env();

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
                options.force_wrap = true;
                let _ = args.next();
            }
            "--no-probe-answer" => {
                options.answer_probe = false;
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
    let code = w1::run_target(target, options);
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
  --list-agents      List recognised coding agents and how each is handled
  --force-wrap       Wrap in a PTY even if the program fully supports OSC 7501
  --no-probe-answer  Inside a terminal multiplexer (Herdr, tmux, Zellij, GNU Screen), do not
                     answer the OSC 7501 probe of a program that fully supports OSC 7501;
                     launch it directly instead. Same as {}=1
  -h, --help         Show this help
  -V, --version      Show version",
        env!("CARGO_PKG_VERSION"),
        w1::NO_PROBE_ANSWER_ENV
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
        println!(
            "{:<16} {:<22} {:<12} {}",
            kind.app_id(),
            kind.display_name(),
            mode,
            kind.summary()
        );
    }
}
