//! Fixed facade-name → contract-interface table for the "Convert facade call
//! to dependency injection" quickfix
//! (`src/actions/facade_to_di_action.rs`).
//!
//! Framework-fixed data (every Laravel installation ships the same facade
//! names bound to the same contracts), not something scanned from the user's
//! project — a plain const table, unlike the workspace-scanned
//! `env`/`config`/`route` indexes elsewhere in this module.

/// `(facade short name, contract FQCN, suggested property/param name)`.
/// Covers the facades most commonly called statically from inside a
/// controller/service method — not exhaustive.
pub(crate) const FACADE_CONTRACTS: &[(&str, &str, &str)] = &[
    ("Cache", "Illuminate\\Contracts\\Cache\\Repository", "cache"),
    ("Auth", "Illuminate\\Contracts\\Auth\\Factory", "auth"),
    ("Log", "Psr\\Log\\LoggerInterface", "log"),
    ("DB", "Illuminate\\Database\\ConnectionInterface", "db"),
    (
        "Storage",
        "Illuminate\\Contracts\\Filesystem\\Factory",
        "storage",
    ),
    ("Mail", "Illuminate\\Contracts\\Mail\\Mailer", "mailer"),
    ("Queue", "Illuminate\\Contracts\\Queue\\Queue", "queue"),
    (
        "Event",
        "Illuminate\\Contracts\\Events\\Dispatcher",
        "events",
    ),
    (
        "Session",
        "Illuminate\\Contracts\\Session\\Session",
        "session",
    ),
    (
        "Config",
        "Illuminate\\Contracts\\Config\\Repository",
        "config",
    ),
    (
        "Validator",
        "Illuminate\\Contracts\\Validation\\Factory",
        "validator",
    ),
    ("Redis", "Illuminate\\Redis\\RedisManager", "redis"),
    ("Http", "Illuminate\\Http\\Client\\Factory", "http"),
];

/// The contract FQCN and suggested property name for `facade_name`, if it's
/// one of the known facades above (exact-case match — real code always
/// calls these in their canonical PascalCase form).
pub(crate) fn lookup(facade_name: &str) -> Option<(&'static str, &'static str)> {
    FACADE_CONTRACTS
        .iter()
        .find(|(name, _, _)| *name == facade_name)
        .map(|(_, contract, prop)| (*contract, *prop))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn looks_up_known_facade() {
        assert_eq!(
            lookup("Cache"),
            Some(("Illuminate\\Contracts\\Cache\\Repository", "cache"))
        );
    }

    #[test]
    fn none_for_unknown_name() {
        assert_eq!(lookup("SomeUnrelatedClass"), None);
    }

    #[test]
    fn case_sensitive_no_match_for_lowercase() {
        assert_eq!(lookup("cache"), None);
    }
}
