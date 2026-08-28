use std::process::ExitCode;

#[tokio::main(flavor = "multi_thread")]
async fn main() -> ExitCode {
    match craxii_server::bootstrap::startup::run_from_env().await {
        Ok(mut application) => {
            if let Err(error) = application.wait_for_shutdown_request().await {
                let _ = application.shutdown().await;
                let _ = craxii_server::bootstrap::startup::write_fatal_diagnostic(
                    &mut std::io::stderr().lock(),
                    &error,
                );
                return ExitCode::FAILURE;
            }
            match application.shutdown().await {
                Ok(()) => ExitCode::SUCCESS,
                Err(error) => {
                    let _ = craxii_server::bootstrap::startup::write_fatal_diagnostic(
                        &mut std::io::stderr().lock(),
                        &error,
                    );
                    ExitCode::FAILURE
                }
            }
        }
        Err(error) => {
            let _ = craxii_server::bootstrap::startup::write_fatal_diagnostic(
                &mut std::io::stderr().lock(),
                &error,
            );
            ExitCode::FAILURE
        }
    }
}
