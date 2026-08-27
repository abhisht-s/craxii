use sha2::{Digest, Sha256};
use sqlx::{AssertSqlSafe, Row, SqliteConnection};

use super::error::{SqliteAdapterError, SqliteFailureKind};

pub const MAX_SUPPORTED_SCHEMA_VERSION: i64 = 1;
pub(super) const CORE_MIGRATION_VERSION: i64 = 1;
pub(super) const CORE_MIGRATION_DESCRIPTION: &str = "core durable schema";
pub(super) const SQLX_CHECKSUM_LENGTH: usize = 48;

pub(super) static MIGRATOR: sqlx::migrate::Migrator = sqlx::migrate!("./migrations");

pub(super) const PRODUCT_TABLES: &[&str] = &[
    "client_commands",
    "client_devices",
    "conversations",
    "craxii_principals",
    "messages",
    "runtime_instances",
    "work_items",
    "workspaces",
    "workstations",
];

pub(super) const PRODUCT_INDEXES: &[&str] = &[
    "ix_messages_conversation",
    "ix_runtime_instances_craxii_state",
    "ix_work_items_nonterminal_by_runtime",
    "ix_work_items_queued_fifo",
    "ix_workspaces_craxii_id",
    "ix_workstations_craxii_id",
    "ux_client_devices_token_hash",
    "ux_conversations_craxii_kind",
    "ux_messages_client_identity",
    "ux_messages_produced_by_work",
    "ux_work_items_conversation_ordinal",
    "ux_work_items_current_model_invocation",
    "ux_work_items_current_tool_execution",
    "ux_work_items_one_active_per_conversation",
    "ux_workspaces_workstation_logical_name",
];

// Filled from the deterministic Stage 6 structural manifest after applying migration 0001 with
// the bundled SQLite engine. The generation test fails closed if this value ever becomes stale.
const CURRENT_SCHEMA_FINGERPRINT: &str =
    "f4636df22c635c90ac469f49f2ac3a9ccb38956f1670d26ab566140a137f5521";

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DatabaseDisposition {
    Empty,
    MigratedUninitialized,
    Current,
    NewerSchema,
    Corrupt,
    Inconsistent,
}

impl DatabaseDisposition {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Empty => "empty",
            Self::MigratedUninitialized => "migrated_uninitialized",
            Self::Current => "current",
            Self::NewerSchema => "newer_schema",
            Self::Corrupt => "corrupt",
            Self::Inconsistent => "inconsistent",
        }
    }
}

pub(super) async fn classify_schema(
    connection: &mut SqliteConnection,
) -> Result<DatabaseDisposition, SqliteAdapterError> {
    let user_version = sqlx::query_scalar::<_, i64>("PRAGMA user_version")
        .fetch_one(&mut *connection)
        .await
        .map_err(SqliteAdapterError::schema_query)?;
    if user_version != 0 {
        return Ok(DatabaseDisposition::Inconsistent);
    }

    let objects = sqlx::query_as::<_, (String, String)>(
        "SELECT name, type FROM sqlite_schema WHERE name NOT LIKE 'sqlite_%' ORDER BY name, type",
    )
    .fetch_all(&mut *connection)
    .await
    .map_err(SqliteAdapterError::schema_query)?;

    if objects.is_empty() {
        return Ok(DatabaseDisposition::Empty);
    }
    if !objects
        .iter()
        .any(|(name, object_type)| name == "_sqlx_migrations" && object_type == "table")
    {
        return Ok(DatabaseDisposition::Inconsistent);
    }

    validate_migration_table_shape(connection).await?;
    let migrations = load_migration_rows(connection).await?;
    if migrations.is_empty() {
        return Ok(
            if objects.as_slice() == [("_sqlx_migrations".to_owned(), "table".to_owned())] {
                DatabaseDisposition::MigratedUninitialized
            } else {
                DatabaseDisposition::Inconsistent
            },
        );
    }

    if !valid_contiguous_history(&migrations) {
        return Ok(DatabaseDisposition::Inconsistent);
    }
    if migrations
        .last()
        .is_some_and(|row| row.version > MAX_SUPPORTED_SCHEMA_VERSION)
    {
        return Ok(DatabaseDisposition::NewerSchema);
    }
    if migrations.len() != 1 || migrations[0].version != CORE_MIGRATION_VERSION {
        return Ok(DatabaseDisposition::Inconsistent);
    }

    if !has_exact_current_objects(&objects) || !current_schema_matches(connection).await? {
        return Ok(DatabaseDisposition::Inconsistent);
    }
    Ok(DatabaseDisposition::Current)
}

