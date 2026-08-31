//! guru → kikimimi rename: small shared helpers for reading a `KIKIMIMI_*`
//! env var with the pre-rename `GURU_*` name as a fallback, so an operator's
//! existing `GURU_*` deployment env keeps working. The fallback path prints a
//! deprecation warning to stderr each time it's taken (callers on a hot path
//! should cache the result rather than re-reading per call).

/// Reads `new_key`; if unset, falls back to `old_key` and prints a
/// deprecation warning to stderr naming both.
pub fn env_with_legacy(new_key: &str, old_key: &str) -> Option<String> {
    if let Ok(v) = std::env::var(new_key) {
        return Some(v);
    }
    let v = std::env::var(old_key).ok()?;
    eprintln!("warning: {old_key} is deprecated, use {new_key} instead (guru → kikimimi rename)");
    Some(v)
}

/// [`env_with_legacy`], parsed as a `u16` (port numbers). An unparsable value
/// is treated the same as unset (falls through to the next source, same as
/// the pre-rename behavior for a malformed `GURU_OTLP_PORT`/`GURU_WEB_PORT`).
pub fn env_u16_with_legacy(new_key: &str, old_key: &str) -> Option<u16> {
    env_with_legacy(new_key, old_key).and_then(|s| s.parse().ok())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serial_test::serial;

    struct EnvGuard(Vec<&'static str>);
    impl Drop for EnvGuard {
        fn drop(&mut self) {
            for k in &self.0 {
                std::env::remove_var(k);
            }
        }
    }

    #[test]
    #[serial]
    fn prefers_new_key_over_legacy() {
        std::env::set_var("KM_TEST_NEW", "new");
        std::env::set_var("KM_TEST_OLD", "old");
        let _g = EnvGuard(vec!["KM_TEST_NEW", "KM_TEST_OLD"]);
        assert_eq!(
            env_with_legacy("KM_TEST_NEW", "KM_TEST_OLD").as_deref(),
            Some("new")
        );
    }

    #[test]
    #[serial]
    fn falls_back_to_legacy_key() {
        std::env::remove_var("KM_TEST_NEW");
        std::env::set_var("KM_TEST_OLD", "old");
        let _g = EnvGuard(vec!["KM_TEST_NEW", "KM_TEST_OLD"]);
        assert_eq!(
            env_with_legacy("KM_TEST_NEW", "KM_TEST_OLD").as_deref(),
            Some("old")
        );
    }

    #[test]
    #[serial]
    fn none_when_neither_set() {
        std::env::remove_var("KM_TEST_NEW");
        std::env::remove_var("KM_TEST_OLD");
        let _g = EnvGuard(vec!["KM_TEST_NEW", "KM_TEST_OLD"]);
        assert_eq!(env_with_legacy("KM_TEST_NEW", "KM_TEST_OLD"), None);
    }

    #[test]
    #[serial]
    fn u16_variant_parses_and_ignores_garbage() {
        std::env::remove_var("KM_TEST_NEW");
        std::env::set_var("KM_TEST_OLD", "not-a-number");
        let _g = EnvGuard(vec!["KM_TEST_NEW", "KM_TEST_OLD"]);
        assert_eq!(env_u16_with_legacy("KM_TEST_NEW", "KM_TEST_OLD"), None);

        std::env::set_var("KM_TEST_OLD", "9999");
        assert_eq!(
            env_u16_with_legacy("KM_TEST_NEW", "KM_TEST_OLD"),
            Some(9999)
        );
    }
}
