use std::ffi::{c_int, CString, OsString};
use std::io;
use std::mem::MaybeUninit;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd, RawFd};
use std::os::unix::ffi::OsStrExt;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicI32, Ordering};
use std::time::{Duration, Instant};

use crate::agents::{AgentKind, NativeSupport, ResolvedTarget};
use crate::osc7501::OscState;
use crate::report::Reporter;
use crate::stream::{Captured, OutputStream, TitleFilter};

const STDIN: RawFd = 0;
const STDOUT: RawFd = 1;
const BUFFER_LIMIT: usize = 256 * 1024;
const READ_CHUNK: usize = 64 * 1024;
const TICK_INTERVAL: Duration = Duration::from_millis(180);
const IDLE_TICK_INTERVAL: Duration = Duration::from_millis(900);
const FINAL_FLUSH_TIMEOUT: Duration = Duration::from_secs(1);
const OSC_HEAD_STALL: Duration = Duration::from_millis(120);

static SIGNAL_PIPE: AtomicI32 = AtomicI32::new(-1);

pub fn run_target(target: ResolvedTarget, force_wrap: bool) -> u8 {
    if target.native == NativeSupport::Full && !force_wrap {
        return run_passthrough(&target);
    }

    // SAFETY: isatty only inspects fd 0.
    if unsafe { libc::isatty(STDIN) } != 1 {
        return exec_or_spawn_direct(&target);
    }

    match PtyWrapper::start(&target) {
        Ok(wrapper) => wrapper.run(target),
        Err(err) => {
            eprintln!("w1: failed to initialize PTY wrapper ({err}), falling back to direct launch");
            exec_or_spawn_direct(&target)
        }
    }
}

fn run_passthrough(target: &ResolvedTarget) -> u8 {
    eprintln!(
        "\x1b[36m[w1]\x1b[0m {} ({}) supports OSC 7501 (Program Status Protocol) natively; launching it directly without wrapping…",
        target.display_name, target.app_id
    );

    let code = spawn_and_wait_direct(target);

    eprintln!(
        "\x1b[36m[w1]\x1b[0m {} exited. It supports OSC 7501 natively, so you can run `{}` directly without `w1`.",
        target.display_name,
        target.program.to_string_lossy()
    );
    code
}

fn spawn_and_wait_direct(target: &ResolvedTarget) -> u8 {
    let mut cmd = Command::new(&target.program);
    cmd.args(&target.args)
        .stdin(Stdio::inherit())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit());

    let mut child = match cmd.spawn() {
        Ok(child) => child,
        Err(err) => {
            eprintln!("w1: failed to launch `{}`: {err}", target.program.to_string_lossy());
            return 127;
        }
    };
    // Like system(3): keyboard signals belong to the child; w1 must outlive it to print the
    // closing hint and forward its exit status. Installed after spawn so the child keeps defaults.
    // SAFETY: Plain signal disposition changes for this process.
    let previous = unsafe {
        [
            libc::signal(libc::SIGINT, libc::SIG_IGN),
            libc::signal(libc::SIGQUIT, libc::SIG_IGN),
        ]
    };
    let status = child.wait();
    // SAFETY: Restores the dispositions saved above.
    unsafe {
        libc::signal(libc::SIGINT, previous[0]);
        libc::signal(libc::SIGQUIT, previous[1]);
    }
    match status {
        Ok(status) => {
            use std::os::unix::process::ExitStatusExt;
            if let Some(code) = status.code() {
                (code & 0xff) as u8
            } else if let Some(sig) = status.signal() {
                128u8.wrapping_add(sig as u8)
            } else {
                1
            }
        }
        Err(err) => {
            eprintln!("w1: failed to wait for `{}`: {err}", target.program.to_string_lossy());
            1
        }
    }
}

fn exec_or_spawn_direct(target: &ResolvedTarget) -> u8 {
    spawn_and_wait_direct(target)
}

