//! pg_noddl - Block DDL statements during logical replication migrations.
//!
//! This extension provides a simple way to prevent schema changes during
//! PostgreSQL major version upgrades using logical replication. When enabled,
//! all DDL statements are rejected with a clear error message, while DML
//! (INSERT, UPDATE, DELETE) and other safe operations continue normally.

use pgrx::pg_sys::ffi::pg_guard_ffi_boundary;
use pgrx::prelude::*;
use std::ffi::{CStr, CString};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::RwLock;

pgrx::pg_module_magic!();

// Global state for DDL blocking
static NODDL_ENABLED: AtomicBool = AtomicBool::new(false);

// Admin user who can bypass certain safe DDL operations
static ADMIN_USER: RwLock<Option<String>> = RwLock::new(None);

/// Returns the extension version.
#[pg_extern]
fn pg_noddl_version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}

/// Enable DDL blocking. All DDL statements will be rejected.
#[pg_extern]
fn pg_noddl_enable() -> bool {
    NODDL_ENABLED.store(true, Ordering::Release);
    pgrx::log!("pg_noddl: DDL blocking enabled");
    true
}

/// Enable DDL blocking with an admin user who can bypass safe DDL.
///
/// The admin user can execute certain "safe" DDL operations that can be
/// manually synchronized to the target database:
/// - Role/User management (CREATE/ALTER/DROP ROLE)
/// - Grant/Revoke permissions
/// - Ownership changes
/// - Extension management
/// - Comments and security labels
#[pg_extern]
fn pg_noddl_enable_with_admin(admin_username: &str) -> bool {
    // Validate that the role exists before enabling
    // get_role_oid with missing_ok=false will throw an error if role doesn't exist
    if !admin_username.is_empty() {
        let c_username = CString::new(admin_username)
            .expect("pg_noddl: invalid username contains null byte");
        unsafe {
            // This will raise a PostgreSQL ERROR if the role doesn't exist
            pg_sys::get_role_oid(c_username.as_ptr(), false);
        }
    }

    // Set admin user
    {
        let mut admin = ADMIN_USER.write().unwrap();
        *admin = Some(admin_username.to_string());
    }
    NODDL_ENABLED.store(true, Ordering::Release);
    pgrx::log!(
        "pg_noddl: DDL blocking enabled with admin user '{}'",
        admin_username
    );
    true
}

/// Disable DDL blocking. All statements will be allowed.
#[pg_extern]
fn pg_noddl_disable() -> bool {
    NODDL_ENABLED.store(false, Ordering::Release);
    // Clear admin user
    {
        let mut admin = ADMIN_USER.write().unwrap();
        *admin = None;
    }
    pgrx::log!("pg_noddl: DDL blocking disabled");
    true
}

/// Check if DDL blocking is currently enabled.
#[pg_extern]
fn pg_noddl_is_enabled() -> bool {
    NODDL_ENABLED.load(Ordering::Acquire)
}

/// Get the current admin user, if any.
#[pg_extern]
fn pg_noddl_admin_user() -> Option<String> {
    ADMIN_USER.read().ok().and_then(|guard| guard.clone())
}

/// Returns true if this statement can be bypassed by an admin user.
///
/// These are DDL operations that:
/// 1. Don't affect table data or structure (so replication continues to work)
/// 2. Can be manually applied to the target database after cutover
/// 3. Are commonly needed during migrations (e.g., permission grants)
fn is_admin_bypassable(node_tag: pg_sys::NodeTag) -> bool {
    use pg_sys::NodeTag::*;

    matches!(
        node_tag,
        // Role/User management - commonly needed during migrations
        T_CreateRoleStmt      // CREATE ROLE/USER
        | T_AlterRoleStmt     // ALTER ROLE/USER
        | T_AlterRoleSetStmt  // ALTER ROLE ... SET
        | T_DropRoleStmt      // DROP ROLE/USER

        // Permissions - don't affect data, can be synced manually
        | T_GrantStmt         // GRANT/REVOKE
        | T_GrantRoleStmt     // GRANT role TO role

        // Ownership - doesn't affect data structure
        | T_AlterOwnerStmt    // ALTER ... OWNER TO

        // Extension management - may be needed for migration tooling
        | T_CreateExtensionStmt      // CREATE EXTENSION
        | T_AlterExtensionStmt       // ALTER EXTENSION
        | T_AlterExtensionContentsStmt // ALTER EXTENSION ... ADD/DROP

        // Metadata - informational only, safe to differ
        | T_CommentStmt       // COMMENT ON
        | T_SecLabelStmt      // SECURITY LABEL
    )
}

