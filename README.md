# pg_noddl

A PostgreSQL extension that blocks DDL statements during logical replication migrations.

## Why?

When performing a PostgreSQL major version upgrade using logical replication:

1. You set up logical replication from source (old version) to target (new version)
2. Data replicates continuously until cutover
3. **Problem**: Schema changes (DDL) are NOT replicated

If someone runs `ALTER TABLE` or `CREATE INDEX` on the source during the migration window, those changes won't exist on the target. This creates drift between instances and can cause the migration to fail or produce inconsistent results.

**pg_noddl solves this** by temporarily blocking DDL statements on the source database during the migration window.

## Installation

### Prerequisites

- Rust toolchain
- [cargo-pgrx](https://github.com/pgcentralfoundation/pgrx) v0.16.1
- PostgreSQL 17 or 18

### Install cargo-pgrx

```bash
cargo install cargo-pgrx --version 0.16.1
cargo pgrx init  # Downloads and compiles PostgreSQL for testing
```

### Build and Install

```bash
# Using just (recommended)
just install         # Install for pg17 (default)
just install pg18    # Install for pg18

# Or using cargo-pgrx directly
cargo pgrx install --pg-config /path/to/pg_config
```

## Usage

### Basic Usage

```sql
-- Load the extension
CREATE EXTENSION pg_noddl;

-- Enable DDL blocking before starting migration
SELECT pg_noddl_enable();

-- All DDL is now blocked
CREATE TABLE foo (id int);
-- ERROR: pg_noddl: CREATE TABLE is blocked during migration

-- DML continues to work normally
INSERT INTO existing_table VALUES (1, 'data');
-- OK

-- Disable blocking after migration completes
SELECT pg_noddl_disable();
```

### Admin Bypass

For long-running migrations, you may need to perform certain safe operations that can be manually synchronized to the target:

```sql
-- Enable with an admin user who can bypass safe DDL
SELECT pg_noddl_enable_with_admin('migration_admin');

-- As migration_admin, these operations are allowed:
CREATE ROLE new_app_user;                    -- OK (can sync manually)
GRANT SELECT ON my_table TO new_app_user;    -- OK (can sync manually)
COMMENT ON TABLE my_table IS 'description';  -- OK (metadata only)

-- But schema-changing DDL is still blocked for everyone:
CREATE TABLE new_table (id int);             -- ERROR (even as admin)
ALTER TABLE my_table ADD COLUMN foo int;     -- ERROR (even as admin)
```

**Admin-bypassable operations:**
- Role/User management (`CREATE/ALTER/DROP ROLE`)
- Permissions (`GRANT/REVOKE`)
- Ownership changes (`ALTER ... OWNER TO`)
- Extension management (`CREATE/ALTER EXTENSION`)
- Comments and security labels

### Status Functions

```sql
-- Check if blocking is enabled
SELECT pg_noddl_is_enabled();

-- Get the admin user (if any)
SELECT pg_noddl_admin_user();

-- Get extension version
SELECT pg_noddl_version();
```

## What Gets Blocked

| Category | Statements |
|----------|------------|
| Tables | `CREATE TABLE`, `ALTER TABLE`, `DROP TABLE` |
| Indexes | `CREATE INDEX`, `DROP INDEX` |
| Types | `CREATE TYPE`, `CREATE DOMAIN`, `CREATE ENUM` |
| Functions | `CREATE FUNCTION`, `ALTER FUNCTION`, `DROP FUNCTION` |
| Views | `CREATE VIEW`, `DROP VIEW` |
| Triggers | `CREATE TRIGGER`, `DROP TRIGGER` |
| Schemas | `CREATE SCHEMA`, `DROP SCHEMA` |
| Roles | `CREATE ROLE`, `ALTER ROLE`, `DROP ROLE` |
| Permissions | `GRANT`, `REVOKE` |
| Extensions | `CREATE EXTENSION`, `ALTER EXTENSION` |
| And more... | See [DESIGN.md](DESIGN.md) for full list |

## What's NOT Blocked

These operations continue to work normally:

| Category | Statements | Reason |
|----------|------------|--------|
| DML | `INSERT`, `UPDATE`, `DELETE` | Replicated by logical replication |
| Maintenance | `VACUUM`, `ANALYZE`, `REINDEX` | Operational, not schema changes |
| Data | `COPY`, `TRUNCATE` | Data operations, replicated |
| Session | `SET`, `SHOW`, transactions | Session-local, no schema impact |
| Replication | `CREATE/ALTER PUBLICATION/SUBSCRIPTION` | Needed to manage the migration itself |

## Development

### Prerequisites

```bash
# Install just (command runner)
brew install just  # macOS
# or: cargo install just

# Install cargo-pgrx (must match version in Cargo.toml)
cargo install cargo-pgrx --version 0.16.1

# Initialize pgrx - downloads and compiles PostgreSQL for testing
cargo pgrx init --pg17 download
```

The justfile assumes pgrx has been initialized. It uses `cargo pgrx info pg-config <version>` to dynamically discover PostgreSQL paths - no hardcoded paths required.

**Verify your setup:**
```bash
cargo pgrx info pg-config pg17
```

### Common Commands

```bash
just              # Show all available commands
just check        # Check compilation
just test         # Run tests (pg17)
just test-all     # Run tests (pg17 + pg18)
just run          # Start psql with extension loaded
just clippy       # Run lints
just fmt          # Format code
```

### Running Tests

```bash
just test         # Test with pg17 (default)
just test pg18    # Test with pg18
just test-all     # Test with both versions
```

### Interactive Testing

```bash
just run          # Starts psql with pg_noddl loaded

# In psql:
SELECT pg_noddl_enable();
CREATE TABLE test (id int);  -- Should fail
SELECT pg_noddl_disable();
CREATE TABLE test (id int);  -- Should succeed
```

## Error Messages

When DDL is blocked, you get a clear error with guidance:

```
ERROR: pg_noddl: CREATE TABLE is blocked during migration
Query: CREATE TABLE foo (id int)...
Hint: Schema changes are not replicated by logical replication.
      Disable pg_noddl with SELECT pg_noddl_disable() or apply
      this change manually to both source and target.
```

## Performance

pg_noddl is designed for minimal overhead:

- **When disabled**: Single atomic load (~1 CPU cycle)
- **When enabled**: Simple pattern match on statement type, no allocations
- **Error path only**: String formatting happens only when blocking a statement

The extension uses a ProcessUtility hook that runs on every utility statement, so performance in the hot path was a key design consideration.

## License

Proprietary - PlanetScale