struct MigrationRow {
    version: i64,
    description: String,
    success: i64,
    checksum: Vec<u8>,
    execution_time: i64,
    storage_types_valid: bool,
}

async fn load_migration_rows(
    connection: &mut SqliteConnection,
) -> Result<Vec<MigrationRow>, SqliteAdapterError> {
    let rows = sqlx::query(
        "SELECT version, description, success, checksum, execution_time, \
         typeof(version) AS version_type, typeof(description) AS description_type, \
         typeof(installed_on) AS installed_on_type, typeof(success) AS success_type, \
         typeof(checksum) AS checksum_type, typeof(execution_time) AS execution_time_type \
         FROM _sqlx_migrations ORDER BY version",
    )
    .fetch_all(&mut *connection)
    .await
    .map_err(SqliteAdapterError::schema_query)?;

    rows.into_iter()
        .map(|row| {
            let storage_types_valid = row.try_get::<String, _>("version_type")? == "integer"
                && row.try_get::<String, _>("description_type")? == "text"
                && row.try_get::<String, _>("installed_on_type")? == "text"
                && row.try_get::<String, _>("success_type")? == "integer"
                && row.try_get::<String, _>("checksum_type")? == "blob"
                && row.try_get::<String, _>("execution_time_type")? == "integer";
            Ok(MigrationRow {
                version: row.try_get("version")?,
                description: row.try_get("description")?,
                success: row.try_get("success")?,
                checksum: row.try_get("checksum")?,
                execution_time: row.try_get("execution_time")?,
                storage_types_valid,
            })
        })
        .collect::<Result<Vec<_>, sqlx::Error>>()
        .map_err(SqliteAdapterError::schema_query)
}

fn valid_contiguous_history(rows: &[MigrationRow]) -> bool {
    let Some(embedded) = MIGRATOR
        .iter()
        .find(|migration| migration.version == CORE_MIGRATION_VERSION)
    else {
        return false;
    };
    if embedded.description.as_ref() != CORE_MIGRATION_DESCRIPTION
        || embedded.checksum.len() != SQLX_CHECKSUM_LENGTH
    {
        return false;
    }

    rows.iter().enumerate().all(|(index, row)| {
        row.version == i64::try_from(index + 1).unwrap_or(i64::MAX)
            && row.success == 1
            && row.execution_time >= 0
            && row.storage_types_valid
            && row.checksum.len() == SQLX_CHECKSUM_LENGTH
            && !row.description.is_empty()
            && (row.version != CORE_MIGRATION_VERSION
                || (row.description == embedded.description
                    && row.checksum.as_slice() == embedded.checksum.as_ref()))
    })
}

fn has_exact_current_objects(objects: &[(String, String)]) -> bool {
    let expected = std::iter::once(("_sqlx_migrations", "table"))
        .chain(PRODUCT_TABLES.iter().copied().map(|name| (name, "table")))
        .chain(PRODUCT_INDEXES.iter().copied().map(|name| (name, "index")))
        .map(|(name, object_type)| (name.to_owned(), object_type.to_owned()))
        .collect::<std::collections::BTreeSet<_>>();
    objects
        .iter()
        .cloned()
        .collect::<std::collections::BTreeSet<_>>()
        == expected
        && objects.len() == expected.len()
}

async fn current_schema_matches(
    connection: &mut SqliteConnection,
) -> Result<bool, SqliteAdapterError> {
    let manifest = structural_manifest(connection).await?;
    let fingerprint = sha256_hex(manifest.as_bytes());
    Ok(fingerprint == CURRENT_SCHEMA_FINGERPRINT)
}

