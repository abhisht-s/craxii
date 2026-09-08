//! Fixed privileged launcher for the Linux Stage 27 workstation identity boundary.

#[cfg(target_os = "linux")]
mod linux {
    use std::ffi::{CStr, OsString};
    use std::os::unix::process::CommandExt as _;
    use std::path::PathBuf;
    use std::process::{Command, ExitCode};

    use nix::libc;

    const SERVER_USER: &CStr = c"craxii-server";
    const WORKSTATION_USER: &CStr = c"craxii";
    const WORKSTATION_GROUP: &CStr = c"craxii";
    const BASH_PATH: &str = "/bin/bash";
    const USER_HOME: &str = "/home/craxii";
    const USER_PATH: &str = "/home/craxii/.local/bin:/home/craxii/.cargo/bin:/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin";
    const FAILURE: ExitCode = ExitCode::FAILURE;

    #[repr(C)]
    struct CapabilityHeader {
        version: u32,
        pid: i32,
    }

    #[repr(C)]
    #[derive(Clone, Copy)]
    struct CapabilityData {
        effective: u32,
        permitted: u32,
        inheritable: u32,
    }

    #[derive(Clone, Copy)]
    struct Identity {
        uid: libc::uid_t,
        gid: libc::gid_t,
    }

    enum Operation {
        Shell {
            work_id: OsString,
            workspace_id: OsString,
            command: OsString,
        },
        ReadFile {
            path: OsString,
            maximum_bytes: OsString,
            expected_device: OsString,
            expected_inode: OsString,
        },
    }

    pub(super) fn main() -> ExitCode {
        run().unwrap_or(FAILURE)
    }

    fn run() -> Result<ExitCode, ()> {
        let operation = parse_operation()?;
        // Resolve the adjacent trusted reader while the setuid image is still executable by its
        // tightly restricted installation group. The resulting path is executed only after every
        // UID/GID/capability drop below.
        let reader = matches!(&operation, Operation::ReadFile { .. })
            .then(adjacent_reader)
            .transpose()?;
        let server = lookup_user(SERVER_USER)?;
        let workstation = lookup_user(WORKSTATION_USER)?;
        let workstation_group = lookup_group(WORKSTATION_GROUP)?;
        if workstation.uid == 0
            || workstation.uid == server.uid
            || workstation.gid != workstation_group
        {
            return Err(());
        }

        // The execute bit is restricted to the craxii-server group, but the real-UID check is the
        // authoritative caller guard. The helper must obtain effective root only from its fixed
        // root-owned setuid installation.
        if unsafe { libc::getuid() } != server.uid || unsafe { libc::geteuid() } != 0 {
            return Err(());
        }
        mark_unrelated_descriptors_close_on_exec()?;
        drop_to_workstation(workstation)?;

        let error = match operation {
            Operation::Shell {
                work_id,
                workspace_id,
                command,
            } => {
                // Re-resolving `.` after the UID drop proves the model identity can search the
                // already-pinned cwd. A server-only directory cannot be smuggled in through the
                // pre-exec fchdir performed by LocalWorkstation.
                if unsafe { libc::chdir(c".".as_ptr()) } != 0 {
                    return Err(());
                }
                Command::new(BASH_PATH)
                    .env_clear()
                    .env("HOME", USER_HOME)
                    .env("USER", "craxii")
                    .env("LOGNAME", "craxii")
                    .env("SHELL", BASH_PATH)
                    .env("LANG", "C.UTF-8")
                    .env("PATH", USER_PATH)
                    .env("CRAXII_WORK_ID", work_id)
                    .env("CRAXII_WORKSPACE_ID", workspace_id)
                    .arg("--noprofile")
                    .arg("--norc")
                    .arg("-o")
                    .arg("pipefail")
                    .arg("-c")
                    .arg(command)
                    .exec()
            }
            Operation::ReadFile {
                path,
                maximum_bytes,
                expected_device,
                expected_inode,
            } => {
                let reader = reader.ok_or(())?;
                Command::new(reader)
                    .env_clear()
                    .env("HOME", USER_HOME)
                    .env("USER", "craxii")
                    .env("LOGNAME", "craxii")
                    .env("SHELL", BASH_PATH)
                    .env("LANG", "C.UTF-8")
                    .env("PATH", USER_PATH)
                    .arg(path)
                    .arg(maximum_bytes)
                    .arg(expected_device)
                    .arg(expected_inode)
                    .exec()
            }
        };
        let _ = error;
        Err(())
    }

    fn parse_operation() -> Result<Operation, ()> {
        let mut arguments = std::env::args_os();
        let _program = arguments.next().ok_or(())?;
        match arguments.next().and_then(|value| value.into_string().ok()) {
            Some(mode) if mode == "shell" => {
                let work_id = arguments.next().ok_or(())?;
                let workspace_id = arguments.next().ok_or(())?;
                let command = arguments.next().ok_or(())?;
                if arguments.next().is_some() {
                    return Err(());
                }
                Ok(Operation::Shell {
                    work_id,
                    workspace_id,
                    command,
                })
            }
            Some(mode) if mode == "read-file" => {
                let path = arguments.next().ok_or(())?;
                let maximum_bytes = arguments.next().ok_or(())?;
                let expected_device = arguments.next().ok_or(())?;
                let expected_inode = arguments.next().ok_or(())?;
                if arguments.next().is_some() {
                    return Err(());
                }
                Ok(Operation::ReadFile {
                    path,
                    maximum_bytes,
                    expected_device,
                    expected_inode,
                })
            }
            _ => Err(()),
        }
    }

