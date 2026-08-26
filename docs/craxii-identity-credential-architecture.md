# Craxii credential and identity architecture

## 1. Executive verdict

Your direction is **directionally correct but architecturally incomplete**.

The central thesis is right:

- Standing authorization and short-lived execution credentials are different things.
- Craxii should be a distinct actor, not an impersonation of its owner.
- Provider-native workload identities should replace static secrets where possible.
- A credential/authority service should manage renewal, rotation, audit, and fallback secrets.
- Raw secrets should stay out of model context whenever possible.

The dangerous incompleteness is the implied security boundary around one powerful persistent VM.

> If Craxii has root-equivalent control of a host, every authority callable from that host is practically authority possessed by that host.

A local broker socket, credential helper, metadata endpoint, environment variable, SSH agent, or supposedly hidden sidecar does not change that. Root can inspect it, impersonate clients, invoke it as an oracle, or alter the code that uses it. A local broker can prevent persistent secret copies and improve rotation, but it cannot contain a compromised host.

The architecture I would ship therefore makes two corrections:

1. **The durable Craxii identity belongs to the control plane, not the VM or a permanent key on it.**
2. **“One coherent computer” is a product abstraction over project-scoped security compartments—not one Linux protection domain containing PTG, Clxrity, browser sessions, cloud roles, and every other credential.**

This preserves the coworker experience. Users still authorize Craxii once and then leave it alone. The compartment and token lifecycle are invisible infrastructure.

The resulting security claim is deliberately limited:

- Craxii can fully exercise authority intentionally granted to its current task and project.
- Root compromise of a PTG workspace compromises PTG data and currently issued PTG capabilities.
- It does not automatically yield Clxrity credentials, OAuth refresh grants, the GitHub App signing key, other project volumes, or renewable cloud roots.
- Compromise of the central authority plane remains potentially catastrophic and must be treated accordingly.

The established parts of this design are workload identity, STS/federation, GitHub Apps, dynamic database credentials, SSH CAs, OAuth refresh handling, and VM isolation. The more speculative, agent-specific parts are task-intent capability compilation, causal model-turn auditing, prompt-injection-aware resource envelopes, and presenting multiple isolated workspaces as one persistent computer.

---

## 2. First-principles identity model

A Craxii should not be a permanent private key living on a workstation. It should be a durable principal in Craxii’s authorization system whose current workloads receive temporary proof that they are acting for that principal.

| Identity | What it is | Lifetime | Where private/root material lives |
|---|---|---:|---|
| User identity | Abhisht’s authenticated Craxii account and recovery identity | Human account lifetime | User IdP/passkey system |
| Craxii identity | Immutable internal principal, e.g. `agent_01...`, owned by Abhisht | Until agent deletion | No permanent agent key on VM |
| Workstation identity | A time-bounded lease for one provisioned machine instance | Minutes–hours, renewable | Instance-bound ephemeral key, preferably vTPM-backed |
| Workspace identity | A principal for one project/security scope, e.g. PTG staging | Task/workspace lifetime | Project VM or workload identity system |
| Task identity | The currently authorized resource and action envelope | Hours–days, renewable | Signed by Craxii authority plane |
| Process/tool identity | A child execution lease for a helper, browser, SSH session, or worker | Seconds–hours | Executor-local ephemeral key |
| Provider identity | GitHub App, AWS role session, Entra service principal, GCP service account, DB user, etc. | Provider-specific | Provider or central credential adapter |

A useful mature identity URI would look like:

```text
spiffe://craxii.internal/agents/agent_123/workspaces/ptg/tasks/task_456
```

