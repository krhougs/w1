//! Process-tree lookups used to attribute an agent's on-disk session to the child `w1` spawned.

const MAX_DEPTH: usize = 32;

#[cfg(target_os = "macos")]
pub fn parent_pid(pid: i32) -> Option<i32> {
    if pid <= 0 {
        return None;
    }
    let mut info = std::mem::MaybeUninit::<libc::proc_bsdinfo>::zeroed();
    let size = std::mem::size_of::<libc::proc_bsdinfo>() as libc::c_int;
    // SAFETY: proc_pidinfo writes at most `size` bytes into `info`.
    let written = unsafe {
        libc::proc_pidinfo(
            pid,
            libc::PROC_PIDTBSDINFO,
            0,
            info.as_mut_ptr().cast(),
            size,
        )
    };
    if written != size {
        return None;
    }
    // SAFETY: The kernel filled the whole struct (checked above).
    let info = unsafe { info.assume_init() };
    i32::try_from(info.pbi_ppid).ok()
}

#[cfg(not(target_os = "macos"))]
pub fn parent_pid(pid: i32) -> Option<i32> {
    if pid <= 0 {
        return None;
    }
    let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    // `pid (comm) state ppid ...`; comm may contain spaces and parentheses.
    let rest = stat.get(stat.rfind(')')? + 1..)?;
    rest.split_whitespace().nth(1)?.parse().ok()
}

/// Whether `pid` is `ancestor` itself or one of its descendants.
pub fn is_descendant_of(pid: i32, ancestor: i32) -> bool {
    let mut current = pid;
    for _ in 0..MAX_DEPTH {
        if current == ancestor {
            return true;
        }
        match parent_pid(current) {
            Some(parent) if parent > 1 && parent != current => current = parent,
            _ => return false,
        }
    }
    false
}