/// Returns true if the statement should be blocked.
///
/// We use a blocklist approach: explicitly block DDL that would break
/// logical replication, allow everything else. This is less restrictive
/// and won't accidentally block legitimate operations.
fn is_blocked_statement(node_tag: pg_sys::NodeTag) -> bool {
    use pg_sys::NodeTag::*;

    matches!(
        node_tag,
        // Table DDL
        T_CreateStmt           // CREATE TABLE
        | T_AlterTableStmt     // ALTER TABLE
        | T_DropStmt           // DROP TABLE/INDEX/etc

        // Index DDL
        | T_IndexStmt          // CREATE INDEX

        // Sequence DDL
        | T_CreateSeqStmt      // CREATE SEQUENCE
        | T_AlterSeqStmt       // ALTER SEQUENCE

        // Schema DDL
        | T_CreateSchemaStmt   // CREATE SCHEMA

        // Function/Procedure DDL
        | T_CreateFunctionStmt // CREATE FUNCTION/PROCEDURE
        | T_AlterFunctionStmt  // ALTER FUNCTION/PROCEDURE

        // View DDL
        | T_ViewStmt           // CREATE VIEW

        // Type DDL
        | T_CompositeTypeStmt  // CREATE TYPE (composite)
        | T_CreateEnumStmt     // CREATE TYPE (enum)
        | T_AlterEnumStmt      // ALTER TYPE (enum)
        | T_CreateRangeStmt    // CREATE TYPE (range)
        | T_CreateDomainStmt   // CREATE DOMAIN
        | T_AlterDomainStmt    // ALTER DOMAIN

        // Trigger DDL
        | T_CreateTrigStmt     // CREATE TRIGGER

        // Rule DDL
        | T_RuleStmt           // CREATE RULE

        // Policy DDL (RLS)
        | T_CreatePolicyStmt   // CREATE POLICY
        | T_AlterPolicyStmt    // ALTER POLICY

        // Extension DDL
        | T_CreateExtensionStmt      // CREATE EXTENSION
        | T_AlterExtensionStmt       // ALTER EXTENSION
        | T_AlterExtensionContentsStmt // ALTER EXTENSION ... ADD/DROP

        // Role/User DDL
        | T_CreateRoleStmt     // CREATE ROLE/USER
        | T_AlterRoleStmt      // ALTER ROLE/USER
        | T_AlterRoleSetStmt   // ALTER ROLE ... SET
        | T_DropRoleStmt       // DROP ROLE/USER

        // Grant/Revoke (permissions not replicated)
        | T_GrantStmt          // GRANT/REVOKE
        | T_GrantRoleStmt      // GRANT role TO role

        // Ownership
        | T_AlterOwnerStmt     // ALTER ... OWNER TO

        // Rename operations
        | T_RenameStmt         // ALTER ... RENAME

        // Schema changes
        | T_AlterObjectSchemaStmt // ALTER ... SET SCHEMA

        // Database DDL
        | T_CreatedbStmt       // CREATE DATABASE
        | T_DropdbStmt         // DROP DATABASE
        | T_AlterDatabaseStmt  // ALTER DATABASE
        | T_AlterDatabaseSetStmt // ALTER DATABASE ... SET

        // Tablespace DDL
        | T_CreateTableSpaceStmt // CREATE TABLESPACE
        | T_DropTableSpaceStmt   // DROP TABLESPACE

        // Foreign data DDL
        | T_CreateFdwStmt           // CREATE FOREIGN DATA WRAPPER
        | T_AlterFdwStmt            // ALTER FOREIGN DATA WRAPPER
        | T_CreateForeignServerStmt // CREATE SERVER
        | T_AlterForeignServerStmt  // ALTER SERVER
        | T_CreateForeignTableStmt  // CREATE FOREIGN TABLE
        | T_CreateUserMappingStmt   // CREATE USER MAPPING
        | T_AlterUserMappingStmt    // ALTER USER MAPPING
        | T_DropUserMappingStmt     // DROP USER MAPPING

        // Conversion/Cast DDL
        | T_CreateConversionStmt // CREATE CONVERSION
        | T_CreateCastStmt       // CREATE CAST

        // Operator DDL
        | T_CreateOpClassStmt   // CREATE OPERATOR CLASS
        | T_CreateOpFamilyStmt  // CREATE OPERATOR FAMILY
        | T_AlterOpFamilyStmt   // ALTER OPERATOR FAMILY

        // Text search DDL
        | T_AlterTSDictionaryStmt // ALTER TEXT SEARCH DICTIONARY

        // Event trigger DDL
        | T_CreateEventTrigStmt // CREATE EVENT TRIGGER
        | T_AlterEventTrigStmt  // ALTER EVENT TRIGGER

        // Transform DDL
        | T_CreateTransformStmt // CREATE TRANSFORM

        // Statistics DDL
        | T_CreateStatsStmt    // CREATE STATISTICS
        | T_AlterStatsStmt     // ALTER STATISTICS

        // Access method DDL
        | T_CreateAmStmt       // CREATE ACCESS METHOD

        // Comment (metadata, but can be important)
        | T_CommentStmt        // COMMENT ON
        | T_SecLabelStmt       // SECURITY LABEL

        // NOTE: Publication/Subscription DDL is ALLOWED because it's needed
        // to manage the logical replication itself during migration.
        // T_CreatePublicationStmt, T_AlterPublicationStmt,
        // T_CreateSubscriptionStmt, T_AlterSubscriptionStmt,
        // T_DropSubscriptionStmt are intentionally NOT in this list.
    )
}