pub(super) async fn structural_manifest(
    connection: &mut SqliteConnection,
) -> Result<String, SqliteAdapterError> {
    let mut manifest = String::new();

    let objects = sqlx::query(
        "SELECT type, name, tbl_name, sql FROM sqlite_schema \
         WHERE name NOT LIKE 'sqlite_%' AND name <> '_sqlx_migrations' \
         ORDER BY type, name",
    )
    .fetch_all(&mut *connection)
    .await
    .map_err(SqliteAdapterError::schema_query)?;
    for row in objects {
        push_fields(
            &mut manifest,
            "object",
            &[
                row.try_get::<String, _>("type")?,
                row.try_get::<String, _>("name")?,
                row.try_get::<String, _>("tbl_name")?,
                normalize_sql(&row.try_get::<String, _>("sql")?),
            ],
        );
    }

    let table_list = sqlx::query("PRAGMA table_list")
        .fetch_all(&mut *connection)
        .await
        .map_err(SqliteAdapterError::schema_query)?;
    let mut product_table_list = Vec::new();
    for row in table_list {
        let name: String = row.try_get("name")?;
        if PRODUCT_TABLES.contains(&name.as_str()) {
            product_table_list.push((
                name,
                row.try_get::<String, _>("type")?,
                row.try_get::<i64, _>("ncol")?,
                row.try_get::<i64, _>("wr")?,
                row.try_get::<i64, _>("strict")?,
            ));
        }
    }
    product_table_list.sort_by(|left, right| left.0.cmp(&right.0));
    if product_table_list.len() != PRODUCT_TABLES.len() {
        return Ok("missing-product-table".to_owned());
    }
    for (name, object_type, columns, without_rowid, strict) in product_table_list {
        push_fields(
            &mut manifest,
            "table_list",
            &[
                name.clone(),
                object_type,
                columns.to_string(),
                without_rowid.to_string(),
                strict.to_string(),
            ],
        );

        let table_xinfo = pragma_rows(connection, "table_xinfo", &name).await?;
        for row in table_xinfo {
            push_fields(
                &mut manifest,
                "table_xinfo",
                &[
                    name.clone(),
                    row.try_get::<i64, _>("cid")?.to_string(),
                    row.try_get::<String, _>("name")?,
                    row.try_get::<String, _>("type")?,
                    row.try_get::<i64, _>("notnull")?.to_string(),
                    row.try_get::<Option<String>, _>("dflt_value")?
                        .unwrap_or_else(|| "<null>".to_owned()),
                    row.try_get::<i64, _>("pk")?.to_string(),
                    row.try_get::<i64, _>("hidden")?.to_string(),
                ],
            );
        }

        let foreign_keys = pragma_rows(connection, "foreign_key_list", &name).await?;
        for row in foreign_keys {
            push_fields(
                &mut manifest,
                "foreign_key",
                &[
                    name.clone(),
                    row.try_get::<i64, _>("id")?.to_string(),
                    row.try_get::<i64, _>("seq")?.to_string(),
                    row.try_get::<String, _>("table")?,
                    row.try_get::<String, _>("from")?,
                    row.try_get::<Option<String>, _>("to")?
                        .unwrap_or_else(|| "<null>".to_owned()),
                    row.try_get::<String, _>("on_update")?,
                    row.try_get::<String, _>("on_delete")?,
                    row.try_get::<String, _>("match")?,
                ],
            );
        }

        let index_rows = pragma_rows(connection, "index_list", &name).await?;
        let mut indexes = Vec::new();
        for row in index_rows {
            indexes.push((
                row.try_get::<String, _>("name")?,
                row.try_get::<i64, _>("unique")?,
                row.try_get::<String, _>("origin")?,
                row.try_get::<i64, _>("partial")?,
            ));
        }
        indexes.sort_by(|left, right| left.0.cmp(&right.0));
        for (index_name, unique, origin, partial) in indexes {
            push_fields(
                &mut manifest,
                "index_list",
                &[
                    name.clone(),
                    index_name.clone(),
                    unique.to_string(),
                    origin,
                    partial.to_string(),
                ],
            );
            let index_xinfo = pragma_rows(connection, "index_xinfo", &index_name).await?;
            for row in index_xinfo {
                push_fields(
                    &mut manifest,
                    "index_xinfo",
                    &[
                        index_name.clone(),
                        row.try_get::<i64, _>("seqno")?.to_string(),
                        row.try_get::<i64, _>("cid")?.to_string(),
                        row.try_get::<Option<String>, _>("name")?
                            .unwrap_or_else(|| "<null>".to_owned()),
                        row.try_get::<i64, _>("desc")?.to_string(),
                        row.try_get::<String, _>("coll")?,
                        row.try_get::<i64, _>("key")?.to_string(),
                    ],
                );
            }
        }
    }
    Ok(manifest)
}

