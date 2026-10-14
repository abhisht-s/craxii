//! Debug-only authentication process for isolated verified-TLS interoperability tests.
#[cfg(all(feature = "test-failpoints", debug_assertions))]
#[tokio::main]
async fn main() {
    use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
    use craxii_server::worker_auth::{
        client::WorkerClient,
        key_store::KeyStore,
        state::{Disposition, IdentityState},
    };
    use serde_json::{Value, json};
    use std::io::{Read, Write};
    use std::path::Path;
    let run = async {
        let mut bytes = Vec::new();
        std::io::stdin()
            .take(131073)
            .read_to_end(&mut bytes)
            .map_err(|_| ())?;
        if bytes.len() > 131072 {
            return Err(());
        }
        let input: Value = serde_json::from_slice(&bytes).map_err(|_| ())?;
        let text = |k: &str| input[k].as_str().ok_or(());
        // SAFETY: read-only process identity query.
        let uid = unsafe { nix::libc::geteuid() };
        let key = KeyStore::isolated_fixture(
            Path::new(text("store")?),
            uid,
            uid + 10000,
            input["first_enrollment"] == true,
        )
        .map_err(|_| ())?;
        if input["action"] == "identity" {
            return Ok(json!({"public_key":key.public_key(),"key_id":key.key_id()}));
        }
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
        let mut client = WorkerClient::isolated_tls_fixture(
            text("origin")?,
            key,
            state,
            text("ca_pem")?.as_bytes(),
        )
        .map_err(|_| ())?;
        let enrollment = if input["action"] == "enroll" {
            Some((text("enrollment_id")?, text("credential")?))
        } else {
            None
        };

        let resumed = if input["action"] == "resume" {
            let doc = URL_SAFE_NO_PAD.decode(text("document")?).map_err(|_| ())?;
            let sig = URL_SAFE_NO_PAD.decode(text("signature")?).map_err(|_| ())?;
            Some(
                client
                    .resume_enrollment(&doc, &sig, None)
                    .await
                    .map_err(|_| ())?,
            )
        } else {
            None
        };
        client
            .negotiate(&input["hello"], enrollment)
            .await
            .map_err(|_| ())?;

        let ack = if let Some((id, credential)) = enrollment {
            let doc = URL_SAFE_NO_PAD.decode(text("document")?).map_err(|_| ())?;
            let sig = URL_SAFE_NO_PAD.decode(text("signature")?).map_err(|_| ())?;
            Some(
                client
                    .enroll(id, credential, text("expected_instance_ref")?, &doc, &sig)
                    .await
                    .map_err(|_| ())?,
            )
        } else {
            resumed
        };

        client.renew().await.map_err(|_| ())?;
        let status = client.status().await.map_err(|_| ())?;

        client.renew().await.map_err(|_| ())?;
        let renewed = client.status().await.map_err(|_| ())?;
        Ok::<Value, ()>(
            json!({"ack":ack,"status":status,"renewed":renewed,"key_id":client.key_id()}),
        )
    };
    match run.await {
        Ok(value) => {
            let bytes = serde_json::to_vec(&value).unwrap();
            std::io::stdout().write_all(&bytes).unwrap();
        }
        Err(()) => {
            eprintln!("worker_auth_fixture_failed");
            std::process::exit(1)
        }
    }
}
#[cfg(not(all(feature = "test-failpoints", debug_assertions)))]
fn main() {
    eprintln!("worker_auth_fixture_unavailable");
    std::process::exit(1);
}