/// Get a human-readable name for common DDL statements.
fn statement_name(node_tag: pg_sys::NodeTag) -> &'static str {
    use pg_sys::NodeTag::*;

    match node_tag {
        T_CreateStmt => "CREATE TABLE",
        T_AlterTableStmt => "ALTER TABLE",
        T_DropStmt => "DROP",
        T_IndexStmt => "CREATE INDEX",
        T_CreateSeqStmt => "CREATE SEQUENCE",
        T_AlterSeqStmt => "ALTER SEQUENCE",
        T_CreateSchemaStmt => "CREATE SCHEMA",
        T_CreateFunctionStmt => "CREATE FUNCTION",
        T_AlterFunctionStmt => "ALTER FUNCTION",
        T_ViewStmt => "CREATE VIEW",
        T_CreateTrigStmt => "CREATE TRIGGER",
        T_RuleStmt => "CREATE RULE",
        T_GrantStmt => "GRANT/REVOKE",
        T_GrantRoleStmt => "GRANT ROLE",
        T_CreateRoleStmt => "CREATE ROLE",
        T_AlterRoleStmt => "ALTER ROLE",
        T_DropRoleStmt => "DROP ROLE",
        T_RenameStmt => "RENAME",
        T_CommentStmt => "COMMENT",
        T_AlterOwnerStmt => "ALTER OWNER",
        T_CreatedbStmt => "CREATE DATABASE",
        T_DropdbStmt => "DROP DATABASE",
        T_AlterDatabaseStmt => "ALTER DATABASE",
        T_CompositeTypeStmt => "CREATE TYPE",
        T_CreateEnumStmt => "CREATE TYPE (enum)",
        T_CreateDomainStmt => "CREATE DOMAIN",
        T_AlterDomainStmt => "ALTER DOMAIN",
        T_CreateExtensionStmt => "CREATE EXTENSION",
        T_AlterExtensionStmt => "ALTER EXTENSION",
        T_CreatePublicationStmt => "CREATE PUBLICATION",
        T_AlterPublicationStmt => "ALTER PUBLICATION",
        T_CreateSubscriptionStmt => "CREATE SUBSCRIPTION",
        T_AlterSubscriptionStmt => "ALTER SUBSCRIPTION",
        _ => "DDL statement",
    }
}