async fn pragma_rows(
    connection: &mut SqliteConnection,
    pragma: &str,
    object: &str,
) -> Result<Vec<sqlx::sqlite::SqliteRow>, SqliteAdapterError> {
    if !object
        .bytes()
        .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
    {
        return Err(SqliteAdapterError::new(
            SqliteFailureKind::InconsistentSchema,
        ));
    }
    let statement = format!("PRAGMA {pragma}('{object}')");
    sqlx::query(AssertSqlSafe(statement))
        .fetch_all(&mut *connection)
        .await
        .map_err(SqliteAdapterError::schema_query)
}

fn normalize_sql(sql: &str) -> String {
    sql.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn sha256_hex(input: &[u8]) -> String {
    use std::fmt::Write as _;

    let digest = Sha256::digest(input);
    let mut output = String::with_capacity(64);
    for byte in digest {
        write!(&mut output, "{byte:02x}").expect("writing to a String cannot fail");
    }
    output
}

fn push_fields(manifest: &mut String, record: &str, fields: &[String]) {
    manifest.push_str(record);
    for field in fields {
        manifest.push('|');
        manifest.push_str(&field.len().to_string());
        manifest.push(':');
        manifest.push_str(field);
    }
    manifest.push('\n');
}

async fn validate_migration_table_shape(
    connection: &mut SqliteConnection,
) -> Result<(), SqliteAdapterError> {
    let rows = sqlx::query("PRAGMA table_info('_sqlx_migrations')")
        .fetch_all(&mut *connection)
        .await
        .map_err(SqliteAdapterError::schema_query)?;
    let shape = rows
        .iter()
        .map(|row| {
            Ok((
                row.try_get::<String, _>("name")?,
                row.try_get::<String, _>("type")?,
                row.try_get::<i64, _>("notnull")?,
                row.try_get::<Option<String>, _>("dflt_value")?,
                row.try_get::<i64, _>("pk")?,
            ))
        })
        .collect::<Result<Vec<_>, sqlx::Error>>()
        .map_err(SqliteAdapterError::schema_query)?;
    if shape
        != [
            ("version".to_owned(), "BIGINT".to_owned(), 0, None, 1),
            ("description".to_owned(), "TEXT".to_owned(), 1, None, 0),
            (
                "installed_on".to_owned(),
                "TIMESTAMP".to_owned(),
                1,
                Some("CURRENT_TIMESTAMP".to_owned()),
                0,
            ),
            ("success".to_owned(), "BOOLEAN".to_owned(), 1, None, 0),
            ("checksum".to_owned(), "BLOB".to_owned(), 1, None, 0),
            ("execution_time".to_owned(), "BIGINT".to_owned(), 1, None, 0),
        ]
    {
        return Err(SqliteAdapterError::new(
            SqliteFailureKind::InconsistentSchema,
        ));
    }
    Ok(())
}

#[cfg(test)]
pub(super) fn expected_schema_fingerprint() -> &'static str {
    CURRENT_SCHEMA_FINGERPRINT
}

#[cfg(test)]
mod tests {
    use std::fs;

    use sqlx::{ConnectOptions, Connection};

    use super::*;

    #[tokio::test]
    async fn current_structural_manifest_has_the_frozen_fingerprint() {
        let path = std::env::temp_dir().join(format!(
            "craxii-schema-fingerprint-{}-{}.sqlite3",
            std::process::id(),
            uuid::Uuid::now_v7()
        ));
        let options = sqlx::sqlite::SqliteConnectOptions::new()
            .filename(&path)
            .create_if_missing(true)
            .foreign_keys(true);
        let mut connection = options.connect().await.unwrap();
        MIGRATOR.run(&mut connection).await.unwrap();
        let manifest = structural_manifest(&mut connection).await.unwrap();
        let actual = sha256_hex(manifest.as_bytes());
        connection.close().await.unwrap();
        fs::remove_file(path).unwrap();
        assert_eq!(actual, expected_schema_fingerprint());
    }
}
