use std::process::ExitCode;

#[tokio::main(flavor = "multi_thread")]
async fn main() -> ExitCode {
    if std::env::args().nth(1).as_deref() == Some("distribution") {
        return match craxii_server::distribution::startup::run_protected().await {
            Ok(()) => ExitCode::SUCCESS,
            Err(_) => {
                eprintln!("distribution_unavailable");
                ExitCode::FAILURE
            }
        };
    }
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