/// Check if current session user is the configured admin.
/// Returns false if no admin is configured or if user doesn't match.
///
/// This is only called when we're about to block a statement that might
/// be admin-bypassable, so it's acceptable to do the RwLock read here.
#[cold]
fn is_current_user_admin() -> bool {
    let admin_guard = match ADMIN_USER.read() {
        Ok(guard) => guard,
        Err(_) => return false,
    };

    let admin_name = match admin_guard.as_ref() {
        Some(name) => name,
        None => return false,
    };

    // Get current session user from PostgreSQL
    let current_user = unsafe {
        let user_id = pg_sys::GetSessionUserId();
        let role_name = pg_sys::GetUserNameFromId(user_id, false);
        if role_name.is_null() {
            return false;
        }
        CStr::from_ptr(role_name).to_str().unwrap_or("")
    };

    current_user == admin_name
}

/// Report a blocked statement - only called when we're about to error.
/// Separated from hot path to keep the main function lean.
#[cold]
#[inline(never)]
fn report_blocked_statement(node_tag: pg_sys::NodeTag, query_string: *const std::ffi::c_char) {
    let stmt_name = statement_name(node_tag);

    // Get the query text for the error message
    let query_text = if !query_string.is_null() {
        unsafe { CStr::from_ptr(query_string) }
            .to_str()
            .unwrap_or("<unknown>")
    } else {
        "<unknown>"
    };

    // Truncate query for error message
    let truncated_query: String = query_text.chars().take(100).collect();
    let ellipsis = if query_text.len() > 100 { "..." } else { "" };

    pgrx::error!(
        "pg_noddl: {} is blocked\nQuery: {}{}\nHint: \
        Disable pg_noddl with SELECT pg_noddl_disable(); or wait for it to be enabled.",
        stmt_name,
        truncated_query,
        ellipsis
    );
}

