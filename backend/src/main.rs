use std::process::ExitCode;

fn main() -> ExitCode {
    match craxii_server::bootstrap::startup::run_from_env() {
        Ok(_application) => ExitCode::SUCCESS,
        Err(error) => {
            let _ = craxii_server::bootstrap::startup::write_fatal_diagnostic(
                &mut std::io::stderr().lock(),
                &error,
            );
            ExitCode::FAILURE
        }
    }
}
