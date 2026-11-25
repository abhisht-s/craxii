//! Fixed offline validator / installed read-only guard. No arbitrary path or fixture selector.
use craxii_server::guest_boundary::{config::GuestConfig, storage};
use std::{
    io::{Read, Write},
    process::ExitCode,
};
fn main() -> ExitCode {
    let args: Vec<_> = std::env::args().skip(1).collect();
    let result = match args.as_slice() {
        [arg] if arg == "validate" => {
            let mut bytes = Vec::new();
            std::io::stdin()
                .take(16385)
                .read_to_end(&mut bytes)
                .map_err(|_| craxii_server::guest_boundary::BoundaryError::GuestConfigInvalid)
                .and_then(|_| GuestConfig::decode(&bytes))
                .map(|_| ())
        }
        [arg] if arg == "verify" => {
            let principal = if unsafe { nix::libc::geteuid() } == 0 {
                Ok(())
            } else {
                storage::harden_worker_process()
            };
            principal
                .and_then(|_| storage::load_installed())
                .and_then(|c| storage::verify_installed(&c))
        }
        _ => Err(craxii_server::guest_boundary::BoundaryError::GuestConfigInvalid),
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            let _ = writeln!(std::io::stderr(), "{}", e.code());
            ExitCode::FAILURE
        }
    }
}