struct PtyWrapper {
    master: OwnedFd,
    child: libc::pid_t,
    signals: OwnedFd,
    outer_termios: libc::termios,
    initial_size: libc::winsize,
}

impl PtyWrapper {
    fn start(target: &ResolvedTarget) -> io::Result<Self> {
        let program_c = to_cstring(&target.program)?;
        let mut argv_c: Vec<CString> = Vec::with_capacity(target.args.len() + 1);
        argv_c.push(program_c.clone());
        for arg in &target.args {
            argv_c.push(to_cstring(arg)?);
        }
        let argv_ptrs: Vec<*const libc::c_char> = argv_c
            .iter()
            .map(|a| a.as_ptr())
            .chain(std::iter::once(std::ptr::null()))
            .collect();

        let signals = install_signal_pipe()?;
        let outer_termios = get_termios(STDIN)?;
        let mut size = get_window_size(STDIN).unwrap_or(libc::winsize {
            ws_row: 24,
            ws_col: 80,
            ws_xpixel: 0,
            ws_ypixel: 0,
        });
        let mut inner_termios = outer_termios;
        let mut master: c_int = -1;
        let mut slave: c_int = -1;

        // SAFETY: Valid stack pointers passed to openpty; descriptors immediately wrapped in OwnedFd.
        let rc = unsafe {
            libc::openpty(
                &mut master,
                &mut slave,
                std::ptr::null_mut(),
                &mut inner_termios,
                &mut size,
            )
        };
        if rc != 0 || master < 0 || slave < 0 {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: Descriptors were just opened by openpty.
        let (master, slave) =
            unsafe { (OwnedFd::from_raw_fd(master), OwnedFd::from_raw_fd(slave)) };
        set_cloexec(&master)?;
        set_cloexec(&slave)?;
        set_nonblocking(master.as_raw_fd())?;

        // SAFETY: Single-threaded before fork; child only calls async-signal-safe functions + execvp.
        let child = unsafe { libc::fork() };
        if child < 0 {
            return Err(io::Error::last_os_error());
        }
        if child == 0 {
            unsafe {
                exec_child(
                    slave.as_raw_fd(),
                    program_c.as_ptr(),
                    argv_ptrs.as_ptr(),
                )
            }
        }

        drop(slave);
        let _ = set_nonblocking(STDIN);
        let _ = set_nonblocking(STDOUT);

        let mut raw = outer_termios;
        // SAFETY: Valid termios struct modified via cfmakeraw and applied to STDIN.
        unsafe {
            libc::cfmakeraw(&mut raw);
            libc::tcsetattr(STDIN, libc::TCSANOW, &raw);
        }

        let wrapper = Self {
            master,
            child,
            signals,
            outer_termios,
            initial_size: size,
        };
        wrapper.sync_window_size();
        Ok(wrapper)
    }

    fn run(self, target: ResolvedTarget) -> u8 {
        let master = self.master.as_raw_fd();
        let mut to_inner: Vec<u8> = Vec::new();
        let mut to_outer: Vec<u8> = Vec::new();
        let mut pending_reports: Vec<u8> = Vec::new();
        let mut chunk = vec![0u8; READ_CHUNK];
        let mut captured: Vec<Captured> = Vec::new();
        let mut exit_code: Option<u8> = None;
        let mut inner_closed = false;
        let mut outer_closed = false;

        let titles = match target.agent {
            Some(AgentKind::Omp) => TitleFilter::Omp,
            Some(AgentKind::Codex) => TitleFilter::Drop,
            Some(_) => TitleFilter::Despin,
            None => TitleFilter::Off,
        };
        let mut stream = OutputStream::new(titles);
        let mut reporter = Reporter::new(&target, self.child, self.initial_size.ws_col, self.initial_size.ws_row);
        reporter.initial(&mut to_outer);

        let mut last_activity = Instant::now();
        let mut last_output = Instant::now();
        let mut next_tick = Instant::now() + TICK_INTERVAL;

        while !(outer_closed || (exit_code.is_some() && inner_closed)) {
            let now = Instant::now();
            let timeout = Some(next_tick.saturating_duration_since(now));
            let read_outer = to_inner.len() < BUFFER_LIMIT;
            let read_inner = !inner_closed && to_outer.len() < BUFFER_LIMIT;

            let ready = match select_fds(
                &[
                    (STDIN, read_outer, false),
                    (master, read_inner, !to_inner.is_empty() && !inner_closed),
                    (STDOUT, false, !to_outer.is_empty()),
                    (self.signals.as_raw_fd(), true, false),
                ],
                timeout,
            ) {
                Ok(ready) => ready,
                Err(err) if err.kind() == io::ErrorKind::Interrupted => continue,
                Err(_) => break,
            };

            let now = Instant::now();
            let mut saw_pty_output = false;

            if ready[3].0 {
                let mut sigbuf = [0u8; 64];
                loop {
                    // SAFETY: Reading from the non-blocking signal self-pipe into a stack buffer.
                    let n = unsafe {
                        libc::read(
                            self.signals.as_raw_fd(),
                            sigbuf.as_mut_ptr().cast(),
                            sigbuf.len(),
                        )
                    };
                    let Ok(n) = usize::try_from(n) else { break };
                    if n == 0 {
                        break;
                    }
                    for &sig in sigbuf.iter().take(n) {
                        let sig = c_int::from(sig);
                        if sig == libc::SIGHUP || sig == libc::SIGTERM {
                            // SAFETY: Forwarding a termination signal to the child's process group.
                            unsafe { libc::kill(-self.child, sig) };
                        }
                    }
                }
                if let Some(sz) = self.sync_window_size() {
                    reporter.resize(sz.ws_col, sz.ws_row);
                }
                if exit_code.is_none() {
                    exit_code = self.reap_child();
                }
            }

            if ready[0].0 {
                match read_fd(STDIN, &mut chunk) {
                    ReadOutcome::Data(n) => {
                        let input = chunk.get(..n).unwrap_or_default();
                        if is_submit(input) {
                            reporter.user_submitted();
                        }
                        to_inner.extend_from_slice(input);
                        last_activity = now;
                    }
                    ReadOutcome::Closed => outer_closed = true,
                    ReadOutcome::Pending => {}
                }
            }

            if ready[1].0 {
                match read_fd(master, &mut chunk) {
                    ReadOutcome::Data(n) => {
                        let data = chunk.get(..n).unwrap_or_default();
                        stream.process(data, &mut to_outer, &mut captured);
                        reporter.feed_output(data, stream.take_reset());
                        for item in captured.drain(..) {
                            match item {
                                Captured::Probe(term) => {
                                    to_inner.extend_from_slice(b"\x1b]7501;?");
                                    to_inner.extend_from_slice(term.bytes());
                                }
                                Captured::Report(payload) => reporter.native_report(&payload),
                            }
                        }
                        saw_pty_output = true;
                        last_activity = now;
                        last_output = now;
                    }
                    ReadOutcome::Closed => {
                        inner_closed = true;
                        if exit_code.is_none() {
                            exit_code = self.reap_child();
                        }
                    }
                    ReadOutcome::Pending => {}
                }
            }

            if stream.stalled_in_osc_head() && now.duration_since(last_output) >= OSC_HEAD_STALL {
                stream.flush_stalled_osc(&mut to_outer);
            }

            if saw_pty_output || now >= next_tick {
                reporter.evaluate(now, &mut pending_reports);
                let interval = if now.duration_since(last_activity) < Duration::from_secs(3)
                    || reporter.current_state() == OscState::Working
                {
                    TICK_INTERVAL
                } else {
                    IDLE_TICK_INTERVAL
                };
                next_tick = now + interval;
            }
            if !pending_reports.is_empty() && stream.at_boundary() {
                to_outer.append(&mut pending_reports);
            }

            if ready[1].1 && write_fd(master, &mut to_inner).is_err() {
                inner_closed = true;
            }
            if ready[2].1 && write_fd(STDOUT, &mut to_outer).is_err() {
                outer_closed = true;
            }
        }

        let code = exit_code
            .or_else(|| self.reap_child())
            .unwrap_or(128 + libc::SIGHUP as u8);

        // Whatever the child left half-written is gone; reset the outer parser before reporting.
        if !stream.at_boundary() {
            to_outer.extend_from_slice(b"\x18");
        }
        to_outer.append(&mut pending_reports);
        let mut final_reports = Vec::new();
        reporter.finish(code, &mut final_reports);
        to_outer.append(&mut final_reports);

        if !outer_closed {
            flush_stdout(&mut to_outer);
        }
        self.restore_outer();
        code
    }

    fn reap_child(&self) -> Option<u8> {
        let mut status: c_int = 0;
        // SAFETY: WNOHANG waitpid on our child pid.
        let pid = unsafe { libc::waitpid(self.child, &mut status, libc::WNOHANG) };
        if pid != self.child {
            return None;
        }
        Some(if libc::WIFEXITED(status) {
            libc::WEXITSTATUS(status) as u8
        } else if libc::WIFSIGNALED(status) {
            128u8.wrapping_add(libc::WTERMSIG(status) as u8)
        } else {
            1
        })
    }

    fn sync_window_size(&self) -> Option<libc::winsize> {
        let size = get_window_size(STDIN)?;
        // SAFETY: Valid pty master fd and winsize struct.
        unsafe { libc::ioctl(self.master.as_raw_fd(), libc::TIOCSWINSZ, &size) };
        Some(size)
    }

    fn restore_outer(&self) {
        // SAFETY: Restores saved termios and nonblocking flags on STDIN/STDOUT.
        unsafe {
            libc::tcsetattr(STDIN, libc::TCSANOW, &self.outer_termios);
            for fd in [STDIN, STDOUT] {
                let flags = libc::fcntl(fd, libc::F_GETFL);
                if flags >= 0 {
                    libc::fcntl(fd, libc::F_SETFL, flags & !libc::O_NONBLOCK);
                }
            }
        }
    }
}

/// Enter in any keyboard encoding agents enable: plain CR/LF, kitty (`CSI 13 u`) or
/// xterm modifyOtherKeys (`CSI 27;…;13 ~`).
fn is_submit(input: &[u8]) -> bool {
    input.iter().any(|b| matches!(b, b'\r' | b'\n'))
        || contains(input, b"\x1b[13u")
        || contains(input, b"\x1b[13;")
        || contains(input, b";13~")
}

fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    haystack.windows(needle.len()).any(|w| w == needle)
}

