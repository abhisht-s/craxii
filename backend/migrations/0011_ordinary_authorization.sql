-- Ordinary work's authorization (go-live plan B2.1). A production work delivery names the running boot's
-- compute grant, period and epoch instead of a signed profile. Admission stores them with the receipt so the
-- task can be claimed and run later, including after a restart; the claim rechecks them against the VM's
-- current root-accepted grant. One row per admitted ordinary task, never changed or removed.
CREATE TABLE distribution_authorizations (
 environment TEXT NOT NULL CHECK(environment IN ('development','production')), central_work_id TEXT NOT NULL,
 local_work_id TEXT NOT NULL UNIQUE,
 period_id TEXT NOT NULL CHECK(length(period_id)=36), compute_grant_id TEXT NOT NULL CHECK(length(compute_grant_id)=36),
 authorization_epoch TEXT NOT NULL CHECK(length(authorization_epoch) BETWEEN 1 AND 20 AND authorization_epoch NOT GLOB '*[^0-9]*' AND authorization_epoch NOT GLOB '0*'),
 PRIMARY KEY(environment,central_work_id),
 FOREIGN KEY(environment,central_work_id) REFERENCES distribution_receipts(environment,central_work_id) ON UPDATE RESTRICT ON DELETE RESTRICT,
 FOREIGN KEY(local_work_id) REFERENCES distribution_receipts(local_work_id) ON UPDATE RESTRICT ON DELETE RESTRICT
) STRICT, WITHOUT ROWID;
CREATE TRIGGER distribution_authorization_immutable BEFORE UPDATE ON distribution_authorizations BEGIN SELECT RAISE(ABORT,'distribution_authorization_immutable'); END;
CREATE TRIGGER distribution_authorization_delete_guard BEFORE DELETE ON distribution_authorizations BEGIN SELECT RAISE(ABORT,'distribution_authorization_immutable'); END;
