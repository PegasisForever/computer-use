//! Startup bind guard: refuse to expose the server on non-loopback
//! addresses without authentication and TLS.
//!
//! This is the mirror of the Windows-MCP `__main__.py` startup guard.

/// Validate that binding to `host` is allowed given the auth/TLS posture.
///
/// | bind | auth key | tls cert | insecure flag | result |
/// |---|---|---|---|---|
/// | loopback | any | any | any | `Ok` |
/// | non-loopback | empty | any | any | `Err` |
/// | non-loopback | set | absent | `false` | `Err` |
/// | non-loopback | set | set or `true` | any | `Ok` |
///
/// Loopback = `localhost`, `127.0.0.1`, `::1`, `[::1]`. Anything else —
/// including hostnames that resolve to loopback — is treated as non-loopback
/// in v1.
pub fn validate_bind(
    auth_key: &str,
    host: &str,
    tls_cert: Option<&str>,
    insecure_remote: bool,
) -> Result<(), String> {
    if is_loopback(host) {
        return Ok(());
    }
    if auth_key.is_empty() {
        return Err(format!(
            "refusing to bind non-loopback address {host} without --auth-key (or COMPUTER_USE_AUTH_KEY)"
        ));
    }
    if tls_cert.is_none() && !insecure_remote {
        return Err(format!(
            "refusing to bind non-loopback address {host} without TLS (--tls-cert/--tls-key) or --allow-insecure-remote"
        ));
    }
    Ok(())
}

/// Whether `host` is a loopback address. A hostname that resolves to loopback
/// is intentionally not handled in v1.
fn is_loopback(host: &str) -> bool {
    matches!(host, "localhost" | "127.0.0.1" | "::1" | "[::1]")
}

#[cfg(test)]
mod tests {
    use super::*;

    const NON_LOOPBACK: &str = "0.0.0.0";

    #[test]
    fn loopback_without_auth_key_ok() {
        for host in ["localhost", "127.0.0.1", "::1", "[::1]"] {
            assert!(
                validate_bind("", host, None, false).is_ok(),
                "loopback host {host:?} must always be allowed"
            );
        }
    }

    #[test]
    fn loopback_with_auth_key_ok() {
        assert!(validate_bind("secret", "127.0.0.1", None, false).is_ok());
    }

    #[test]
    fn non_loopback_without_auth_key_err() {
        let err = validate_bind("", NON_LOOPBACK, None, false).unwrap_err();
        assert!(err.contains("non-loopback address 0.0.0.0"), "err: {err}");
        assert!(err.contains("--auth-key"), "err: {err}");
        assert!(err.contains("COMPUTER_USE_AUTH_KEY"), "err: {err}");
    }

    #[test]
    fn non_loopback_key_no_tls_no_flag_err() {
        let err = validate_bind("secret", NON_LOOPBACK, None, false).unwrap_err();
        assert!(err.contains("non-loopback address 0.0.0.0"), "err: {err}");
        assert!(err.contains("--tls-cert"), "err: {err}");
        assert!(err.contains("--allow-insecure-remote"), "err: {err}");
    }

    #[test]
    fn non_loopback_key_with_tls_ok() {
        assert!(validate_bind("secret", NON_LOOPBACK, Some("/certs/cert.pem"), false).is_ok());
    }

    #[test]
    fn non_loopback_key_with_insecure_flag_ok() {
        assert!(validate_bind("secret", NON_LOOPBACK, None, true).is_ok());
    }

    #[test]
    fn non_loopback_key_tls_and_insecure_flag_ok() {
        assert!(validate_bind("secret", NON_LOOPBACK, Some("/certs/cert.pem"), true).is_ok());
    }

    #[test]
    fn other_addresses_are_non_loopback() {
        for host in ["0.0.0.0", "::", "192.168.1.5", "10.0.0.1", "example.com"] {
            assert!(
                validate_bind("", host, None, false).is_err(),
                "host {host:?} must be treated as non-loopback"
            );
        }
    }
}