fn to_cstring(s: &OsString) -> io::Result<CString> {
    CString::new(s.as_os_str().as_bytes())
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "argument contains NUL byte"))
}

/// # Safety
/// Must only be called in the forked child before execvp.
unsafe fn exec_child(
    slave: RawFd,
    program: *const libc::c_char,
    argv: *const *const libc::c_char,
) -> ! {
    libc::setsid();
    libc::ioctl(slave, libc::TIOCSCTTY as _, 0);
    for fd in [STDIN, STDOUT, 2] {
        libc::dup2(slave, fd);
    }
    if slave > 2 {
        libc::close(slave);
    }
    reset_signals();
    libc::execvp(program, argv);
    let msg = b"w1: execvp failed\r\n";
    libc::write(2, msg.as_ptr().cast(), msg.len());
    libc::_exit(127)
}

/// # Safety
/// Calls only async-signal-safe libc signal functions.
unsafe fn reset_signals() {
    for sig in [
        libc::SIGPIPE,
        libc::SIGCHLD,
        libc::SIGWINCH,
        libc::SIGHUP,
        libc::SIGINT,
        libc::SIGQUIT,
        libc::SIGTERM,
        libc::SIGTSTP,
        libc::SIGTTIN,
        libc::SIGTTOU,
        libc::SIGUSR1,
        libc::SIGUSR2,
        libc::SIGALRM,
    ] {
        libc::signal(sig, libc::SIG_DFL);
    }
    let mut empty = MaybeUninit::<libc::sigset_t>::uninit();
    libc::sigemptyset(empty.as_mut_ptr());
    libc::sigprocmask(libc::SIG_SETMASK, empty.as_ptr(), std::ptr::null_mut());
}