/// Register the ProcessUtility hook following pgrx best practices.
///
/// The hook and previous hook pointer are defined together to ensure
/// they reference the same state. We use pg_guard_ffi_boundary when
/// calling the previous hook since it may be a C function from another extension.
unsafe fn register_hook() {
    static mut PREV_PROCESS_UTILITY_HOOK: pg_sys::ProcessUtility_hook_type = None;

    /// ProcessUtility hook implementation.
    ///
    /// Hot path optimization: We use Acquire ordering for the atomic load which is
    /// cheaper than SeqCst. The check is the very first thing we do - if DDL blocking
    /// is disabled, we immediately fall through to the previous hook with zero
    /// additional overhead beyond the atomic load.
    #[pg_guard]
    unsafe extern "C-unwind" fn pg_noddl_process_utility(
        pstmt: *mut pg_sys::PlannedStmt,
        query_string: *const std::ffi::c_char,
        read_only_tree: bool,
        context: pg_sys::ProcessUtilityContext::Type,
        params: pg_sys::ParamListInfo,
        query_env: *mut pg_sys::QueryEnvironment,
        dest: *mut pg_sys::DestReceiver,
        qc: *mut pg_sys::QueryCompletion,
    ) {
        // Hot path: single atomic load with Acquire ordering (no locks, no allocations)
        // If disabled, skip straight to previous hook
        if NODDL_ENABLED.load(Ordering::Acquire) {
            // Only dereference pointers if blocking is enabled
            let utility_stmt = (*pstmt).utilityStmt;

            if !utility_stmt.is_null() {
                let node_tag = (*utility_stmt).type_;

                // is_blocked_statement is a simple match - no allocations, no locks
                if is_blocked_statement(node_tag) {
                    // Check if admin can bypass this statement type
                    // Only do the more expensive admin check if the statement is bypassable
                    let allow_admin_bypass =
                        is_admin_bypassable(node_tag) && is_current_user_admin();

                    if !allow_admin_bypass {
                        // Only do expensive string work when we're about to error
                        report_blocked_statement(node_tag, query_string);
                    }
                }
            }
        }

        // Call the previous hook or standard function
        // Use pg_guard_ffi_boundary for the previous hook since it may be
        // a C function from another extension
        if let Some(prev_hook) = PREV_PROCESS_UTILITY_HOOK {
            pg_guard_ffi_boundary(|| {
                prev_hook(
                    pstmt,
                    query_string,
                    read_only_tree,
                    context,
                    params,
                    query_env,
                    dest,
                    qc,
                )
            });
        } else {
            pg_sys::standard_ProcessUtility(
                pstmt,
                query_string,
                read_only_tree,
                context,
                params,
                query_env,
                dest,
                qc,
            );
        }
    }

    // Save the previous hook and install ours
    PREV_PROCESS_UTILITY_HOOK = pg_sys::ProcessUtility_hook;
    pg_sys::ProcessUtility_hook = Some(pg_noddl_process_utility);
}

/// Extension initialization - install the ProcessUtility hook.
#[pg_guard]
pub unsafe extern "C-unwind" fn _PG_init() {
    register_hook();
    pgrx::log!("pg_noddl: extension loaded");
}

#[cfg(any(test, feature = "pg_test"))]
#[pg_schema]
mod tests {
    use super::*;

    #[pg_test]
    fn test_version() {
        let version = crate::pg_noddl_version();
        assert!(!version.is_empty());
    }

    #[pg_test]
    fn test_enable_disable() {
        // Initially disabled
        assert!(!crate::pg_noddl_is_enabled());

        // Enable
        assert!(crate::pg_noddl_enable());
        assert!(crate::pg_noddl_is_enabled());

        // Disable
        assert!(crate::pg_noddl_disable());
        assert!(!crate::pg_noddl_is_enabled());
    }

    #[pg_test]
    fn test_allowed_statements_when_enabled() {
        crate::pg_noddl_enable();

        // These should all work
        Spi::run("SELECT 1").unwrap();
        Spi::run("SET search_path TO public").unwrap();
        Spi::run("SHOW search_path").unwrap();

        crate::pg_noddl_disable();
    }

    #[pg_test]
    fn test_blocklist() {
        use pg_sys::NodeTag::*;

        // Allowed statements (not in blocklist)
        assert!(!is_blocked_statement(T_TransactionStmt));
        assert!(!is_blocked_statement(T_VariableSetStmt));
        assert!(!is_blocked_statement(T_VacuumStmt));
        assert!(!is_blocked_statement(T_TruncateStmt));
        assert!(!is_blocked_statement(T_CopyStmt));
        assert!(!is_blocked_statement(T_ExplainStmt));

        // Blocked statements (in blocklist)
        assert!(is_blocked_statement(T_CreateStmt));
        assert!(is_blocked_statement(T_AlterTableStmt));
        assert!(is_blocked_statement(T_DropStmt));
        assert!(is_blocked_statement(T_IndexStmt));
        assert!(is_blocked_statement(T_GrantStmt));
        assert!(is_blocked_statement(T_CreateRoleStmt));
    }

