//! Debug-only process driver for the actual receipt-only distribution composition.
#[cfg(all(feature = "test-failpoints", debug_assertions))]
#[tokio::main]
async fn main() {
    use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
    use craxii_server::ports::state_store::BootstrapStateStore;
    use craxii_server::{
        distribution::startup::{DistributionConfig, DistributionRuntime},
        worker_auth::{
            client::WorkerClient,
            key_store::KeyStore,
            state::{Disposition, IdentityState},
        },
    };
    use serde_json::{Value, json};
    use std::{
        io::{Read, Write},
        path::Path,
    };
    let run = async {
        let mut bytes = Vec::new();
        std::io::stdin()
            .take(131073)
            .read_to_end(&mut bytes)
            .map_err(|_| ())?;
        let input = craxii_server::distribution_contracts::raw_json::parse_bounded(
            &bytes,
            craxii_server::distribution_contracts::raw_json::Limits {
                bytes: 131072,
                nodes: 4096,
                depth: 32,
            },
        )
        .map_err(|_| ())?;
        let text = |k: &str| input[k].as_str().ok_or(());
        let uid = unsafe { nix::libc::geteuid() };
        let key = KeyStore::isolated_fixture(
            Path::new(text("store")?),
            uid,
            uid + 10000,
            input["first_enrollment"] == true,
        )
        .map_err(|_| ())?;
        let state = IdentityState::load(&key)
            .map_err(|_| ())?
            .unwrap_or(IdentityState {
                version: 1,
                installation_id: text("installation_id")?.into(),
                customer_id: text("customer_id")?.into(),
                logical_agent_id: text("logical_agent_id")?.into(),
                generation: text("generation")?.into(),
                key_id: key.key_id(),
                highest_epoch: "1".into(),
                binding_id: None,
                pending_signed: None,
                disposition: Disposition::Unbound,
            });
        let config: DistributionConfig =
            serde_json::from_value(input["config"].clone()).map_err(|_| ())?;
        let runtime = DistributionRuntime::isolated_fixture(config, text("boot_id")?.into())
            .await
            .map_err(|e| {
                eprintln!("distribution_fixture_startup_{e:?}");
                ()
            })?;
        let mut client = WorkerClient::isolated_tls_fixture(
            text("origin")?,
            key,
            state,
            text("ca_pem")?.as_bytes(),
        )
        .map_err(|_| ())?;
        if input["action"] == "enroll" {
            client
                .negotiate(
                    &runtime.hello(),
                    Some((text("enrollment_id")?, text("credential")?)),
                )
                .await
                .map_err(|_| ())?;
            client
                .enroll(
                    text("enrollment_id")?,
                    text("credential")?,
                    text("expected_instance_ref")?,
                    &URL_SAFE_NO_PAD.decode(text("document")?).map_err(|_| ())?,
                    &URL_SAFE_NO_PAD.decode(text("signature")?).map_err(|_| ())?,
                )
                .await
                .map_err(|_| ())?;
            client.renew().await.map_err(|_| ())?;
            client.status().await.map_err(|_| ())?;
        } else {
            runtime.reconnect(&mut client).await.map_err(|e| {
                eprintln!("distribution_fixture_reconnect_{e:?}");
                ()
            })?;
        }
        if input["action"] == "commit" {
            runtime
                .isolated_commit_without_ack(&mut client)
                .await
                .map_err(|_| ())?;
            std::io::stdout()
                .write_all(b"distribution_local_commit\n")
                .map_err(|_| ())?;
            std::io::stdout().flush().map_err(|_| ())?;
            std::future::pending::<()>().await;
        } else if input["action"] != "enroll" {
            runtime
                .transfer_pending(&mut client)
                .await
                .map_err(|_| ())?;
            runtime.poll_once(&mut client).await.map_err(|_| ())?;
        }
        runtime
            .store
            .verify_application_consistency()
            .await
            .map_err(|_| ())?;
        runtime
            .store
            .verify_distribution_consistency()
            .await
            .map_err(|_| ())?;
        let pending = runtime
            .store
            .pending_distribution_receipts()
            .await
            .map_err(|_| ())?
            .len();
        let controls = runtime
            .store
            .pending_distribution_controls()
            .await
            .map_err(|_| ())?
            .len();
        let result = json!({"key_id":client.key_id(),"pending_receipts":pending,"pending_controls":controls,"execution_enabled":false});
        runtime.shutdown().await.map_err(|_| ())?;
        Ok::<Value, ()>(result)
    };
    match run.await {
        Ok(v) => {
            std::io::stdout()
                .write_all(&serde_json::to_vec(&v).unwrap())
                .unwrap();
        }
        Err(()) => {
            eprintln!("worker_distribution_fixture_failed");
            std::process::exit(1);
        }
    }
}
#[cfg(not(all(feature = "test-failpoints", debug_assertions)))]
fn main() {
    eprintln!("worker_distribution_fixture_unavailable");
    std::process::exit(1);
}