extern "C" fn on_signal(signal: c_int) {
    let fd = SIGNAL_PIPE.load(Ordering::Relaxed);
    if fd < 0 {
        return;
    }
    let saved = get_errno();
    let byte = signal as u8;
    unsafe { libc::write(fd, (&byte as *const u8).cast(), 1) };
    set_errno(saved);
}

#[cfg(target_os = "macos")]
fn get_errno() -> c_int {
    unsafe { *libc::__error() }
}

#[cfg(target_os = "macos")]
fn set_errno(val: c_int) {
    unsafe { *libc::__error() = val };
}

#[cfg(not(target_os = "macos"))]
fn get_errno() -> c_int {
    unsafe { *libc::__errno_location() }
}

#[cfg(not(target_os = "macos"))]
fn set_errno(val: c_int) {
    unsafe { *libc::__errno_location() = val };
}

fn install_signal_pipe() -> io::Result<OwnedFd> {
    let mut fds = [-1 as c_int; 2];
    if unsafe { libc::pipe(fds.as_mut_ptr()) } != 0 {
        return Err(io::Error::last_os_error());
    }
    let (read, write) = unsafe { (OwnedFd::from_raw_fd(fds[0]), OwnedFd::from_raw_fd(fds[1])) };
    for fd in [&read, &write] {
        set_cloexec(fd)?;
        set_nonblocking(fd.as_raw_fd())?;
    }
    SIGNAL_PIPE.store(
        std::os::fd::IntoRawFd::into_raw_fd(write),
        Ordering::Relaxed,
    );
    for (sig, flags) in [
        (libc::SIGWINCH, libc::SA_RESTART),
        (libc::SIGCHLD, libc::SA_RESTART | libc::SA_NOCLDSTOP),
        (libc::SIGHUP, libc::SA_RESTART),
        (libc::SIGTERM, libc::SA_RESTART),
    ] {
        unsafe {
            let mut action: libc::sigaction = std::mem::zeroed();
            action.sa_sigaction = on_signal as extern "C" fn(c_int) as usize;
            action.sa_flags = flags;
            libc::sigemptyset(&mut action.sa_mask);
            if libc::sigaction(sig, &action, std::ptr::null_mut()) != 0 {
                return Err(io::Error::last_os_error());
            }
        }
    }
    Ok(read)
}

