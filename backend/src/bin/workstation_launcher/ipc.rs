//! Installed tool IPC policy. Applied after privilege drop, before any untrusted instruction.
//! Filesystem Landlock does not mediate Unix socket connections. Deny their creation instead,
//! including socketpair (a datagram pair can be reconnected), and alternate FD acquisition.
use nix::libc;

/// The installed guest is x86_64 only. Reject compat/x32 ABIs rather than interpreting their
/// syscall numbers as native calls. No optional policy file or environment switch exists.
#[cfg(target_arch = "x86_64")]
pub fn restrict() -> Result<(), ()> {
    const LD: u16 = 0x20; // BPF_LD | BPF_W | BPF_ABS
    const EQ: u16 = 0x15; // BPF_JMP | BPF_JEQ | BPF_K
    const GE: u16 = 0x35; // BPF_JMP | BPF_JGE | BPF_K
    const RET: u16 = 0x06;
    const ALLOW: u32 = 0x7fff_0000;
    const DENY: u32 = 0x0005_0000 | libc::EPERM as u32;
    const fn op(code: u16, jt: u8, jf: u8, k: u32) -> libc::sock_filter {
        libc::sock_filter { code, jt, jf, k }
    }
    let filter = [
        op(LD, 0, 0, 4),           // seccomp_data.arch
        op(EQ, 1, 0, 0xc000_003e), // AUDIT_ARCH_X86_64
        op(RET, 0, 0, DENY), // no kill/trap/log action that could itself feed kernel audit logs
        op(LD, 0, 0, 0),     // seccomp_data.nr
        op(GE, 0, 1, 0x4000_0000), // x32 and invalid negative syscall numbers
        op(RET, 0, 0, DENY),
        op(EQ, 0, 1, libc::SYS_io_uring_setup as u32),
        op(RET, 0, 0, DENY),
        op(EQ, 0, 1, libc::SYS_pidfd_getfd as u32),
        op(RET, 0, 0, DENY),
        // A tool must not install RET_LOG/LOG-flag filters and feed privileged audit logging.
        // These APIs are denied only after this fixed policy has been installed.
        op(EQ, 0, 1, libc::SYS_seccomp as u32),
        op(RET, 0, 0, DENY),
        op(EQ, 8, 0, libc::SYS_prctl as u32),
        op(EQ, 2, 0, libc::SYS_socket as u32),
        op(EQ, 1, 0, libc::SYS_socketpair as u32),
        op(RET, 0, 0, ALLOW),
        op(LD, 0, 0, 16), // low word of seccomp_data.args[0]; kernel domain is int
        op(EQ, 2, 0, libc::AF_INET as u32),
        op(EQ, 1, 0, libc::AF_INET6 as u32),
        op(RET, 0, 0, DENY),
        op(RET, 0, 0, ALLOW),
        op(LD, 0, 0, 16), // prctl option, also an int
        op(EQ, 0, 1, libc::PR_SET_SECCOMP as u32),
        op(RET, 0, 0, DENY),
        op(RET, 0, 0, ALLOW),
    ];
    let program = libc::sock_fprog {
        len: filter.len() as u16,
        filter: filter.as_ptr().cast_mut(),
    };
    // Launcher is single-threaded and already NNP with no capabilities. Failure is fatal.
    if unsafe { libc::prctl(libc::PR_GET_NO_NEW_PRIVS, 0, 0, 0, 0) } != 1
        || unsafe { libc::syscall(libc::SYS_seccomp, 1, 0, &program) } != 0
        || unsafe { libc::prctl(libc::PR_GET_SECCOMP, 0, 0, 0, 0) } != 2
    {
        return Err(());
    }
    Ok(())
}

#[cfg(not(target_arch = "x86_64"))]
pub fn restrict() -> Result<(), ()> {
    Err(())
}

/// A caller must not supply journald/socket-activation/uring descriptors as tool stdio.
/// Higher descriptors are all CLOEXEC before bash, closing SCM_RIGHTS and inherited-ring paths.
pub fn verify_stdio() -> Result<(), ()> {
    for fd in 0..=2 {
        let mut info = std::mem::MaybeUninit::<libc::stat>::uninit();
        if unsafe { libc::fstat(fd, info.as_mut_ptr()) } != 0 {
            return Err(());
        }
        let info = unsafe { info.assume_init() };
        let valid = if fd == 0 {
            info.st_mode & libc::S_IFMT == libc::S_IFCHR && info.st_rdev == libc::makedev(1, 3) // /dev/null
        } else {
            info.st_mode & libc::S_IFMT == libc::S_IFIFO
        };
        if !valid {
            return Err(());
        }
    }
    Ok(())
}

/// Startup uses the same compiled policy and privilege transition as real guest-shell.
/// This fixed operation accepts no command or path and grants no execution authority.
pub fn probe() -> Result<(), ()> {
    restrict()?;
    let fd = unsafe { libc::socket(libc::AF_UNIX, libc::SOCK_DGRAM, 0) };
    if fd != -1 || std::io::Error::last_os_error().raw_os_error() != Some(libc::EPERM) {
        return Err(());
    }
    let mut pair = [-1; 2];
    if unsafe { libc::socketpair(libc::AF_UNIX, libc::SOCK_DGRAM, 0, pair.as_mut_ptr()) } != -1
        || std::io::Error::last_os_error().raw_os_error() != Some(libc::EPERM)
    {
        return Err(());
    }
    for family in [libc::AF_INET, libc::AF_INET6] {
        let fd = unsafe { libc::socket(family, libc::SOCK_STREAM | libc::SOCK_CLOEXEC, 0) };
        if fd < 0 {
            return Err(());
        }
        unsafe { libc::close(fd) };
    }
    Ok(())
}