    #[pg_test]
    fn test_admin_bypassable() {
        use pg_sys::NodeTag::*;

        // Admin-bypassable statements
        assert!(is_admin_bypassable(T_CreateRoleStmt));
        assert!(is_admin_bypassable(T_AlterRoleStmt));
        assert!(is_admin_bypassable(T_DropRoleStmt));
        assert!(is_admin_bypassable(T_GrantStmt));
        assert!(is_admin_bypassable(T_GrantRoleStmt));
        assert!(is_admin_bypassable(T_AlterOwnerStmt));
        assert!(is_admin_bypassable(T_CreateExtensionStmt));
        assert!(is_admin_bypassable(T_AlterExtensionStmt));
        assert!(is_admin_bypassable(T_CommentStmt));

        // NOT admin-bypassable (schema-changing DDL)
        assert!(!is_admin_bypassable(T_CreateStmt));
        assert!(!is_admin_bypassable(T_AlterTableStmt));
        assert!(!is_admin_bypassable(T_DropStmt));
        assert!(!is_admin_bypassable(T_IndexStmt));
        assert!(!is_admin_bypassable(T_CreateFunctionStmt));
        assert!(!is_admin_bypassable(T_ViewStmt));
    }

    #[pg_test]
    fn test_enable_with_admin() {
        // Initially no admin
        assert!(crate::pg_noddl_admin_user().is_none());

        // Create a test role
        Spi::run("CREATE ROLE migration_admin").unwrap();

        // Enable with admin
        assert!(crate::pg_noddl_enable_with_admin("migration_admin"));
        assert!(crate::pg_noddl_is_enabled());
        assert_eq!(
            crate::pg_noddl_admin_user(),
            Some("migration_admin".to_string())
        );

        // Disable clears admin
        assert!(crate::pg_noddl_disable());
        assert!(!crate::pg_noddl_is_enabled());
        assert!(crate::pg_noddl_admin_user().is_none());

        // Cleanup
        Spi::run("DROP ROLE migration_admin").unwrap();
    }

    #[pg_test]
    fn test_admin_bypass_grant() {
        // Get current user (should be the test superuser)
        let current_user: Option<String> =
            Spi::get_one("SELECT current_user::text").unwrap();
        let username = current_user.unwrap();

        // Enable with current user as admin
        crate::pg_noddl_enable_with_admin(&username);

        // Admin should be able to run GRANT (admin-bypassable)
        // Create a test role first (also admin-bypassable)
        let result = Spi::run("CREATE ROLE test_grant_role");
        assert!(result.is_ok(), "Admin should be able to CREATE ROLE");

        // Grant should also work
        let result = Spi::run("GRANT SELECT ON ALL TABLES IN SCHEMA public TO test_grant_role");
        assert!(result.is_ok(), "Admin should be able to GRANT");

        // Cleanup
        let _ = Spi::run("DROP ROLE test_grant_role");
        crate::pg_noddl_disable();
    }

    #[pg_test]
    #[should_panic(expected = "pg_noddl")]
    fn test_admin_cannot_bypass_table_ddl() {
        // Get current user
        let current_user: Option<String> =
            Spi::get_one("SELECT current_user::text").unwrap();
        let username = current_user.unwrap();

        // Enable with current user as admin
        crate::pg_noddl_enable_with_admin(&username);

        // Admin should NOT be able to CREATE TABLE (not admin-bypassable)
        // This should panic with pg_noddl error
        Spi::run("CREATE TABLE test_admin_table (id int)").unwrap();
    }
}

/// This module is required by `cargo pgrx test`.
#[cfg(test)]
pub mod pg_test {
    pub fn setup(_options: Vec<&str>) {
        // No setup needed
    }

    #[must_use]
    pub fn postgresql_conf_options() -> Vec<&'static str> {
        vec![]
    }
}
