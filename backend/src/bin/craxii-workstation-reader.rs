//! Fixed bounded file reader executed only after the Linux workstation identity drop.

#[cfg(target_os = "linux")]
mod linux {
    use std::io::{Read as _, Write as _};
    use std::os::unix::fs::{MetadataExt as _, OpenOptionsExt as _};
    use std::path::PathBuf;
    use std::process::ExitCode;

    use nix::libc;

    const EXIT_USAGE: u8 = 64;
    const EXIT_INVALID: u8 = 65;
    const EXIT_NOT_FOUND: u8 = 66;
    const EXIT_PERMISSION: u8 = 67;
    const EXIT_TOO_LARGE: u8 = 68;
    const EXIT_CHANGED: u8 = 69;
    const EXIT_IO: u8 = 74;

    pub(super) fn main() -> ExitCode {
        ExitCode::from(match run() {
            Ok(code) | Err(code) => code,
        })
    }

    fn run() -> Result<u8, u8> {
        let mut arguments = std::env::args_os();
        let _program = arguments.next().ok_or(EXIT_USAGE)?;
        let path = PathBuf::from(arguments.next().ok_or(EXIT_USAGE)?);
        let maximum_bytes = parse_u64(arguments.next().ok_or(EXIT_USAGE)?)?;
        let expected_device = parse_u64(arguments.next().ok_or(EXIT_USAGE)?)?;
        let expected_inode = parse_u64(arguments.next().ok_or(EXIT_USAGE)?)?;
        if maximum_bytes == 0 || arguments.next().is_some() {
            return Err(EXIT_USAGE);
        }

        let mut options = std::fs::OpenOptions::new();
        options
            .read(true)
            .custom_flags(libc::O_CLOEXEC | libc::O_NOFOLLOW | libc::O_NONBLOCK);
        let mut file = options.open(path).map_err(map_io)?;
        let initial = file.metadata().map_err(map_io)?;
        if !initial.is_file() {
            return Err(EXIT_INVALID);
        }
        if initial.dev() != expected_device || initial.ino() != expected_inode {
            return Err(EXIT_CHANGED);
        }
        if initial.len() > maximum_bytes {
            let _ = writeln!(std::io::stderr().lock(), "{}", initial.len());
            return Err(EXIT_TOO_LARGE);
        }

        let capacity = usize::try_from(initial.len()).map_err(|_| EXIT_TOO_LARGE)?;
        let mut bytes = Vec::with_capacity(capacity);
        let mut limited = std::io::Read::by_ref(&mut file).take(maximum_bytes.saturating_add(1));
        limited.read_to_end(&mut bytes).map_err(map_io)?;
        let final_metadata = file.metadata().map_err(map_io)?;
        if bytes.len() as u64 > maximum_bytes
            || initial.dev() != final_metadata.dev()
            || initial.ino() != final_metadata.ino()
            || initial.len() != final_metadata.len()
            || initial.mode() != final_metadata.mode()
            || initial.mtime() != final_metadata.mtime()
            || initial.mtime_nsec() != final_metadata.mtime_nsec()
            || bytes.len() as u64 != initial.len()
        {
            return Err(EXIT_CHANGED);
        }
        std::io::stdout().lock().write_all(&bytes).map_err(map_io)?;
        Ok(0)
    }

    fn parse_u64(value: std::ffi::OsString) -> Result<u64, u8> {
        value
            .into_string()
            .map_err(|_| EXIT_USAGE)?
            .parse()
            .map_err(|_| EXIT_USAGE)
    }

    fn map_io(error: std::io::Error) -> u8 {
        match error.kind() {
            std::io::ErrorKind::NotFound => EXIT_NOT_FOUND,
            std::io::ErrorKind::PermissionDenied => EXIT_PERMISSION,
            _ => EXIT_IO,
        }
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
