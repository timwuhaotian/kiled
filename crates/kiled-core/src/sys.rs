//! The crate's only unsafe code: Darwin calls the standard library doesn't offer, for
//! ending a language server and everything it started (DESIGN.md §6.3), and for a saved
//! file to keep its ACL and extended attributes (§4.4).

use std::{fs::File, io, os::fd::AsRawFd};

/// Sends `signal` to every process in the group led by `leader`.
pub(crate) fn kill_group(leader: u32, signal: i32) -> io::Result<()> {
    let group = libc::pid_t::try_from(leader).map_err(io::Error::other)?;
    // SAFETY: killpg takes two integers and touches no memory of ours.
    #[expect(unsafe_code, reason = "Darwin FFI")]
    let result = unsafe { libc::killpg(group, signal) };
    if result == 0 {
        Ok(())
    } else {
        Err(io::Error::last_os_error())
    }
}

/// Whether child `pid` has exited, without reaping it: until it is reaped its pid, and so
/// its process group id, can't be given to another process.
pub(crate) fn exited(pid: u32) -> bool {
    let id: libc::id_t = pid;
    let mut info = std::mem::MaybeUninit::<libc::siginfo_t>::zeroed();
    // SAFETY: `info` is a valid, zeroed siginfo_t for waitid to fill in; WNOWAIT leaves the
    // child unreaped, WNOHANG makes the call return at once.
    #[expect(unsafe_code, reason = "Darwin FFI")]
    let result = unsafe {
        libc::waitid(
            libc::P_PID,
            id,
            info.as_mut_ptr(),
            libc::WEXITED | libc::WNOHANG | libc::WNOWAIT,
        )
    };
    if result != 0 {
        // ECHILD: no such child, so nothing is left to wait for.
        return true;
    }
    // SAFETY: zeroed and then possibly filled in by waitid; every field is plain data.
    #[expect(unsafe_code, reason = "Darwin FFI")]
    let info = unsafe { info.assume_init() };
    info.si_pid != 0
}

/// Gives `to` the extended attributes of `from`, leaving its contents, mode and times alone.
pub(crate) fn copy_attributes(from: &File, to: &File) -> io::Result<()> {
    copy_metadata(from, to, libc::COPYFILE_XATTR)
}

/// Gives `to` the ACL of `from`.
pub(crate) fn copy_acl(from: &File, to: &File) -> io::Result<()> {
    copy_metadata(from, to, libc::COPYFILE_ACL)
}

fn copy_metadata(from: &File, to: &File, what: libc::copyfile_flags_t) -> io::Result<()> {
    // SAFETY: both descriptors stay open while borrowed here, and a null state asks
    // fcopyfile to keep none of its own.
    #[expect(unsafe_code, reason = "Darwin FFI")]
    let result =
        unsafe { libc::fcopyfile(from.as_raw_fd(), to.as_raw_fd(), std::ptr::null_mut(), what) };
    if result == 0 {
        Ok(())
    } else {
        Err(io::Error::last_os_error())
    }
}
