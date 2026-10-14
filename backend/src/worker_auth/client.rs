use super::{
    AuthError, AuthResult,
    key_store::KeyStore,
    state::{Disposition, IdentityState},
};
use crate::distribution_contracts::{
    canonical::{canonical_bytes, sha256},
    primitives::timestamp_micros,
    signing::SignaturePurpose,
    wire::decode_message,
};
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use reqwest::{Client, Url};
use serde_json::{Value, json};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
const AUDIENCE: &str = "craxii.worker:development";
const MAX_RESPONSE: usize = 65536;
/// Secrets never implement Debug or Serialize and never enter agent SQLite.
pub struct WorkerClient {
    http: Client,
    origin: Url,
    key: KeyStore,
    state: IdentityState,
    selection: Option<String>,
    session: Option<Session>,
}
struct Session {
    id: String,
    expires_at: String,
    clock_seen: SystemTime,
    credential: String,
    deadline: Instant,
    scopes: Vec<String>,
}
impl std::fmt::Debug for WorkerClient {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("WorkerClient([REDACTED])")
    }
}
fn field<'a>(v: &'a Value, k: &str) -> AuthResult<&'a str> {
    v.get(k)
        .and_then(Value::as_str)
        .ok_or(AuthError::InvalidContext)
}
fn object(v: &Value) -> AuthResult<&serde_json::Map<String, Value>> {
    v.as_object().ok_or(AuthError::InvalidContext)
}
fn envelope(kind: &str, body: Value) -> AuthResult<Value> {
    let v =
        json!({"protocol":"craxii.worker","version":{"major":1,"minor":0},"type":kind,"body":body});
    decode_message(&canonical_bytes(&v).map_err(|_| AuthError::InvalidContext)?)
        .map_err(|_| AuthError::InvalidContext)?;
    Ok(v)
}
impl WorkerClient {
    pub fn new(origin: &str, key: KeyStore, state: IdentityState) -> AuthResult<Self> {
        Self::build(origin, key, state, None)
    }
    #[cfg(all(feature = "test-failpoints", debug_assertions))]
    pub fn isolated_tls_fixture(
        origin: &str,
        key: KeyStore,
        state: IdentityState,
        pem: &[u8],
    ) -> AuthResult<Self> {
        let u = Url::parse(origin).map_err(|_| AuthError::InvalidContext)?;
        if !matches!(u.host_str(), Some("localhost" | "127.0.0.1")) {
            return Err(AuthError::InvalidContext);
        }
        Self::build(
            origin,
            key,
            state,
            Some(reqwest::Certificate::from_pem(pem).map_err(|_| AuthError::InvalidContext)?),
        )
    }
    fn build(
        origin: &str,
        key: KeyStore,
        state: IdentityState,
        root: Option<reqwest::Certificate>,
    ) -> AuthResult<Self> {
        let origin = Url::parse(origin).map_err(|_| AuthError::InvalidContext)?;
        if origin.scheme() != "https"
            || origin.host_str().is_none()
            || origin.username() != ""
            || origin.password().is_some()
            || origin.query().is_some()
            || origin.fragment().is_some()
            || origin.path() != "/"
        {
            return Err(AuthError::InvalidContext);
        }
        state.validate(&key)?;
        state.persist(&key)?;
        let mut builder = Client::builder()
            .https_only(true)
            .no_proxy()
            .redirect(reqwest::redirect::Policy::none())
            .timeout(Duration::from_secs(5))
            .connect_timeout(Duration::from_secs(2))
            .pool_max_idle_per_host(2);
        if let Some(root) = root {
            builder = builder.add_root_certificate(root);
        }
        let http = builder.build().map_err(|_| AuthError::Transport)?;
        Ok(Self {
            http,
            origin,
            key,
            state,
            selection: None,
            session: None,
        })
    }
    pub fn public_key(&self) -> String {
        self.key.public_key()
    }
    pub fn key_id(&self) -> String {
        self.key.key_id()
    }
    pub fn identity(&self) -> &IdentityState {
        &self.state
    }
    fn active(&self) -> AuthResult<()> {
        match self.state.disposition {
            Disposition::Revoked => Err(AuthError::Revoked),
            Disposition::Stale => Err(AuthError::Stale),
            Disposition::Support => Err(AuthError::KeyUnavailable),
            _ => Ok(()),
        }
    }
    async fn exchange(
        &self,
        path: &str,
        body: Option<&Value>,
        credential: Option<(&str, &str)>,
    ) -> AuthResult<Value> {
        self.active()?;
        let response_limit = if path == "/v1/worker/poll" {
            1048576
        } else if path.starts_with("/v1/worker/enrollment/") {
            MAX_RESPONSE
        } else {
            16384
        };
        let url = self
            .origin
            .join(path)
            .map_err(|_| AuthError::InvalidContext)?;
        if url.origin() != self.origin.origin() {
            return Err(AuthError::InvalidContext);
        }
        let mut request = if let Some(body) = body {
            let bytes = canonical_bytes(body).map_err(|_| AuthError::InvalidContext)?;
            if bytes.len()
                > if path == "/v1/worker/results"
                    || path == "/v1/worker/requests/challenge"
                        && body["request"]["type"] == "result_export"
                {
                    557056
                } else if path.starts_with("/v1/worker/enrollment/") {
                    65536
                } else {
                    16384
                }
            {
                return Err(AuthError::InvalidContext);
            }
            self.http
                .post(url)
                .header("content-type", "application/json")
                .body(bytes)
        } else {
            self.http.get(url)
        };
        if let Some((scheme, value)) = credential {
            let mut header = reqwest::header::HeaderValue::from_str(&format!("{scheme} {value}"))
                .map_err(|_| AuthError::InvalidContext)?;
            header.set_sensitive(true);
            request = request.header(reqwest::header::AUTHORIZATION, header);
        }
        let mut response = request.send().await.map_err(|_| AuthError::Transport)?;
        let status = response.status();
        if response
            .content_length()
            .is_some_and(|n| n > response_limit as u64)
        {
            return Err(AuthError::InvalidContext);
        }
        let mut bytes = Vec::new();
        while let Some(chunk) = response.chunk().await.map_err(|_| AuthError::Transport)? {
            if bytes.len() + chunk.len() > response_limit {
                return Err(AuthError::InvalidContext);
            }
            bytes.extend_from_slice(&chunk);
        }
        // Shared raw decoder rejects duplicate keys and ambiguous canonical bytes.
        let value = crate::distribution_contracts::raw_json::parse_bounded(
            &bytes,
            crate::distribution_contracts::raw_json::Limits {
                bytes: response_limit,
                nodes: if response_limit > 65536 { 32768 } else { 4096 },
                depth: 32,
            },
        )
        .map_err(|_| AuthError::InvalidContext)?;
        if !status.is_success() {
            if bytes.len() > 4096 {
                return Err(AuthError::InvalidContext);
            }
            return Err(match value["reason"].as_str() {
                Some("session_expired" | "unauthenticated") => AuthError::SessionExpired,
                Some("revoked_authority") => AuthError::Revoked,
                Some("stale_generation") => AuthError::Stale,
                Some("reconciliation_required") => AuthError::ResultMissing,
                Some("result_conflict") => AuthError::ResultConflict,
                _ if status.is_server_error() => AuthError::Unavailable,
                _ => AuthError::InvalidContext,
            });
        }
        Ok(value)
    }
    pub async fn negotiate(
        &mut self,
        hello: &Value,
        enrollment: Option<(&str, &str)>,
    ) -> AuthResult<()> {
        decode_message(&canonical_bytes(hello).map_err(|_| AuthError::InvalidContext)?)
            .map_err(|_| AuthError::InvalidContext)?;
        if field(hello, "type")? != "hello" {
            return Err(AuthError::InvalidContext);
        }
        let mut body = json!({"installation_id":self.state.installation_id,"public_key":self.key.public_key(),"hello":hello});
        if let Some((id, _)) = enrollment {
            body["enrollment_id"] = json!(id);
        }
        let response = self
            .exchange(
                "/v1/worker/negotiation",
                Some(&body),
                enrollment.map(|(_, c)| ("CraxiiEnrollment", c)),
            )
            .await?;
        decode_message(&canonical_bytes(&response).map_err(|_| AuthError::InvalidContext)?)
            .map_err(|_| AuthError::InvalidContext)?;
        if field(&response, "type")? != "selected_protocol"
            || response["body"]["selected_version"] != json!({"major":1,"minor":0})
            || response["body"]["canonicalization_version"] != "craxii.cjson/1"
            || response["body"]["signature_version"] != "craxii.signature/1"
        {
            return Err(AuthError::InvalidContext);
        }
        self.selection = Some(field(&response["body"], "selection_id")?.to_owned());
        Ok(())
    }
    fn context(&self, body: &Value, purpose: &str) -> AuthResult<()> {
        self.active()?;
        for (k, v) in [
            ("audience", AUDIENCE),
            ("environment", "development"),
            ("customer_id", self.state.customer_id.as_str()),
            ("logical_agent_id", self.state.logical_agent_id.as_str()),
            ("installation_id", self.state.installation_id.as_str()),
            ("generation", self.state.generation.as_str()),
            ("purpose", purpose),
            (
                "selection_id",
                self.selection.as_deref().ok_or(AuthError::InvalidContext)?,
            ),
        ] {
            if field(body, k)? != v {
                return Err(AuthError::InvalidContext);
            }
        }
        let issued =
            timestamp_micros(field(body, "issued_at")?).map_err(|_| AuthError::InvalidContext)?;
        let expires =
            timestamp_micros(field(body, "expires_at")?).map_err(|_| AuthError::InvalidContext)?;
        if expires <= issued || expires - issued > 60_000_000 {
            return Err(AuthError::InvalidContext);
        }
        Ok(())
    }
    pub fn enrollment_proof(
        &self,
        challenge: &Value,
        enrollment_id: &str,
        expected_instance_ref: &str,
        document: &[u8],
        signature: &[u8],
    ) -> AuthResult<Value> {
        decode_message(&canonical_bytes(challenge).map_err(|_| AuthError::InvalidContext)?)
            .map_err(|_| AuthError::InvalidContext)?;
        if field(challenge, "type")? != "enrollment_challenge"
            || document.is_empty()
            || document.len() > 16384
            || signature.is_empty()
            || signature.len() > 8192
        {
            return Err(AuthError::InvalidContext);
        }
        let body = &challenge["body"];
        self.context(body, "enroll")?;
        if field(body, "enrollment_id")? != enrollment_id
            || field(body, "expected_instance_ref")? != expected_instance_ref
        {
            return Err(AuthError::InvalidContext);
        }
        let mut signed = object(body)?.clone();
        signed.insert("purpose".into(), json!("enrollment"));
        signed.insert("signature_version".into(), json!(1));
        signed.insert("key_id".into(), json!(self.key.key_id()));
        signed.insert("public_key".into(), json!(self.key.public_key()));
        signed.insert(
            "instance_identity_document_sha256".into(),
            json!(sha256(document)),
        );
        signed.insert(
            "instance_identity_signature_sha256".into(),
            json!(sha256(signature)),
        );
        let signed = Value::Object(signed);
        envelope(
            "enrollment_proof",
            json!({"proof_signature":self.key.sign(SignaturePurpose::Enrollment,&signed)?,"signed":signed,"instance_identity_document_b64":URL_SAFE_NO_PAD.encode(document),"instance_identity_signature_b64":URL_SAFE_NO_PAD.encode(signature)}),
        )
    }
    pub async fn enroll(
        &mut self,
        id: &str,
        credential: &str,
        expected_instance_ref: &str,
        document: &[u8],
        signature: &[u8],
    ) -> AuthResult<Value> {
        if let Some(signed) = &self.state.pending_signed {
            if field(signed, "enrollment_id")? != id
                || field(signed, "expected_instance_ref")? != expected_instance_ref
            {
                return Err(AuthError::InvalidContext);
            }
            return self
                .resume_enrollment(document, signature, Some(credential))
                .await;
        }
        let challenge=self.exchange("/v1/worker/enrollment/challenge",Some(&json!({"enrollment_id":id,"selection_id":self.selection.as_deref().ok_or(AuthError::InvalidContext)?,"public_key":self.key.public_key()})),Some(("CraxiiEnrollment",credential))).await?;
        let proof =
            self.enrollment_proof(&challenge, id, expected_instance_ref, document, signature)?;
        self.state.disposition = Disposition::Enrolling;
        self.state.pending_signed = Some(proof["body"]["signed"].clone());
        self.state.persist(&self.key)?;
        // A caller may reconcile this exact proof; no new key/operation follows ambiguous transport.
        self.submit_enrollment(&proof, Some(credential)).await
    }
    pub async fn resume_enrollment(
        &mut self,
        document: &[u8],
        signature: &[u8],
        credential: Option<&str>,
    ) -> AuthResult<Value> {
        self.state.validate(&self.key)?;
        let signed = self
            .state
            .pending_signed
            .as_ref()
            .ok_or(AuthError::InvalidContext)?
            .clone();
        if document.len() > 16384
            || signature.len() > 8192
            || signed["instance_identity_document_sha256"] != sha256(document)
            || signed["instance_identity_signature_sha256"] != sha256(signature)
        {
            return Err(AuthError::InvalidContext);
        }
        let proof = envelope(
            "enrollment_proof",
            json!({"signed":signed,"proof_signature":self.key.sign(SignaturePurpose::Enrollment,&signed)?,"instance_identity_document_b64":URL_SAFE_NO_PAD.encode(document),"instance_identity_signature_b64":URL_SAFE_NO_PAD.encode(signature)}),
        )?;
        self.submit_enrollment(&proof, credential).await
    }
    pub async fn submit_enrollment(
        &mut self,
        proof: &Value,
        credential: Option<&str>,
    ) -> AuthResult<Value> {
        let response = self
            .exchange(
                "/v1/worker/enrollment/proof",
                Some(proof),
                credential.map(|c| ("CraxiiEnrollment", c)),
            )
            .await?;
        decode_message(&canonical_bytes(&response).map_err(|_| AuthError::InvalidContext)?)
            .map_err(|_| AuthError::InvalidContext)?;
        let body = &response["body"];
        if field(&response, "type")? != "enrollment_ack"
            || field(body, "installation_id")? != self.state.installation_id
            || field(body, "generation")? != self.state.generation
            || field(body, "key_id")? != self.key.key_id()
            || field(body, "enrollment_id")? != field(&proof["body"]["signed"], "enrollment_id")?
        {
            return Err(AuthError::InvalidContext);
        }
        self.state.binding_id = Some(field(body, "binding_id")?.to_owned());
        self.state.pending_signed = None;
        self.state.disposition = Disposition::Bound;
        self.state.persist(&self.key)?;
        Ok(response)
    }
    pub fn session_proof(&self, challenge: &Value) -> AuthResult<Value> {
        decode_message(&canonical_bytes(challenge).map_err(|_| AuthError::InvalidContext)?)
            .map_err(|_| AuthError::InvalidContext)?;
        if field(challenge, "type")? != "session_challenge" {
            return Err(AuthError::InvalidContext);
        }
        let body = &challenge["body"];
        self.context(body, "session")?;
        if field(body, "key_id")? != self.key.key_id()
            || !scope_policy(&body["scopes"])
            || field(body, "revocation_epoch")?
                .parse::<i64>()
                .map_err(|_| AuthError::InvalidContext)?
                < self
                    .state
                    .highest_epoch
                    .parse::<i64>()
                    .map_err(|_| AuthError::InvalidContext)?
        {
            return Err(AuthError::InvalidContext);
        }
        let mut signed = object(body)?.clone();
        signed.insert("signature_version".into(), json!(1));
        let signed = Value::Object(signed);
        envelope(
            "session_proof",
            json!({"signed":signed,"proof_signature":self.key.sign(SignaturePurpose::Session,&signed)?}),
        )
    }
    async fn renew_once(&mut self) -> AuthResult<()> {
        self.session = None;
        let started = Instant::now();
        let challenge=self.exchange("/v1/worker/session/challenge",Some(&json!({"installation_id":self.state.installation_id,"key_id":self.key.key_id(),"selection_id":self.selection.as_deref().ok_or(AuthError::InvalidContext)?})),None).await?;
        let proof = self.session_proof(&challenge)?;
        let grant = self
            .exchange("/v1/worker/session/proof", Some(&proof), None)
            .await?;
        decode_message(&canonical_bytes(&grant).map_err(|_| AuthError::InvalidContext)?)
            .map_err(|_| AuthError::InvalidContext)?;
        let body = &grant["body"];
        for k in [
            "customer_id",
            "logical_agent_id",
            "installation_id",
            "generation",
            "selection_id",
            "key_id",
            "scopes",
            "revocation_epoch",
        ] {
            if body[k] != challenge["body"][k] {
                return Err(AuthError::InvalidContext);
            }
        }
        if field(&grant, "type")? != "session_grant" {
            return Err(AuthError::InvalidContext);
        }
        let issued =
            timestamp_micros(field(body, "issued_at")?).map_err(|_| AuthError::InvalidContext)?;
        let expires =
            timestamp_micros(field(body, "expires_at")?).map_err(|_| AuthError::InvalidContext)?;
        if expires <= issued || expires - issued > 300_000_000 {
            return Err(AuthError::InvalidContext);
        }
        self.state.highest_epoch = field(body, "revocation_epoch")?.to_owned();
        self.state.disposition = Disposition::Bound;
        self.state.persist(&self.key)?;
        self.session = Some(Session {
            id: field(body, "session_id")?.to_owned(),
            expires_at: field(body, "expires_at")?.to_owned(),
            clock_seen: SystemTime::now(),
            credential: field(body, "session_credential")?.to_owned(),
            deadline: started + Duration::from_micros((expires - issued) as u64),
            scopes: body["scopes"]
                .as_array()
                .ok_or(AuthError::InvalidContext)?
                .iter()
                .map(|v| {
                    v.as_str()
                        .map(str::to_owned)
                        .ok_or(AuthError::InvalidContext)
                })
                .collect::<AuthResult<Vec<_>>>()?,
        });
        Ok(())
    }
    pub async fn renew(&mut self) -> AuthResult<()> {
        let result = self.renew_once().await;
        if let Err(error) = result {
            self.session = None;
            if matches!(error, AuthError::Revoked | AuthError::Stale) {
                self.state.disposition = if error == AuthError::Revoked {
                    Disposition::Revoked
                } else {
                    Disposition::Stale
                };
                self.state.persist(&self.key)?;
            }
        }
        result
    }
    pub async fn status(&mut self) -> AuthResult<Value> {
        let s = self.session.as_ref().ok_or(AuthError::Unavailable)?;
        let clock = SystemTime::now();
        let epoch = timestamp_micros("1970-01-01T00:00:00.000000Z")
            .map_err(|_| AuthError::InvalidContext)?;
        let now = clock
            .duration_since(UNIX_EPOCH)
            .map_err(|_| AuthError::SessionExpired)?
            .as_micros() as i128
            + epoch;
        let expires = timestamp_micros(&s.expires_at).map_err(|_| AuthError::InvalidContext)?;
        if Instant::now() >= s.deadline || now >= expires || clock < s.clock_seen {
            self.session = None;
            return Err(AuthError::Unavailable);
        }
        let result = self
            .exchange(
                "/v1/worker/auth/status",
                None,
                Some(("CraxiiWorker", &s.credential)),
            )
            .await;
        if matches!(result, Err(AuthError::SessionExpired)) {
            self.session = None;
        }
        if matches!(result, Err(AuthError::Stale)) {
            self.session = None;
            self.state.disposition = Disposition::Stale;
            self.state.persist(&self.key)?;
        }
        if result == Err(AuthError::Revoked) {
            self.session = None;
            self.state.disposition = Disposition::Revoked;
            self.state.persist(&self.key)?;
        }
        if let Ok(v) = &result {
            let fields = [
                "contract_version",
                "customer_id",
                "logical_agent_id",
                "installation_id",
                "generation",
                "key_id",
                "binding_id",
                "session_id",
                "revocation_epoch",
                "expires_at",
                "chargeable",
                "authentication_only",
            ];
            let session = self.session.as_ref().ok_or(AuthError::SessionExpired)?;
            if object(v)?.len() != fields.len()
                || object(v)?.keys().any(|k| !fields.contains(&k.as_str()))
                || v["contract_version"] != 1
                || v["installation_id"] != self.state.installation_id
                || v["generation"] != self.state.generation
                || v["customer_id"] != self.state.customer_id
                || v["logical_agent_id"] != self.state.logical_agent_id
                || v["key_id"] != self.key.key_id()
                || v["revocation_epoch"] != self.state.highest_epoch
                || v["session_id"] != session.id
                || v["expires_at"] != session.expires_at
                || v["chargeable"] != false
                || v["authentication_only"] != true
            {
                return Err(AuthError::InvalidContext);
            }
            crate::distribution_contracts::primitives::uuid(field(v, "binding_id")?, Some(4))
                .map_err(|_| AuthError::InvalidContext)?;
            if self
                .state
                .binding_id
                .as_ref()
                .is_some_and(|id| v["binding_id"] != *id)
            {
                return Err(AuthError::InvalidContext);
            }
            if self.state.binding_id.is_none() {
                self.state.binding_id = Some(field(v, "binding_id")?.to_owned());
                self.state.persist(&self.key)?;
            }
        }
        result
    }
    pub fn current_authority(&self) -> AuthResult<Value> {
        self.check_session()?;
        let session = self.session.as_ref().ok_or(AuthError::SessionExpired)?;
        Ok(
            json!({"customer_id":self.state.customer_id,"logical_agent_id":self.state.logical_agent_id,"installation_id":self.state.installation_id,"generation":self.state.generation,"session_id":session.id}),
        )
    }
    fn check_session(&self) -> AuthResult<()> {
        self.active()?;
        let s = self.session.as_ref().ok_or(AuthError::SessionExpired)?;
        let now = crate::distribution::now().map_err(|_| AuthError::SessionExpired)?;
        if Instant::now() >= s.deadline
            || SystemTime::now() < s.clock_seen
            || now >= timestamp_micros(&s.expires_at).map_err(|_| AuthError::InvalidContext)?
        {
            return Err(AuthError::SessionExpired);
        }
        Ok(())
    }
    async fn session_exchange(&mut self, path: &str, request: &Value) -> AuthResult<Value> {
        self.check_session()?;
        let credential = self
            .session
            .as_ref()
            .ok_or(AuthError::SessionExpired)?
            .credential
            .clone();
        let result = self
            .exchange(path, Some(request), Some(("CraxiiWorker", &credential)))
            .await;
        if let Err(error) = result {
            if matches!(error, AuthError::Revoked | AuthError::Stale) {
                self.state.disposition = if error == AuthError::Revoked {
                    Disposition::Revoked
                } else {
                    Disposition::Stale
                };
                self.state.persist(&self.key)?;
            }
            if matches!(
                error,
                AuthError::SessionExpired | AuthError::Revoked | AuthError::Stale
            ) {
                self.session = None;
            }
        }
        result
    }
    pub async fn poll(
        &mut self,
        boot: &str,
        power: &str,
        profile_key: &str,
    ) -> AuthResult<Vec<crate::distribution::ValidatedDelivery>> {
        self.check_session()?;
        let s = self.session.as_ref().ok_or(AuthError::SessionExpired)?;
        if !["status", "receipt_commit", "cancel"]
            .iter()
            .all(|scope| s.scopes.iter().any(|s| s == scope))
        {
            return Err(AuthError::InvalidContext);
        }
        let id = crate::distribution::request_id().map_err(|_| AuthError::InvalidContext)?;
        let authority = self.current_authority()?;
        let request = json!({"contract_version":1,"request_id":id,"authority":authority,"boot_id":boot,"power_operation_id":power,"max_items":4,"wait_ms":0});
        let response = self.session_exchange("/v1/worker/poll", &request).await?;
        let items = crate::distribution::transport::poll_response(&response, &id)
            .map_err(|_| AuthError::InvalidContext)?;
        let mut result = Vec::new();
        for message in items.iter().filter(|m| m["type"] != "execution_profile") {
            let profile = items
                .iter()
                .find(|p| {
                    p["type"] == "execution_profile"
                        && p["body"]["profile"]["work_id"] == message["body"]["material"]["work_id"]
                })
                .cloned();
            result.push(
                crate::distribution::ValidatedDelivery::from_authenticated(
                    message.clone(),
                    profile,
                    profile_key.to_owned(),
                    &authority,
                    boot,
                    power,
                )
                .map_err(|_| AuthError::InvalidContext)?,
            );
        }
        Ok(result)
    }
    pub async fn send_receipt(&mut self, body: &Value, control: bool) -> AuthResult<Value> {
        let authority = self.current_authority()?;
        let id = crate::distribution::request_id().map_err(|_| AuthError::InvalidContext)?;
        let kind = if control {
            "cancel_receipt"
        } else {
            "work_receipt"
        };
        let request = json!({"protocol":"craxii.worker","version":{"major":1,"minor":0},"type":kind,"request_id":id,"environment":"development","authority":authority,"body":body});
        decode_message(&canonical_bytes(&request).map_err(|_| AuthError::InvalidContext)?)
            .map_err(|_| AuthError::InvalidContext)?;
        let proof = self
            .sensitive_proof(&request, &authority, &id, kind)
            .await?;
        let response = self
            .session_exchange(
                if control {
                    "/v1/worker/control-receipts"
                } else {
                    "/v1/worker/receipts"
                },
                &json!({"contract_version":1,"request":request,"proof":proof}),
            )
            .await?;
        if control {
            let o = object(&response)?;
            if o.len() != 5
                || o.keys().any(|k| {
                    !matches!(
                        k.as_str(),
                        "contract_version"
                            | "request_id"
                            | "control_id"
                            | "control_digest"
                            | "accepted"
                    )
                })
                || response["contract_version"] != 1
                || response["request_id"] != id
                || response["control_id"] != body["control_id"]
                || response["control_digest"] != body["control_digest"]
                || response["accepted"] != true
            {
                return Err(AuthError::InvalidContext);
            }
        } else {
            decode_message(&canonical_bytes(&response).map_err(|_| AuthError::InvalidContext)?)
                .map_err(|_| AuthError::InvalidContext)?;
            if response["type"] != "receipt_status"
                || response["request_id"] != id
                || response["authority"] != authority
                || response["body"]["status"] != "committed"
                || response["body"]["receipt"] != body["receipt"]
            {
                return Err(AuthError::InvalidContext);
            }
        }
        Ok(response)
    }
    async fn sensitive_proof(
        &mut self,
        request: &Value,
        authority: &Value,
        id: &str,
        kind: &str,
    ) -> AuthResult<Value> {
        let signed = self
            .session_exchange(
                "/v1/worker/requests/challenge",
                &json!({"contract_version":1,"request":request}),
            )
            .await?;
        let expected_digest = crate::distribution_contracts::canonical::digest_material(
            crate::distribution_contracts::canonical::DigestDomain::AuthenticatedRequest,
            request,
        )
        .map_err(|_| AuthError::InvalidContext)?;
        if signed["purpose"] != "sensitive_request"
            || signed["signature_version"] != 1
            || signed["audience"] != AUDIENCE
            || signed["environment"] != "development"
            || signed["session_id"] != authority["session_id"]
            || signed["installation_id"] != authority["installation_id"]
            || signed["generation"] != authority["generation"]
            || signed["key_id"] != self.key.key_id()
            || signed["revocation_epoch"] != self.state.highest_epoch
            || signed["request_id"] != id
            || signed["operation_type"] != kind
            || signed["request_digest"] != expected_digest
        {
            return Err(AuthError::InvalidContext);
        }
        let proof = json!({"protocol":"craxii.worker","version":{"major":1,"minor":0},"type":"sensitive_request_proof","request_id":id,"environment":"development","authority":authority,"body":{"signed":signed,"proof_signature":self.key.sign(SignaturePurpose::SensitiveRequest,&signed)?}});
        decode_message(&canonical_bytes(&proof).map_err(|_| AuthError::InvalidContext)?)
            .map_err(|_| AuthError::InvalidContext)?;
        let issued = timestamp_micros(field(&signed, "issued_at")?)
            .map_err(|_| AuthError::InvalidContext)?;
        let expiry = timestamp_micros(field(&signed, "expires_at")?)
            .map_err(|_| AuthError::InvalidContext)?;
        let now = crate::distribution::now().map_err(|_| AuthError::InvalidContext)?;
        if issued > now || expiry <= now || expiry - issued > 60_000_000 {
            return Err(AuthError::InvalidContext);
        }
        Ok(proof)
    }
    pub async fn query_result(&mut self, body: &Value) -> AuthResult<Value> {
        let s = &body["snapshot"];
        let query = json!({"work_id":s["work_id"],"export_id":s["export_id"],"export_revision":"1","export_digest":body["export_digest"]});
        let authority = self.current_authority()?;
        let id = crate::distribution::request_id().map_err(|_| AuthError::InvalidContext)?;
        let request = json!({"protocol":"craxii.worker","version":{"major":1,"minor":0},"type":"result_query","request_id":id,"environment":"development","authority":authority,"body":query});
        let response = self
            .session_exchange("/v1/worker/result-status", &request)
            .await?;
        self.validate_result_acceptance(&response, &authority, &id, body)?;
        Ok(response)
    }
    pub async fn send_result(&mut self, body: &Value) -> AuthResult<Value> {
        let authority = self.current_authority()?;
        let id = crate::distribution::request_id().map_err(|_| AuthError::InvalidContext)?;
        let request = json!({"protocol":"craxii.worker","version":{"major":1,"minor":0},"type":"result_export","request_id":id,"environment":"development","authority":authority,"body":body});
        decode_message(&canonical_bytes(&request).map_err(|_| AuthError::InvalidContext)?)
            .map_err(|_| AuthError::InvalidContext)?;
        let proof = self
            .sensitive_proof(&request, &authority, &id, "result_export")
            .await?;
        let response = self
            .session_exchange(
                "/v1/worker/results",
                &json!({"contract_version":1,"request":request,"proof":proof}),
            )
            .await?;
        self.validate_result_acceptance(&response, &authority, &id, body)?;
        Ok(response)
    }
    fn validate_result_acceptance(
        &self,
        r: &Value,
        a: &Value,
        id: &str,
        b: &Value,
    ) -> AuthResult<()> {
        decode_message(&canonical_bytes(r).map_err(|_| AuthError::InvalidContext)?)
            .map_err(|_| AuthError::InvalidContext)?;
        if r["type"] != "result_acceptance"
            || r["authority"] != *a
            || r["request_id"] != id
            || r["body"]["work_id"] != b["snapshot"]["work_id"]
            || r["body"]["export_id"] != b["snapshot"]["export_id"]
            || r["body"]["export_revision"] != "1"
            || r["body"]["export_digest"] != b["export_digest"]
        {
            return Err(AuthError::InvalidContext);
        }
        Ok(())
    }
}

fn scope_policy(value: &Value) -> bool {
    let Some(scopes) = value.as_array() else {
        return false;
    };
    scopes == &vec![json!("status")]
        || scopes == &vec![json!("status"), json!("receipt_commit"), json!("cancel")]
        || scopes
            == &vec![
                json!("status"),
                json!("receipt_commit"),
                json!("cancel"),
                json!("result_export"),
            ]
}
