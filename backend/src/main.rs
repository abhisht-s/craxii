use std::process::ExitCode;

#[tokio::main(flavor = "multi_thread")]
async fn main() -> ExitCode {
    match craxii_server::bootstrap::startup::run_from_env().await {
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