fn get_termios(fd: RawFd) -> io::Result<libc::termios> {
    let mut attrs = MaybeUninit::<libc::termios>::uninit();
    if unsafe { libc::tcgetattr(fd, attrs.as_mut_ptr()) } != 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(unsafe { attrs.assume_init() })
}

fn get_window_size(fd: RawFd) -> Option<libc::winsize> {
    let mut size = MaybeUninit::<libc::winsize>::zeroed();
    (unsafe { libc::ioctl(fd, libc::TIOCGWINSZ, size.as_mut_ptr()) } == 0)
        .then(|| unsafe { size.assume_init() })
}

fn set_cloexec(fd: &OwnedFd) -> io::Result<()> {
    unsafe {
        let flags = libc::fcntl(fd.as_raw_fd(), libc::F_GETFD);
        if flags < 0 || libc::fcntl(fd.as_raw_fd(), libc::F_SETFD, flags | libc::FD_CLOEXEC) < 0 {
            return Err(io::Error::last_os_error());
        }
    }
    Ok(())
}

fn set_nonblocking(fd: RawFd) -> io::Result<()> {
    unsafe {
        let flags = libc::fcntl(fd, libc::F_GETFL);
        if flags < 0 || libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK) < 0 {
            return Err(io::Error::last_os_error());
        }
    }
    Ok(())
}

