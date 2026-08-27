# SQLite migrations

This directory is the embedded, forward-only SQLx migration source.

Stage 5 intentionally contains zero SQL migration files. It creates only SQLx-owned migration
metadata and keeps `MAX_SUPPORTED_SCHEMA_VERSION` at zero. Stage 6 owns migration `0001` and every
Craxii domain table; do not add a placeholder migration or schema object here.