SPIFFE is useful because it standardizes workload identity, short-lived X.509/JWT SVIDs, trust domains, and automated rotation. SPIRE adds node and workload attestation. It does not itself decide what Craxii may do; authorization remains Craxii’s job. [SPIFFE’s model](https://spiffe.io/docs/latest/spiffe/concepts/) and [SPIRE’s attestation architecture](https://spiffe.io/docs/latest/spire-about/spire-concepts/) fit the mature design well.

### Identity belongs to the agent; keys belong to instances

`CRXI-Abhisht-00` is a durable control-plane object containing:

- owner and provenance;
- provider identity mappings;
- durable delegation records;
- policy and revocation state;
- audit history;
- project and memory relationships.

A workstation proves only:

> “I am a currently valid workstation instance assigned by Craxii’s control plane to agent X and workspace Y.”

It does not prove:

> “Possession of my disk or key permanently constitutes agent X.”

That distinction is what makes safe machine replacement possible.

### Workstation enrollment

A new workstation should:

1. Boot with a minimal hosting-cloud role that has no customer authority.
2. Present a one-time enrollment nonce plus cloud-native instance evidence.
3. Generate an ephemeral key, preferably non-exportable in a vTPM.
4. Present instance/image/boot attestation where supported.
5. Receive a short-lived workload certificate containing the assigned agent, workspace, and machine-session IDs.
6. Renew only while the control plane still recognizes that machine lease.

AWS signed instance identity documents and NitroTPM attestation can prove instance and boot provenance. NitroTPM can also bind keys to measured state. But attestation proves what booted; it does not prove that arbitrary runtime code remains benign after Craxii installs packages and executes repositories. [AWS explicitly frames NitroTPM attestation around trusted software and boot measurements](https://docs.aws.amazon.com/AWSEC2/latest/UserGuide/nitrotpm-attestation.html).

A root attacker can use a non-exportable TPM key as an online signing oracle. The value is that the attacker cannot copy it into a permanent off-machine identity and that destroying the instance destroys its use.

### Root of trust

There is no honest single “ultimate key.” There are several roots:

- **Ownership root:** user authentication and account recovery.
- **Authorization root:** the provider’s own installation, IAM role, RBAC assignment, DB grant, or SaaS consent.
- **Craxii root:** durable delegation records plus the authority service’s signing and encryption keys.
- **Machine root:** hosting-cloud instance identity and optional vTPM attestation.
- **Operational root:** Craxii’s cloud organization, KMS/HSM administration, deployment pipeline, and incident-response access.

Provider authorization remains the final outer boundary. Craxii cannot override GitHub branch protections, an AWS SCP, a database read-only role, or a revoked OAuth grant.

If the control plane and authority service are both fully compromised, the attacker can exercise most connected authority even if HSM keys are non-exportable. An HSM prevents key theft; it does not stop malicious use of an online signing service. This is why tenant cells, narrowly permissioned provider adapters, tamper-evident audit, and administrative separation matter.

---

## 3. Recommended architecture

```text
                         USER / CUSTOMER ADMINS
                   one-time installs, roles, grants, consent
                                   |
                                   v
                    +------------------------------+
                    |       GRANT CONTROL PLANE    |
                    |                              |
                    | Agent registry               |
                    | Delegation + revocation DB   |
                    | Connection/resource metadata |
                    | Task/resource policy compiler|
                    +---------------+--------------+
                                    |
                       signed, versioned grant state
                                    |
             +----------------------+----------------------+
             |                                             |
             v                                             v
 +---------------------------+                 +-------------------------+
 | WORKLOAD IDENTITY ISSUER  |                 | SECRET / KEY BACKENDS   |
 | KMS/HSM-backed CA/OIDC    |                 | Secrets Manager + KMS   |
 | Machine/task certificates|                 | Later: Vault DB/SSH     |
 +-------------+-------------+                 +------------+------------+
               |                                            |
               +-------------------+------------------------+
                                   |
                                   v
                    +------------------------------+
                    | CRAXII AUTHORITY DATA PLANE  |
                    | HA, outside workstation VMs  |
                    |                              |
                    | AuthN + policy evaluation    |
                    | Token exchange/minting       |
                    | Credential injection/proxy   |
                    | Rotation and revocation      |
                    | Audit correlation            |
                    +--+-------+--------+---------+
                       |       |        |
        GitHub signer--+   Cloud STS    +--OAuth/API/DB/SSH adapters
                       |       |        |
                       v       v        v
                  EXTERNAL PROVIDERS AND CUSTOMER SYSTEMS


       ONE CONTINUOUS CRAXII RELATIONSHIP / PLANNER / MEMORY
                              |
                    signed task resource envelope
                              |
            +-----------------+------------------+
            |                                    |
            v                                    v
 +------------------------+           +-------------------------+
 | PTG PROJECT VM         |           | CLXRITY PROJECT VM      |
 | Root inside guest      |           | Root inside guest       |
 | PTG volume and caches  |           | Clxrity volume/caches   |
 | PTG workspace identity |           | Clxrity workspace ID    |
 | No durable roots       |           | No durable roots        |
 +-----------+------------+           +------------+------------+
             |                                      |
       scoped helpers                         scoped helpers
       and task workers                       and task workers

 General browsing and model calls do not receive provider secrets.
 A cross-project task coordinates separate compartments; it does not mount both
 projects and all credentials into one guest.
```

### Durable delegation

A delegation record should resemble:

```text
delegation_id
agent_id
owner_id
provider_connection_id
resource selectors
allowed operations
environment classification
provider principal
grant provenance and consent event
policy constraints
valid_from / valid_until
revocation_epoch
version
```

For each task, the authority service computes:

```text
effective authority =
    standing delegation
  ∩ task resource envelope
  ∩ explicit user constraints
  ∩ workspace policy
  ∩ provider-side permissions
```

Thus “fix PTG; don’t deploy” can yield repository write, PR, staging logs, and test DB capabilities while excluding deployment and production mutation—even if a broader standing cloud role exists.

This is not a user approval workflow. It is automatic capability selection within previously granted authority.

### Trust domains

| Domain | What it trusts | What compromise means | What crosses the boundary |
|---|---|---|---|
| User | Craxii UI, IdP, provider consent pages | Attacker can add/revoke standing grants | Consent and configuration, never provider roots from the workstation |
| Grant control plane | User auth, provider callbacks, deployment integrity | Can falsify tasks or delegations; potentially broad impact | Signed grant state and task envelopes |
| Workload issuer | Cloud attestation, provisioning records | Can mint fake Craxii/workspace identities | Short-lived certs/JWTs only |
| Authority broker | Workload identity, grant versions, provider trust | Can use all authority reachable in its cell | Provider tokens or proxied operations |
| Secret store/KMS | Adapter IAM identities and key policy | Static secrets/refresh grants exposed if decryption is also compromised | Long-lived fallback material only to adapters |
| Persistent relationship/planner | Control plane and model provider | Can steer Craxii and expose context, but has no raw roots | Plans, code, task IDs, tool results |
| Project workspace | Signed task lease, broker, project volume | Full project compromise and current scoped authority | Project-scoped credentials, preferably only during use |
| Tool/process | Executor and one operation/session | Current credential or operation can be stolen/abused | Minimum executable credential |
| Temporary worker | Parent task and broker | Child task compromised, not parent’s complete authority | Attenuated child lease |
| External provider | Its own IAM and Craxii provider identity | Provider account/resource impact | Provider-specific short-lived credential |
| Frontier-model provider | Prompt, selected files, tool results | Malicious planning within active task envelope | No raw secrets; nevertheless sees sensitive code/context |

A local UID, container name, or Unix socket is not a strong trust domain when Craxii has guest root.

---

## 4. Credential lifecycle

### Onboarding a Craxii

1. Abhisht creates Craxii.
2. Craxii assigns an immutable internal `agent_id`; `CRXI-Abhisht-00` is a display name.
3. The user account becomes its owner and revocation authority.
4. No VM or permanent agent key is required yet.
5. Project workspaces enroll with instance identity and receive temporary workload identities.

### Connecting GitHub

1. Craxii launches GitHub’s App Manifest flow to create a **customer-owned, dedicated GitHub App** for this Craxii and GitHub organization.
2. The user names it, for example, `CRXI-Abhisht-00`, and installs it on selected repositories.
3. GitHub returns the app configuration and private key to Craxii’s server-side flow. GitHub’s manifest flow exists specifically to create a preconfigured app and return its ID, webhook secret, and private key. [GitHub App Manifest documentation](https://docs.github.com/en/apps/sharing-github-apps/registering-a-github-app-from-a-manifest)
4. The key is encrypted centrally and accessible only to a GitHub signer adapter.
5. The standing grant stores the app and installation IDs, repositories, and approved permissions.
6. For a PTG task, the signer mints an installation token narrowed to PTG and the permissions needed. GitHub installation tokens expire after one hour and can be further restricted by repository and permission at mint time. [GitHub installation token documentation](https://docs.github.com/en/apps/creating-github-apps/authenticating-with-a-github-app/generating-an-installation-access-token-for-a-github-app)

### Connecting AWS

1. The customer deploys a Craxii IAM role in their account, ideally one per project/environment.
2. Its trust policy trusts only Craxii’s hardened broker role—or Craxii’s OIDC issuer and exact subject.
3. For direct cross-account trust, require a unique External ID to prevent confused-deputy problems.
4. The permission policy expresses the durable outer boundary.
5. Craxii stores the role ARN and External ID, not AWS access keys.
6. When needed, the broker calls STS and sets `SourceIdentity`, session name, and task/session tags. AWS recommends roles and temporary credentials for workloads and documents External IDs specifically for third-party cross-account access. [AWS third-party role guidance](https://docs.aws.amazon.com/IAM/latest/UserGuide/id_roles_common-scenarios_third-party.html)

### Connecting a database

Use this preference order:

1. Cloud/IAM-native DB authentication.
2. A customer DB proxy or identity-aware connector.
3. Dynamic users minted by Vault or an equivalent DB credential engine.
4. A unique, least-privileged static user for this Craxii and project, centrally stored.
5. Never a shared production superuser.

The durable authorization is the IAM/Entra/service account or database role. Actual login tokens/users remain ephemeral.

### Connecting a generic API-key service

1. The user supplies the key once through a control-plane secret-entry flow.
2. The key is stored in Secrets Manager under a project/provider connection.
3. Metadata separately specifies the permitted hostname, project, account, and intended use.
4. The preferred execution path is a credential-injecting HTTP proxy or dedicated tool adapter.
5. If an arbitrary CLI truly requires `API_KEY`, a scoped executor receives it only for that command. That command can steal or print it; this is unavoidable.
6. No other project’s secrets enter that executor.

### Starting a task

1. The task compiler identifies the primary project, environment, and explicit constraints.
2. It creates a task envelope from standing delegations.
3. A matching project workspace is selected.
4. The workspace authenticates with its temporary workload identity.
5. The broker evaluates agent, workspace, task, resource, operation, grant version, and revocation epoch.
6. It then proxies the operation or issues the narrowest provider credential available.

### A ten-hour task

No credential is “extended.” New credentials are obtained.

- GitHub credential helper fetches a new one-hour installation token for later Git operations.
- AWS SDK/CLI uses a credential-provider process and obtains a new STS session before expiry.
- Azure and GCP credential libraries perform new token exchanges.
- DB connectors acquire fresh login tokens for new connections.
- SSH receives a new certificate before reconnecting.
- OAuth adapters use the centrally stored refresh grant.
- Long operations that encounter an expiry retry with a new credential.

AWS role sessions can be configured up to 12 hours, although role chaining is limited to one hour. Craxii should renew normally rather than maximize lifetime. [AWS `AssumeRole` durations](https://docs.aws.amazon.com/STS/latest/APIReference/API_AssumeRole.html)

The durable task is a state machine; it must not depend on one process environment containing a token minted at task start.

### Revocation

The broker:

1. Increments the delegation’s revocation epoch.
2. Denies new credentials immediately.
3. Stops or quarantines affected workspaces.
4. Revokes provider credentials where supported.
5. Changes provider-side authorization when fast enforcement is important.
6. Rotates any static credential that may have entered the workspace.

Existing credential reality differs by provider:

- GitHub installation tokens can be explicitly revoked; otherwise they expire within one hour. [GitHub token revocation endpoint](https://docs.github.com/en/rest/apps/installations#revoke-an-installation-access-token)
- AWS can deny role sessions issued before a timestamp; AWS describes enforcement approximately 30 seconds into the future, but the operation affects every current session of that role. This argues for a dedicated role per Craxii/project. [AWS role session revocation](https://docs.aws.amazon.com/IAM/latest/UserGuide/id_roles_use_revoke-sessions.html)
- Azure access tokens normally last roughly 60–90 minutes. Continuous Access Evaluation for workload identities currently has important identity and resource limitations, so do not promise immediate universal invalidation. [Microsoft access-token lifetimes](https://learn.microsoft.com/en-us/entra/identity-platform/access-tokens)
- GCP service-account access tokens are normally one hour and cannot themselves be revoked. Provider IAM changes may remove effective authorization, but the credential remains valid until expiry. [Google service-account credential documentation](https://cloud.google.com/docs/authentication/token-types)
- SSH certificates remain usable until expiry unless the target supports an online revocation mechanism or the CA is removed.
- Database logins can be disabled and active sessions terminated server-side.
- A cloned repository and data already exfiltrated cannot be cryptographically “unread.”

### VM compromise and replacement

On suspected compromise:

1. Quarantine network access.
2. Revoke the machine lease.
3. Revoke or wait out issued bearer tokens.
4. Rotate every static secret exposed to the workspace.
5. Preserve forensic disk state separately.
6. Destroy the instance.
7. Create a clean instance and new workload key.
8. Restore only project data—not the old operating system, credential caches, shell startup hooks, browser profiles, or active process state.
9. Treat restored executable content as untrusted.

The new VM remains the same Craxii because the durable `agent_id`, delegations, task state, memory, and project metadata survived in the control plane.

Never back up:

- STS credentials;
- GitHub installation tokens;
- temporary OAuth access tokens;
- SSH private keys or certificates;
- workstation identity keys;
- task capability leases.

Back up, encrypted and separately:

- delegation and audit records;
- unavoidable API keys and refresh grants;
- project volumes and artifacts;
- task/memory state;
- optional browser profile state, in its own security domain.

---

## 5. GitHub deep dive

### Recommendation

Use a **dedicated, customer-owned GitHub App per Craxii × GitHub organization/trust boundary**, created through the GitHub App Manifest flow.

Use:

- Installation access tokens.
- HTTPS Git, not SSH.
- A repo-scoped token for each workspace/task.
- App-bot author/committer identity.
- Central signing key storage.
- Token refresh through a Git credential helper or Git proxy.

Why customer-owned and dedicated rather than one global Craxii App?

- The bot can be named for this Craxii.
- The customer owns and can delete or rotate it.
- One App private-key compromise does not expose every Craxii customer.
- Provider attribution is more specific.
- It approximates hiring a dedicated coworker more closely.
- The manifest flow keeps registration to a one-time provider experience.

A global marketplace app is simpler but creates a shared signing root across every installation and attributes all actions to the same `craxii[bot]`. I would not make that the foundational identity architecture.

### What it supports

With the correct permissions, a GitHub App installation token can:

- clone and fetch private repositories;
- create commits and branches;
- push over HTTPS;
- create and update pull requests;
- comment on PRs and issues;
- manage labels and issue state;
- interact with checks, workflows, and deployments when explicitly permitted.

GitHub Apps request granular permissions and use installation tokens for HTTP-based Git with the Contents permission. [GitHub App permissions](https://docs.github.com/en/apps/creating-github-apps/registering-a-github-app/choosing-permissions-for-a-github-app)

Installation-token activity is attributed to the app, not Abhisht. GitHub explicitly recommends installation tokens for automations acting independently and says that activity is attributed to the app. [GitHub App best practices](https://docs.github.com/en/apps/creating-github-apps/about-creating-github-apps/best-practices-for-creating-a-github-app)

Configure Git commits as:

```text
CRXI-Abhisht-00[bot]
<BOT_USER_ID>+CRXI-Abhisht-00[bot]@users.noreply.github.com
```

GitHub’s maintained App-token action documents this bot commit identity. [GitHub App token action](https://github.com/actions/create-github-app-token#configure-git-cli-for-an-apps-bot-user)

### UX awkwardness

A GitHub App is not perfectly human-like:

- It carries a `[bot]` identity.
- It has no password, interactive inbox, or normal personal settings.
- Team membership, notifications, some assignments, social workflows, and user-only endpoints can be awkward.
- It may not behave exactly like a colleague account in every GitHub feature.

A machine user is superior if the overriding requirement is a literal user profile that can join teams and participate in every human workflow. GitHub permits machine users, but they are personal accounts with password/account security and consume an Enterprise seat. [GitHub’s App-versus-machine-user comparison](https://docs.github.com/en/apps/oauth-apps/building-oauth-apps/differences-between-github-apps-and-oauth-apps)

For Craxii’s core engineering workflow, the App’s short-lived, repository-selectable credentials and distinct actor outweigh the social awkwardness. Support a customer-managed machine-user mode later for organizations whose workflows genuinely require human-account semantics.

### Alternatives

| Mechanism | Verdict |
|---|---|
| GitHub App | Recommended default |
| Machine user | Optional enterprise compatibility mode |
| User OAuth identity | Wrong default: attributes activity to Abhisht and inherits user access |
| Fine-grained PAT | Fallback only; manual account-bound lifecycle and organization approval/expiry policies |
| Classic PAT | Reject |
| Deploy key | Useful for one-repo Git transport only; no issues/PR/API coworker identity |
| Persistent SSH key | Reject as default |
| GitHub SSH CA | Interesting for Enterprise machine users, but not needed for App-based Git; individual issued certs cannot be selectively revoked |

GitHub itself generally prefers Apps over OAuth Apps because Apps have finer permissions, repository selection, and short-lived tokens. [Official comparison](https://docs.github.com/en/apps/oauth-apps/building-oauth-apps/differences-between-github-apps-and-oauth-apps)

---

## 6. Cloud identity deep dive

Craxii’s hosting identity and customer-cloud identity must be separate.

The hosting identity means:

> “This compute belongs to Craxii’s infrastructure.”

The customer identity means:

> “This Craxii may perform these actions in this customer project/account.”

Never attach customer AWS, Azure, or GCP authority to the base workstation instance role.

| Cloud | Standing customer delegation | Execution credential | Provider-visible actor |
|---|---|---|---|
| AWS | Customer IAM role per Craxii/project/environment | STS role session | Customer role + `SourceIdentity`/session tags |
| Azure | Customer-tenant service principal or user-assigned managed identity with federated credential | Entra access token | Customer service principal/managed identity |
| GCP | Workload Identity Federation plus dedicated service account, or direct federated principal | STS/federated token and usually SA impersonation token | Service account plus external subject delegation info |

### AWS

For Craxii hosted in AWS, V0 should use:

```text
Craxii hardened broker role
    -> cross-account AssumeRole
    -> customer-created Craxii project role
```

Require:

- unique External ID;
- one role per project/environment or meaningful authorization boundary;
- narrow permission policy;
- session policy intersection when useful;
- `SourceIdentity=CRXI-Abhisht-00`;
- session name or tags containing task/workspace IDs;
- CloudTrail enabled.

AWS `SourceIdentity` persists through role chaining and is recorded in CloudTrail, although Craxii should avoid unnecessary chains. [AWS SourceIdentity documentation](https://docs.aws.amazon.com/IAM/latest/UserGuide/id_credentials_temp_control-access_monitor.html)

Mature or non-AWS hosting can use Craxii’s OIDC issuer with `AssumeRoleWithWebIdentity`, exact issuer/audience/subject conditions, and a project-specific subject. IAM Roles Anywhere is another option for X.509 workloads, but asking every customer to configure a CA trust anchor is usually less convenient than OIDC or cross-account IAM. It is valuable when the customer already has PKI. [IAM Roles Anywhere](https://docs.aws.amazon.com/rolesanywhere/latest/userguide/introduction.html)

### Azure

If Craxii runs on Azure in the customer’s tenant, use a user-assigned managed identity.

If Craxii runs elsewhere:

1. Customer creates a single-tenant app/service principal or user-assigned managed identity.
2. Customer configures a federated identity credential trusting Craxii’s issuer.
3. `issuer`, `audience`, and immutable `subject` identify the exact Craxii/workspace class.
4. The customer assigns Azure RBAC roles to that service principal.
5. Craxii exchanges its workload assertion for an Entra token.

Microsoft’s workload identity federation is explicitly designed for external OIDC workloads to exchange external tokens without storing client secrets. [Entra workload federation](https://learn.microsoft.com/en-us/entra/workload-id/workload-identity-federation-create-trust)

Do not attach every customer Azure role to Craxii’s hosting managed identity. Microsoft notes that all code on an Azure VM can request tokens for any managed identity available on that VM. [Managed-identity VM security boundary](https://learn.microsoft.com/en-us/entra/identity/managed-identities-azure-resources/how-to-use-vm-token)

### GCP

Use:

```text
Craxii workload assertion
    -> customer Workload Identity Pool
    -> exact external principal
    -> dedicated service account impersonation
    -> short-lived Google access token
```

Prefer one service account per Craxii/project/environment. Bind `roles/iam.workloadIdentityUser` only to the exact subject or constrained attribute set—not the whole pool. Google recommends unique immutable subjects, application-specific service accounts, and audit logging of STS and impersonation events. [GCP WIF best practices](https://cloud.google.com/iam/docs/best-practices-for-using-workload-identity-federation)

GCP service-account access tokens default to one hour. [Short-lived service-account credentials](https://cloud.google.com/iam/docs/service-account-creds)

Google’s current Agent Identity preview is notable because it independently converges on SPIFFE-based per-agent identity, attestation, certificate-bound tokens, and a centralized auth manager for OAuth/API keys. It validates the direction but is pre-GA, vendor-specific, and tied to Google’s agent runtime, so Craxii should not base its portable architecture on it. [Google Agent Identity overview](https://cloud.google.com/iam/docs/agent-identity-overview)

---

## 7. Database, SSH, API, OAuth, and browser credential model

| Resource | Preferred primitive | Fallback |
|---|---|---|
| Managed database | Cloud/IAM/Entra identity | Dynamic DB user |
| Self-hosted database | Dynamic leased user | Unique static per-Craxii/project user |
| SSH | Short-lived SSH certificate or cloud-native login | Project-specific static key |
| Generic HTTP API | Auth-injecting proxy | Scoped command executor |
| OAuth SaaS | Central refresh grant, short access tokens | Provider-specific service account |
| Browser-only SaaS | Dedicated isolated browser profile/account | User session only as explicit high-risk mode |

### Databases

Preference order:

1. **RDS IAM authentication:** token replaces password and lasts 15 minutes. [RDS IAM authentication](https://docs.aws.amazon.com/AmazonRDS/latest/UserGuide/UsingWithRDS.IAMDBAuth.html)
2. **Azure SQL with Entra service principal or managed identity:** Microsoft recommends managed identities because they are passwordless. [Azure SQL service principals](https://learn.microsoft.com/en-us/azure/azure-sql/database/authentication-aad-service-principal)
3. **Cloud SQL automatic IAM DB authentication:** the connector refreshes one-hour access tokens for long-running applications. [Cloud SQL IAM authentication](https://cloud.google.com/sql/docs/mysql/iam-authentication)
4. **Vault dynamic users:** unique SQL username, lease, renewal, revocation, and improved audit. [Vault database engine](https://developer.hashicorp.com/vault/docs/secrets/databases)
5. **Static fallback:** unique role per Craxii/project/environment, never shared and never superuser.

A connection proxy helps with TLS, token renewal, and connection pooling. It does not replace database grants. Give Craxii the exact SQL permissions intended: staging read/write, production read-only, no role creation, no extension installation, and so forth.

Token or certificate expiration frequently applies to new authentication. An already-established database or SSH connection may survive until explicitly terminated. Revocation procedures must therefore include terminating active server-side sessions when that matters.

### SSH

The standing delegation should be:

> Target fleet trusts Craxii’s SSH CA for specified principals and target classes.

For each connection:

1. Generate an ephemeral key pair in the project workspace.
2. Send only the public key to the signer.
3. Mint a 5–30 minute certificate containing:
   - Craxii/project principal;
   - target/user principals;
   - task ID as key ID;
   - permitted extensions;
   - no agent forwarding by default.
4. Connect.
5. Destroy the private key after the task/session.

Vault’s SSH engine can sign client keys with controlled principals, extensions, and TTLs. [Vault signed SSH certificates](https://developer.hashicorp.com/vault/docs/secrets/ssh/signed-ssh-certificates)

Do not forward an SSH agent into remote hosts. An accessible agent socket may hide key bytes but still provides a signing oracle.

When the customer cannot trust a Craxii CA, prefer cloud-native mechanisms such as EC2 Instance Connect, GCP OS Login, or Azure Entra SSH before accepting a persistent private key.

### Generic API keys

Three execution modes:

1. **Operation proxy:** broker calls the provider API. Best secret isolation, least CLI flexibility.
2. **Credential-injecting HTTP proxy:** workspace sends an unauthenticated or mTLS-authenticated request to a Craxii proxy; the proxy adds the API key only for an approved hostname/path.
3. **Credentialed executor:** run a chosen command with the key provided through a pipe, memory file, or short-lived environment.

Mode 3 cannot protect the key from that command. Pipes and memory files reduce accidental persistence; they do not stop hostile code.

For providers with unscoped master keys, create a dedicated Craxii subaccount/key when possible. Broker policy cannot turn an unrestricted stolen key into a restricted one.

### OAuth

Use Authorization Code with PKCE for user consent.

- User connects once.
- Store refresh tokens centrally, encrypted and scoped to user, Craxii, provider, tenant, and granted scopes.
- Mint short-lived access tokens automatically.
- Use incremental authorization only when genuinely adding a new scope.
- Record the consent event and grant version.
- Support provider revocation and refresh-token rotation.

OAuth’s current security BCP recommends sender-constrained tokens where supported and refresh-token rotation or sender constraint to detect replay. [OAuth 2.0 Security BCP](https://www.rfc-editor.org/rfc/rfc9700.html)

Prefer explicit delegation semantics—Craxii acting for Abhisht—over impersonation where the provider supports it. OAuth Token Exchange formalizes that distinction. [RFC 8693](https://www.rfc-editor.org/rfc/rfc8693.html)

A provider may still require re-consent because:

- the refresh grant expired or was inactive;
- the user or administrator revoked it;
- requested scopes changed;
- organization policy changed;
- the provider requires periodic sign-in or MFA.

Craxii cannot abstract those away honestly.

### Browser credentials

Browser session state is credential material. Playwright warns that stored authentication state can contain cookies and headers capable of impersonating the account. [Playwright authentication-state guidance](https://playwright.dev/docs/auth)

Use:

- a dedicated Craxii service account where possible;
- one browser profile per provider/project/security boundary;
- an encrypted browser-profile volume separate from project shells;
- a remote browser executor, not a profile mounted in the general workspace;
- domain and download restrictions;
- provider session revocation hooks;
- no cookie export API to the model or shell;
- interactive user participation for initial login, MFA, passkey enrollment, and provider-forced risk challenges.

Cookies are often worse than API tokens: broad, bearer-style, long-lived, weakly scoped, and poorly audited. The fact that the model cannot read `Cookies` does not prevent it from using the authenticated browser to take actions.

---

## 8. Secret storage

### V0 recommendation

Use **AWS Secrets Manager backed by customer-managed AWS KMS keys** if Craxii’s control plane is hosted in AWS.

Store only unavoidable durable material:

- GitHub App private keys and webhook secrets;
- OAuth refresh grants;
- generic API keys;
- static fallback database/SSH credentials;
- provider client secrets;
- CA signing material if it cannot be held directly in HSM/PCA.

Do not store:

- installation tokens;
- STS sessions;
- ordinary access tokens;
- task leases;
- temporary SSH keys;
- workstation keys.

AWS Secrets Manager uses envelope encryption with KMS and records operations in CloudTrail. [Secrets Manager encryption](https://docs.aws.amazon.com/secretsmanager/latest/userguide/security-encryption.html) and [CloudTrail events](https://docs.aws.amazon.com/secretsmanager/latest/userguide/cloudtrail_log_entries.html)

Partition access so that:

- the main web/control-plane service cannot call `GetSecretValue`;
- each provider adapter gets only its connector/cell paths;
- no broker role has wildcard access to every tenant;
- metadata such as secret names avoids unnecessary customer disclosure;
- secret reads and rotations are audited;
- provider-specific rotation is implemented by adapters.

### Vault verdict

Do not operate self-managed Vault merely to say Craxii uses Vault.

Adopt managed Vault/HCP Vault—or a comparable dynamic-credential service—when you need enough of:

- dynamic database users;
- SSH CA issuance;
- PKI lifecycle;
- leased legacy credentials;
- multi-cloud dynamic secret engines;
- customer-managed Vault integration.

Vault materially solves dynamic issuance, renewal, and revocation. It does not solve workspace isolation or malicious model behavior. It also becomes a high-value operational root. [Vault’s architecture](https://developer.hashicorp.com/vault/docs/about-vault/how-vault-works)

Craxii’s credential abstraction should sit above Secrets Manager, Vault, customer vaults, and provider federation. Do not expose those backends directly to workspaces.

Using AWS, Azure, and GCP secret stores simultaneously for the same credential set creates synchronization and recovery problems. Use one authoritative store per regional/security cell. Later support “bring your own vault” for customers with sovereignty requirements.

---

## 9. Credential broker verdict

Yes—but call it an **Authority Service**, because “credential broker” understates its role and encourages a `get_secret(name)` API.

### Responsibilities

It should:

- authenticate agent/workspace/task identities;
- evaluate standing delegations and task constraints;
- maintain provider connection mappings;
- exchange or mint short-lived credentials;
- narrow provider tokens where the provider supports it;
- renew credentials transparently;
- perform credential injection or provider operations;
- track issued credential handles and expirations;
- execute revocation;
- produce causal audit events;
- isolate provider signing and refresh roots in adapters;
- support attenuated worker leases.

It should not:

- decide whether a model’s plan is intelligent;
- replace provider IAM;
- be treated as a sandbox boundary for a root-controlled caller;
- expose a generic “fetch any secret” interface;
- store source code or model memory;
- grant resources not present in standing delegation;
- run inside Craxii’s project VM.

### Authentication to the service

Requests should combine:

- short-lived mTLS/workload identity;
- agent ID;
- workspace and machine-session IDs;
- task lease;
- target resource and operation;
- grant/revocation version;
- tool-call ID.

A local helper socket may improve ergonomics, but the broker must authorize against the external identity and task—not merely Unix UID or socket possession.

### Credential return modes

Use a mixed strategy:

1. **Perform operation on behalf of caller** for structured APIs and high-risk secrets.
2. **Inject at a network/session proxy** for HTTP, DB, SSH, and browsers.
3. **Return short-lived credential material** to a scoped project workspace only when arbitrary CLI compatibility requires it.
4. **Return static material** only to an isolated command executor as a last resort.

A full operation-proxy-only architecture would cripple unknown tools and general shell work. A raw-token-only broker would fail to provide meaningful secret isolation. Craxii needs both.

HashiCorp Boundary makes the same useful distinction between credential brokering and credential injection: injected credentials remain on the connection worker, while brokered credentials reach the client. [Boundary credential management](https://developer.hashicorp.com/boundary/docs/concepts/credential-management)

### Outage behavior

Separate the administrative control plane from the authority data plane.

Authority replicas should have:

- replicated, signed delegation snapshots;
- a revocation stream and epochs;
- access to regional secret/KMS backends;
- independent provider adapters;
- no synchronous dependency on the chat or billing service.

During an outage:

- local edits, tests, builds, and commits continue;
- already issued provider credentials work until expiry;
- queued external operations wait;
- multiple authority replicas continue renewing if the main control plane is down;
- if every authority replica is unreachable, external work stops after token expiry.

Do not place GitHub App keys, OAuth refresh grants, or cloud federation roots on the workstation merely to survive a broker outage. That converts an availability problem into a durable compromise problem.

For low-risk/read-only workloads, longer task or provider leases may be a customer-configurable availability tradeoff. Production write authority should fail closed.

---

## 10. General-purpose shell problem

“The LLM never sees secrets” has three distinct meanings.

### 1. Secrets absent from model context

This is realistic and valuable.

- Do not serialize credentials into prompts.
- Redact known secret values and token patterns from tool output.
- Prevent shell history and command tracing from containing them.
- Do not put credentials in `.env`, repository files, Git config URLs, or global environment variables.
- Disable core dumps and credential-bearing debug logs.
- Log HMAC fingerprints or provider IDs instead of raw tokens.

This protects against accidental disclosure to the model provider and ordinary logging.

### 2. Secrets absent from the invoking process

This is possible only when an external proxy/executor performs authentication.

If Craxii calls:

```text
github.create_pull_request(...)
```

and a remote adapter adds the token, the workspace does not need the token.

If an arbitrary CLI requires:

```text
API_KEY=...
```

the CLI necessarily gains the key. It can print or transmit it.

### 3. Secrets protected from root on the host

This is not realistic for credentials delivered to that host.

Root can:

- inspect process memory and `/proc`;
- read environment variables and files;
- ptrace helpers;
- replace credential-helper binaries;
- invoke accessible broker sockets;
- use SSH agents as signing oracles;
- reach metadata endpoints;
- inspect browser profile state;
- instrument or alter TLS clients.

UID separation, `hidepid`, Yama, seccomp, memory files, and namespaces are defense-in-depth against accidents or non-root processes—not against guest root.

Even sender-constrained tokens do not make corrupted client software harmless when the attacker can also invoke the bound key. The OAuth Security BCP explicitly notes that proof-of-possession is undermined when the attacker obtains both token use and key access. [OAuth token replay discussion](https://www.rfc-editor.org/rfc/rfc9700.html#name-misuse-of-stolen-access-to)

Therefore the real boundary is:

> External authority plane + project-scoped VM/workspace + provider-side permissions.

A PTG workspace may practically possess its current PTG capability. It must not possess the renewable roots or other projects’ capabilities.

---

## 11. Project isolation

A single conversation does not require a single kernel.

For unrelated trust domains, use one persistent VM per project/security scope in V0:

```text
PTG VM
  PTG repository and caches
  PTG task/workspace identity
  PTG staging DB access
  PTG GitHub installation scope
  no Clxrity volume or credential path

Clxrity VM
  Clxrity repository and caches
  Clxrity workspace identity
  no PTG access
```

Craxii may have root inside each guest. The cloud hypervisor, not a container namespace, is the security boundary.

### Why OS users or ordinary containers are insufficient

If Craxii has root or the Docker socket on the host, it can cross OS-user and ordinary-container boundaries. Containers remain useful for reproducibility and process isolation inside one project, but not as the sole boundary between mutually untrusted customer projects.

VMs are the straightforward V0 answer. Later, microVMs or a mature sandbox such as gVisor can improve density. gVisor’s own documentation contrasts Linux namespace controls with stronger VM-like isolation and stresses that a compromised sandbox retains everything configured into that sandbox. [gVisor security model](https://gvisor.dev/docs/architecture_guide/intro/)

### Coherent-computer experience

The product can still present:

```text
/workspaces/ptg
/workspaces/clxrity
```

Commands are routed to the correct workspace invisibly. Long-running services, caches, databases, and project files persist in that workspace. The user speaks to one Craxii and receives one task history.

Do not share across project VMs:

- writable home directories;
- Docker sockets;
- credential caches;
- browser profiles;
- package-manager lifecycle state;
- shell startup scripts;
- SSH agents;
- mutable tool binaries.

Read-only content-addressed caches and signed platform tooling can be shared. Tools Craxii invents should remain project-local until explicitly promoted to a trusted, versioned tool layer.

### Cross-project tasks

A legitimate cross-project task receives a composite task envelope, but execution still occurs in separate workspaces. Artifacts and summaries move through the control plane.

A malicious PTG `README` can still try to persuade the model to invoke Clxrity tools—a confused-deputy attack. The task envelope must reject unrelated resource expansion even though the model knows those resources exist. This is why filesystem isolation alone is insufficient.

---

## 12. Threat model

| Threat | Severity | Likelihood | Primary mitigation | Residual truth |
|---|---:|---:|---|---|
| Prompt injection causes cross-project action | Critical | High | Task resource envelope, workspace isolation, taint/provenance, no global broker authority | Injection can still abuse valid current-task authority |
| Malicious npm/pip/test code steals credentials | High | High | Run uncredentialed by default; scoped helper/executor; project VM; restricted credentialed egress | It can steal any credential deliberately delivered to its process |
| Root compromise of monolithic VM | Critical | Medium–high | Do not build monolithic multi-project VM | One project VM remains fully compromised |
| Authority broker compromise | Critical | Medium–low | Tenant cells, adapter IAM isolation, HSM/KMS, no wildcard secret access, rapid revocation | Attacker can use online authorities in the compromised cell |
| Grant control-plane compromise | Critical | Medium–low | Separate grant and data planes, signed/versioned records, change audit, privileged deployment controls | Could authorize fraudulent work if broker accepts it |
| User account compromise | Critical | Medium | Passkeys, step-up for new standing grants, provider confirmation, notifications | Attacker can intentionally delegate broad access |
| Browser session theft/prompt injection | High | Medium–high | Browser enclave, dedicated accounts, profile partition, domain controls | Browser automation inherently exercises session authority |
| Secret appears in logs/model output | High | Medium | No global env, redaction, schema-limited tool output, DLP, no shell tracing | Deliberate encoding can defeat simple redaction |
| Stolen VM snapshot | High | Medium | No roots on disk, per-volume KMS, separate browser state, token TTL | Source code and project data remain sensitive |
| Static fallback API key stolen | High | Medium | Dedicated scoped key, proxy, project isolation, provider rotation | Unscoped providers impose unavoidable blast radius |
| Stale issued token after revoke | Medium–high | Expected | Short TTL, token inventory, provider-side deny/revoke | Revocation is not universally instantaneous |
| Malicious model/provider behavior | High | Low/unknown | No raw secrets, task envelope, provider policy, deterministic constraints | Active-task authority can still be misused |
| Compromised dependency exfiltrates source | High | Medium | Egress segmentation and uncredentialed test runners | Broad internet plus arbitrary code cannot guarantee source confidentiality |

The biggest practical risks are not cryptographic:

1. Prompt injection turning the model into a confused deputy.
2. Untrusted project code executing while credentials are present.
3. Collapsing multiple organizations into one root-controlled host.
4. Central broker/control-plane compromise.

Short-lived tokens help recovery and theft duration. They do not prevent an online compromised workspace from using current authority.

---

## 13. Product-experience audit

### Initial experience

The user may need to:

- create/sign into Craxii;
- create/install Craxii’s GitHub App and select repositories;
- deploy or approve one AWS role;
- create one Azure federated service principal/managed identity;
- configure one GCP WIF/service account connection;
- grant a DB role or have an administrator configure it;
- paste a generic API key once;
- complete OAuth consent once;
- log into a browser-only service once, including MFA.

After that, ordinary operation is:

```text
User: PTG’s review flow looks broken. Figure it out and fix it. Don’t deploy.

Craxii:
- selects the PTG workspace;
- uses PTG’s standing GitHub/log/DB delegations;
- obtains and renews execution credentials;
- investigates, edits, tests, pushes, and opens a PR;
- never asks about one-hour token renewal;
- never asks separately for clone, branch, push, or PR permission;
- honors “don’t deploy” through task policy.
```

### When the user must authorize again

Only when:

- adding a new provider or GitHub organization;
- adding repositories or permissions not covered by the existing App installation;
- adding OAuth scopes;
- connecting a new cloud account, subscription, project, database, or browser account;
- an administrator or provider revoked/expired the standing grant;
- provider policy requires re-consent, MFA, or risk reauthentication;
- Craxii needs authority genuinely outside the established delegation;
- the user configured a standing high-risk boundary that requires step-up, such as first-time production mutation.

Token expiry, VM replacement, a ten-hour task, and ordinary command execution are not authorization events.

---

## 14. Tradeoff table

Scores are 1–5, where 5 is best. Complexity is stated separately.

| Architecture | UX | Autonomy | Security | Blast radius | Operability | Portability | Audit | Recovery | Arbitrary engineering | Complexity |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|---|
| A. Static secrets on persistent VM | 5 | 5 | 1 | 1 | 2 | 2 | 1 | 1 | 5 | Low initially |
| B. Central secret store + local broker on same VM | 5 | 5 | 2 | 2 | 3 | 4 | 4 | 4 | 5 | Medium |
| C. External authority service + federation + secret fallback + project VMs | 5 | 5 | 4 | 4 | 4 | 4 | 5 | 5 | 4.5 | High but justified |
| D. Every provider operation performed by remote structured proxies | 3 | 2 | 5 | 5 | 2 | 2 | 5 | 5 | 2 | Very high |

Architecture B is much better operationally than A but is frequently overclaimed. It protects renewable material and improves audit; it does not contain root on the broker’s host.

Architecture C is the correct Craxii foundation.

Architecture D should be used selectively for sensitive providers, not as the universal computing model.

---

## 15. V0 versus eventual architecture

### Minimum architecture for the first real Craxii

Build:

1. Durable internal Craxii principal and ownership model.
2. Versioned delegation and revocation records.
3. External, multi-AZ Authority Service.
4. AWS Secrets Manager + KMS for unavoidable roots.
5. Dedicated customer-owned GitHub App via manifest flow.
6. GitHub HTTPS credential helper with repo-scoped installation tokens.
7. AWS cross-account role support with External ID and SourceIdentity.
8. Central OAuth refresh handling and generic-key storage.
9. One persistent EC2 VM and encrypted EBS volume per project/security scope.
10. Agent root inside the project VM, but no customer permissions on its EC2 instance profile.
11. Cloud-instance bootstrap identity followed by short-lived mTLS workspace certificates.
12. Task resource envelopes.
13. No credentials in global environment or home-directory dotfiles.
14. A structured API proxy plus CLI credential-provider fallback.
15. Causal audit records tied to task, model turn, tool call, workspace, credential issuance, and provider request.
16. One-click quarantine and revoke workflow.

Do not require SPIRE initially. Use a narrowly designed certificate/OIDC issuer and adopt SPIFFE-compatible identifiers so migration is possible.

Do not run Vault initially unless dynamic database or SSH issuance is a V0 requirement. Secrets Manager plus cloud-native identity is enough for the first production system.

A single V0 VM is acceptable only while every repository and authority mounted there belongs to one security scope. The moment PTG and Clxrity are mutually untrusted, they require separate VMs.

### Mature architecture

Add:

- SPIFFE/SPIRE or an equivalent managed workload identity layer;
- vTPM-bound workstation keys and measured enrollment;
- regional authority cells and replicated revocation state;
- customer-owned or tenant-cell provider signers;
- dynamic DB/SSH credentials through managed Vault or customer vaults;
- per-task disposable workers and microVMs;
- persistent project volumes independent of worker lifecycle;
- credential-injecting egress proxies;
- sender-constrained OAuth tokens when providers support them;
- isolated browser workers and virtual passkey storage;
- append-only/WORM audit export;
- customer-controlled retention, residency, and encryption keys;
- attenuated worker capabilities.

### Architectural traps

Avoid:

- equating Craxii with one VM ID;
- a durable Craxii private key on disk;
- user PATs or personal cloud credentials;
- a shared GitHub App signing root across every customer if dedicated Apps are feasible;
- one global Linux user or broker socket for all projects;
- attaching customer roles to the hosting VM profile;
- shared writable `$HOME`, Docker socket, browser profile, or credential cache;
- backing up active tokens with VM snapshots;
- making every CLI consume global environment credentials;
- treating containers or UIDs as boundaries against guest root;
- exposing Vault or Secrets Manager directly to the model;
- coupling renewal to the chat/control-plane API;
- introducing Biscuit/macaroons before there is a real offline attenuation requirement.

### Biscuit/macaroons verdict

Do not use a custom capability-token format in V0.

Biscuit and macaroons provide genuine offline attenuation: a holder can derive narrower credentials without contacting the issuer. But they remain bearer-style capabilities, require a coherent policy vocabulary, and add revocation-state distribution. Biscuit’s own documentation says revocation remains external state. [Biscuit attenuation](https://doc.biscuitsec.org/reference/specifications) and [revocation model](https://www.biscuitsec.org/docs/guides/revocation/)

For V0, use:

- short-lived workload identity;
- a signed task lease, ideally mTLS-bound;
- central policy evaluation;
- standard provider IAM;
- central child-lease issuance for workers.

Revisit Biscuit when temporary workers must attenuate authority while disconnected from the broker. Until then, conventional identity plus centralized exchange is simpler and more revocable.

---

## 16. Concrete recommendation

**Craxii principal:**  
Immutable internal UUID and ownership record. No permanent agent private key on a workstation.

**User identity:**  
OIDC/passkey-based user account with step-up only for new or expanded standing grants.

**Workstation identity:**  
Hosting-cloud instance identity for bootstrap, ephemeral vTPM-backed key where available, short-lived X.509 workload certificate containing agent/workspace/machine-session claims.

**Root of trust:**  
Craxii cloud organization, KMS/HSM-backed issuer keys, signed grant state, provider-native authorization, and user account recovery. No exportable universal master credential.

**Credential broker:**  
External HA Authority Service, separated from project VMs and logically separated from the ordinary web/control plane.

**Internal authorization:**  
Typed delegation records plus signed task leases. Use standard JWT/mTLS/OAuth token-exchange concepts; do not invent a Biscuit-like token initially.

**Secret store:**  
AWS Secrets Manager with customer-managed KMS keys and adapter-specific IAM. Managed Vault later for dynamic DB/SSH/PKI.

**GitHub:**  
Customer-owned dedicated GitHub App per Craxii × organization, created through App Manifest; installation tokens; HTTPS Git; one-repository token per task where possible.

**AWS:**  
Customer-created IAM role per project/environment, cross-account trust to hardened broker role with External ID, SourceIdentity, session tags, and short STS sessions. OIDC federation later or for non-AWS hosting.

**Azure:**  
Customer-tenant service principal or user-assigned managed identity with a federated identity credential trusting an exact Craxii subject; Azure RBAC defines outer permissions.

**GCP:**  
Customer Workload Identity Pool plus exact Craxii subject and dedicated service account impersonation; one service account per project/environment.

**Databases:**  
IAM/Entra/service-account authentication first; connector/proxy second; Vault dynamic users third; unique least-privileged static account last.

**SSH:**  
Short-lived SSH certificates from a customer-trusted Craxii CA or Vault; cloud-native login where available; no persistent general-purpose SSH private key.

**Generic API keys:**  
Dedicated scoped key stored centrally. Prefer auth-injecting proxy; otherwise deliver only to a project-scoped executor and accept that the process can read it.

**OAuth:**  
Authorization Code + PKCE, central encrypted refresh grant, automatic rotation/refresh, incremental scopes, explicit revocation.

**Browser sessions:**  
Dedicated account and isolated remote browser profile per provider/project; encrypted separately; no profile or cookie database in the general shell workspace.

**Project isolation:**  
Persistent VM per project/security scope in V0. Per-task VMs or microVMs later. One logical Craxii experience across them.

**Frontier model:**  
Receives code, context, and redacted tool results; never receives raw secret material intentionally. It remains capable of exercising active task authority through tools.

**Audit:**  
Append-only events containing user, agent, task, model/turn, tool call, workspace, machine session, process/executor, delegation version, credential issuance ID/fingerprint, provider actor/request ID, and result.

**Revocation:**  
Central epoch change in seconds; provider token revocation or IAM deny where supported; bounded residual token lifetime; workspace quarantine; rotate exposed static credentials; acknowledge that copied data cannot be revoked.

**Recovery:**  
Reprovision clean VM, enroll a new workload key, reattach clean project data, and reacquire credentials from unchanged standing delegations. Never restore workstation identity or active credential caches.

**Future workers:**  
Each worker receives its own workload identity and a child task lease whose resources, operations, environment, and expiration are a subset of the parent. Workers never inherit Craxii’s complete identity, refresh grants, or provider roots.

---

## 17. Open questions

These materially change the design:

1. **Is Craxii a consumer-owned agent, an organization-owned employee, or both?**  
   Organization ownership affects offboarding, shared projects, GitHub App ownership, and who may revoke delegations.

2. **Will Craxii’s control plane always be Craxii-hosted, or must enterprise customers run authority components in their cloud?**  
   Customer-hosted brokers substantially reduce central blast radius but change operations and availability.

3. **Is a logical computer composed of separate project VMs acceptable as long as the UX is seamless?**  
   If one literal root-controlled VM is non-negotiable, strong cross-project credential isolation is impossible.

4. **What production authority will the product permit by default?**  
   Provider IAM architecture is unchanged, but incident severity and required deterministic guardrails differ greatly between read-only production access and production mutation/deployment.

5. **What broker outage duration must autonomous work tolerate?**  
   An hour, a day, and a week imply different token TTL, regional replication, and fail-open/fail-closed tradeoffs.

6. **Must browser-only services and generic master API keys be supported in the first production release?**  
   They dominate residual secret-exposure risk and may justify a separate high-risk connection tier.

7. **What regulatory and residency boundaries are expected?**  
   This determines tenant cells, per-tenant keys, customer-managed vault support, audit immutability, and whether a central US-hosted secret store is acceptable.

The most important product decision is number 3. The security architecture is defensible if “one Craxii, one computer” means one continuous relationship and coherent environment spanning isolated project computers. It is not defensible if it requires putting every customer trust domain behind one root-controlled kernel.