    fn adjacent_reader() -> Result<PathBuf, ()> {
        let executable =
            std::fs::canonicalize(std::env::current_exe().map_err(|_| ())?).map_err(|_| ())?;
        let parent = executable.parent().ok_or(())?;
        Ok(parent.join("craxii-workstation-reader"))
    }

    fn lookup_user(name: &CStr) -> Result<Identity, ()> {
        let mut record = std::mem::MaybeUninit::<libc::passwd>::uninit();
        let mut result = std::ptr::null_mut();
        let mut buffer = vec![0_u8; 16 * 1024];
        let status = unsafe {
            libc::getpwnam_r(
                name.as_ptr(),
                record.as_mut_ptr(),
                buffer.as_mut_ptr().cast(),
                buffer.len(),
                &mut result,
            )
        };
        if status != 0 || result.is_null() {
            return Err(());
        }
        let record = unsafe { record.assume_init() };
        Ok(Identity {
            uid: record.pw_uid,
            gid: record.pw_gid,
        })
    }

    fn lookup_group(name: &CStr) -> Result<libc::gid_t, ()> {
        let mut record = std::mem::MaybeUninit::<libc::group>::uninit();
        let mut result = std::ptr::null_mut();
        let mut buffer = vec![0_u8; 16 * 1024];
        let status = unsafe {
            libc::getgrnam_r(
                name.as_ptr(),
                record.as_mut_ptr(),
                buffer.as_mut_ptr().cast(),
                buffer.len(),
                &mut result,
            )
        };
        if status != 0 || result.is_null() {
            return Err(());
        }
        Ok(unsafe { record.assume_init() }.gr_gid)
    }

    fn drop_to_workstation(identity: Identity) -> Result<(), ()> {
        if unsafe { libc::setgroups(0, std::ptr::null()) } != 0 {
            return Err(());
        }
        if unsafe {
            libc::prctl(
                libc::PR_CAP_AMBIENT,
                libc::PR_CAP_AMBIENT_CLEAR_ALL,
                0,
                0,
                0,
            )
        } != 0
        {
            return Err(());
        }
        if unsafe { libc::prctl(libc::PR_SET_NO_NEW_PRIVS, 1, 0, 0, 0) } != 0 {
            return Err(());
        }
        if unsafe { libc::setresgid(identity.gid, identity.gid, identity.gid) } != 0 {
            return Err(());
        }
        if unsafe { libc::setresuid(identity.uid, identity.uid, identity.uid) } != 0 {
            return Err(());
        }
        clear_capabilities()?;

        let mut real_uid = 0;
        let mut effective_uid = 0;
        let mut saved_uid = 0;
        let mut real_gid = 0;
        let mut effective_gid = 0;
        let mut saved_gid = 0;
        if unsafe { libc::getresuid(&mut real_uid, &mut effective_uid, &mut saved_uid) } != 0
            || unsafe { libc::getresgid(&mut real_gid, &mut effective_gid, &mut saved_gid) } != 0
            || [real_uid, effective_uid, saved_uid] != [identity.uid; 3]
            || [real_gid, effective_gid, saved_gid] != [identity.gid; 3]
            || unsafe { libc::getgroups(0, std::ptr::null_mut()) } != 0
        {
            return Err(());
        }
        Ok(())
    }

    fn clear_capabilities() -> Result<(), ()> {
        const LINUX_CAPABILITY_VERSION_3: u32 = 0x2008_0522;
        let mut header = CapabilityHeader {
            version: LINUX_CAPABILITY_VERSION_3,
            pid: 0,
        };
        let data = [
            CapabilityData {
                effective: 0,
                permitted: 0,
                inheritable: 0,
            },
            CapabilityData {
                effective: 0,
                permitted: 0,
                inheritable: 0,
            },
        ];
        let result = unsafe {
            libc::syscall(
                libc::SYS_capset,
                std::ptr::addr_of_mut!(header),
                data.as_ptr(),
            )
        };
        (result == 0).then_some(()).ok_or(())
    }

    fn mark_unrelated_descriptors_close_on_exec() -> Result<(), ()> {
        const CLOSE_RANGE_CLOEXEC: libc::c_uint = 1 << 2;
        let closed =
            unsafe { libc::syscall(libc::SYS_close_range, 3_u32, u32::MAX, CLOSE_RANGE_CLOEXEC) };
        if closed == 0 {
            return Ok(());
        }
        let descriptors = std::fs::read_dir("/proc/self/fd").map_err(|_| ())?;
        for descriptor in descriptors {
            let descriptor = descriptor
                .map_err(|_| ())?
                .file_name()
                .into_string()
                .map_err(|_| ())?
                .parse::<libc::c_int>()
                .map_err(|_| ())?;
            if descriptor <= 2 {
                continue;
            }
            let flags = unsafe { libc::fcntl(descriptor, libc::F_GETFD) };
            if flags == -1
                || unsafe { libc::fcntl(descriptor, libc::F_SETFD, flags | libc::FD_CLOEXEC) } == -1
            {
                return Err(());
            }
        }
        Ok(())
    }
}

#[cfg(target_os = "linux")]
fn main() -> std::process::ExitCode {
    linux::main()
}

#[cfg(not(target_os = "linux"))]
fn main() -> std::process::ExitCode {
    std::process::ExitCode::FAILURE
}