fn select_fds<const N: usize>(
    fds: &[(RawFd, bool, bool); N],
    timeout: Option<Duration>,
) -> io::Result<[(bool, bool); N]> {
    let mut read_set = MaybeUninit::<libc::fd_set>::uninit();
    let mut write_set = MaybeUninit::<libc::fd_set>::uninit();
    let mut highest = -1;
    unsafe {
        libc::FD_ZERO(read_set.as_mut_ptr());
        libc::FD_ZERO(write_set.as_mut_ptr());
        for &(fd, r, w) in fds {
            if fd < 0 || fd as usize >= libc::FD_SETSIZE {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "descriptor out of select range",
                ));
            }
            if r {
                libc::FD_SET(fd, read_set.as_mut_ptr());
                highest = highest.max(fd);
            }
            if w {
                libc::FD_SET(fd, write_set.as_mut_ptr());
                highest = highest.max(fd);
            }
        }
    }
    let mut tv = timeout.map(|t| libc::timeval {
        tv_sec: t.as_secs() as libc::time_t,
        tv_usec: t.subsec_micros() as libc::suseconds_t,
    });
    let tv_ptr = tv
        .as_mut()
        .map_or(std::ptr::null_mut(), |t| t as *mut libc::timeval);
    let ready = unsafe {
        libc::select(
            highest + 1,
            read_set.as_mut_ptr(),
            write_set.as_mut_ptr(),
            std::ptr::null_mut(),
            tv_ptr,
        )
    };
    if ready < 0 {
        return Err(io::Error::last_os_error());
    }
    let mut out = [(false, false); N];
    for (slot, &(fd, r, w)) in out.iter_mut().zip(fds.iter()) {
        unsafe {
            *slot = (
                r && libc::FD_ISSET(fd, read_set.as_ptr()),
                w && libc::FD_ISSET(fd, write_set.as_ptr()),
            );
        }
    }
    Ok(out)
}

enum ReadOutcome {
    Data(usize),
    Closed,
    Pending,
}

fn read_fd(fd: RawFd, buf: &mut [u8]) -> ReadOutcome {
    let n = unsafe { libc::read(fd, buf.as_mut_ptr().cast(), buf.len()) };
    if n > 0 {
        return ReadOutcome::Data(n as usize);
    }
    if n == 0 {
        return ReadOutcome::Closed;
    }
    match io::Error::last_os_error().kind() {
        io::ErrorKind::WouldBlock | io::ErrorKind::Interrupted => ReadOutcome::Pending,
        _ => ReadOutcome::Closed,
    }
}

fn write_fd(fd: RawFd, buf: &mut Vec<u8>) -> io::Result<()> {
    if buf.is_empty() {
        return Ok(());
    }
    let written = unsafe { libc::write(fd, buf.as_ptr().cast(), buf.len()) };
    if written >= 0 {
        buf.drain(..written as usize);
        return Ok(());
    }
    let err = io::Error::last_os_error();
    match err.kind() {
        io::ErrorKind::WouldBlock | io::ErrorKind::Interrupted => Ok(()),
        _ => Err(err),
    }
}

fn flush_stdout(buf: &mut Vec<u8>) {
    let deadline = Instant::now() + FINAL_FLUSH_TIMEOUT;
    while !buf.is_empty() {
        let now = Instant::now();
        if now >= deadline {
            return;
        }
        match select_fds(&[(STDOUT, false, true)], Some(deadline - now)) {
            Ok(ready) if ready[0].1 => {
                if write_fd(STDOUT, buf).is_err() {
                    return;
                }
            }
            Ok(_) => return,
            Err(err) if err.kind() == io::ErrorKind::Interrupted => {}
            Err(_) => return,
        }
    }
}
