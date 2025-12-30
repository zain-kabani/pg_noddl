# pg_noddl justfile
# Run `just --list` to see all available recipes

# Default PostgreSQL version
PG_VERSION := "pg17"

# Show available recipes
default:
    @just --list

# Check compilation (default: pg17)
check pg=PG_VERSION:
    cargo check --features {{pg}}

# Format code
fmt:
    cargo fmt

# Check formatting
fmt-check:
    cargo fmt -- --check

# Run clippy lints (default: pg17)
clippy pg=PG_VERSION:
    cargo clippy --features {{pg}} -- -D warnings

# Run tests (default: pg17)
test pg=PG_VERSION:
    cargo pgrx test {{pg}}

# Run SQL regression tests (default: pg17)
test-sql pg=PG_VERSION:
    #!/usr/bin/env bash
    set -e
    PGBIN="$(dirname "$(cargo pgrx info pg-config {{pg}})")"
    cargo pgrx start {{pg}}

    case "{{pg}}" in
        pg17) PORT=28817 ;;
        pg18) PORT=28818 ;;
        *) echo "Unknown PG version: {{pg}}"; exit 1 ;;
    esac

    "$PGBIN/psql" -h localhost -p $PORT -d postgres -c \
        "DROP EXTENSION IF EXISTS pg_noddl CASCADE; \
         DROP TABLE IF EXISTS test_table; \
         DROP PUBLICATION IF EXISTS test_pub; \
         DROP ROLE IF EXISTS noddl_admin; \
         DROP ROLE IF EXISTS regular_user;" 2>/dev/null || true

    ACTUAL=$(mktemp)
    "$PGBIN/psql" -h localhost -p $PORT -d postgres -f sql/test.sql > "$ACTUAL" 2>&1

    if diff -u expected/test.out "$ACTUAL"; then
        echo "✓ SQL regression tests passed"
        rm "$ACTUAL"
    else
        echo "✗ SQL regression tests failed - see diff above"
        rm "$ACTUAL"
        exit 1
    fi

# Update expected test output after intentional changes (default: pg17)
test-sql-update pg=PG_VERSION:
    #!/usr/bin/env bash
    set -e
    PGBIN="$(dirname "$(cargo pgrx info pg-config {{pg}})")"
    cargo pgrx start {{pg}}

    case "{{pg}}" in
        pg17) PORT=28817 ;;
        pg18) PORT=28818 ;;
        *) echo "Unknown PG version: {{pg}}"; exit 1 ;;
    esac

    "$PGBIN/psql" -h localhost -p $PORT -d postgres -c \
        "DROP EXTENSION IF EXISTS pg_noddl CASCADE; \
         DROP TABLE IF EXISTS test_table; \
         DROP PUBLICATION IF EXISTS test_pub; \
         DROP ROLE IF EXISTS noddl_admin; \
         DROP ROLE IF EXISTS regular_user;" 2>/dev/null || true

    "$PGBIN/psql" -h localhost -p $PORT -d postgres -f sql/test.sql > expected/test.out 2>&1
    echo "Updated expected/test.out"

# Build extension package (default: pg17)
package pg=PG_VERSION:
    cargo pgrx package --pg-config "$(cargo pgrx info pg-config {{pg}})"

# Install extension (default: pg17)
install pg=PG_VERSION:
    cargo pgrx install --pg-config "$(cargo pgrx info pg-config {{pg}})"

# Start psql with extension loaded (default: pg17)
run pg=PG_VERSION:
    cargo pgrx run {{pg}}

# Clean build artifacts
clean:
    cargo clean
