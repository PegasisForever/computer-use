//! Optional TLS support for the Streamable HTTP transport.
//!
//! Gated behind the `tls` cargo feature (default off). When the feature is
//! disabled, certificate configuration fails closed at startup and the binary
//! contains no rustls symbols.

use std::net::SocketAddr;
use std::path::{Path, PathBuf};

use anyhow::Result;
use axum::Router;
use tokio::net::TcpListener;

#[cfg(feature = "tls")]
use anyhow::Context;
#[cfg(feature = "tls")]
use axum_server::tls_rustls::RustlsConfig;

/// Serve `router` at `addr`, optionally wrapped in TLS.
///
/// This is the one call site the HTTP transport needs:
/// `crate::tls::serve(addr, router, tls_opt)`.
///
/// - `None`: plain HTTP via `axum::serve`.
/// - `Some((cert, key))` with the `tls` feature: HTTPS via axum-server rustls.
/// - `Some((cert, key))` without the `tls` feature: fails closed (Err).
pub async fn serve(
    addr: SocketAddr,
    router: Router,
    tls: Option<(PathBuf, PathBuf)>,
) -> Result<()> {
    match tls {
        Some((cert, key)) => serve_tls(addr, router, cert, key).await,
        None => serve_plain(addr, router).await,
    }
}

/// Serve plain HTTP with `axum::serve`. Shared by both feature states.
async fn serve_plain(addr: SocketAddr, router: Router) -> Result<()> {
    axum::serve(TcpListener::bind(addr).await?, router)
        .await
        .map_err(|e| anyhow::anyhow!("HTTP server error: {e}"))
}

/// Serve HTTPS with axum-server's rustls acceptor.
///
/// ALPN is pinned to `http/1.1` because this crate builds axum without the
/// `http2` feature; axum-server's `RustlsConfig::from_config` does not set
/// ALPN itself.
#[cfg(feature = "tls")]
async fn serve_tls(addr: SocketAddr, router: Router, cert: PathBuf, key: PathBuf) -> Result<()> {
    let config = load_tls_config(&cert, &key)?;
    axum_server::bind_rustls(addr, RustlsConfig::from_config(std::sync::Arc::new(config)))
        .serve(router.into_make_service())
        .await
        .map_err(|e| anyhow::anyhow!("TLS server error: {e}"))
}

/// HTTPS serving when the `tls` feature is disabled: fail closed rather than
/// silently serving plaintext where TLS was requested.
#[cfg(not(feature = "tls"))]
async fn serve_tls(
    _addr: SocketAddr,
    _router: Router,
    _cert: PathBuf,
    _key: PathBuf,
) -> Result<()> {
    anyhow::bail!("TLS support not compiled in; rebuild with --features tls")
}

/// Validate that TLS certificate/key configuration is complete and parseable.
///
/// Works without the `tls` feature (fails closed when certs are configured but
/// the feature is off). With the feature on, the PEM files are actually parsed
/// so garbage is rejected at startup.
///
/// # NOTE
/// `#[allow(dead_code)]`: the binary never calls this directly — `tls::serve`
/// parses the pair itself via `load_tls_config` when TLS is configured — so
/// only the tests exercise it. Keep the attribute.
#[allow(dead_code)]
pub fn validate_certs(cert: Option<&Path>, key: Option<&Path>) -> Result<()> {
    match (cert, key) {
        (None, None) => Ok(()),
        (Some(_), None) => {
            anyhow::bail!("TLS certificate set but no private key (--tls-key)")
        }
        (None, Some(_)) => {
            anyhow::bail!("TLS private key set but no certificate (--tls-cert)")
        }
        (Some(cert), Some(key)) => validate_pair(cert, key),
    }
}

/// Parse the cert/key pair. Only compiled when the `tls` feature is on.
#[cfg(feature = "tls")]
fn validate_pair(cert: &Path, key: &Path) -> Result<()> {
    load_tls_config(cert, key).map(|_| ())
}

/// Fail closed when certs are configured but TLS support is not compiled in.
#[cfg(not(feature = "tls"))]
fn validate_pair(_cert: &Path, _key: &Path) -> Result<()> {
    anyhow::bail!("TLS support not compiled in; rebuild with --features tls")
}

/// Load a `rustls::ServerConfig` from PEM certificate and key files.
///
/// Rejects unreadable or unparseable PEM, empty cert files, and key files that
/// contain no private key. Returns `Err` (never panics) for all of these.
#[cfg(feature = "tls")]
pub fn load_tls_config(cert: &Path, key: &Path) -> Result<rustls::ServerConfig> {
    // rustls 0.23 requires a process-default CryptoProvider. axum-server's
    // `tls-rustls` feature compiles rustls with the `aws-lc-rs` provider
    // (not `ring`), so install that one. `install_default` errors only when a
    // provider is already installed — that is fine to ignore.
    let _ = rustls::crypto::aws_lc_rs::default_provider().install_default();

    let cert_pem = std::fs::read(cert)
        .with_context(|| format!("failed to read TLS certificate {}", cert.display()))?;
    let key_pem = std::fs::read(key)
        .with_context(|| format!("failed to read TLS private key {}", key.display()))?;

    let certs = rustls_pemfile::certs(&mut cert_pem.as_slice())
        .collect::<std::io::Result<Vec<_>>>()
        .with_context(|| format!("invalid PEM in TLS certificate {}", cert.display()))?;
    if certs.is_empty() {
        anyhow::bail!(
            "no certificates found in TLS certificate file {}",
            cert.display()
        );
    }

    let key = rustls_pemfile::private_key(&mut key_pem.as_slice())
        .with_context(|| format!("invalid PEM in TLS private key {}", key.display()))?
        .ok_or_else(|| anyhow::anyhow!("no private key found in TLS key file {}", key.display()))?;

    let mut config = rustls::ServerConfig::builder()
        .with_no_client_auth()
        .with_single_cert(certs, key)
        .map_err(|e| anyhow::anyhow!("invalid TLS certificate/key pair: {e}"))?;

    config.alpn_protocols = vec![b"http/1.1".to_vec()];
    Ok(config)
}

#[cfg(all(test, feature = "tls"))]
mod tests {
    use super::*;
    use std::fs;
    use std::sync::Arc;

    // Self-signed RSA-2048 certificate for CN=localhost, generated with
    // `openssl req -x509 -newkey rsa:2048 -nodes -days 3650`.
    const CERT_PEM: &str = "-----BEGIN CERTIFICATE-----\n\
MIIDHzCCAgegAwIBAgIUNMqJ8F7YmIdpGDYyUjYQkENymJ0wDQYJKoZIhvcNAQEL\n\
BQAwFDESMBAGA1UEAwwJbG9jYWxob3N0MB4XDTI2MDgxNTA0NTYwMFoXDTM2MDgx\n\
MjA0NTYwMFowFDESMBAGA1UEAwwJbG9jYWxob3N0MIIBIjANBgkqhkiG9w0BAQEF\n\
AAOCAQ8AMIIBCgKCAQEAqx8ubwar3R0WB+fuAQzNENG6ojPyN8wgKUUAcDMrBY8i\n\
ZcPIbbU+NJ2JlkNYXYf9FTMFjISE3anuysYMAS2AK+9wFffdYpHLOcCXJELe/i/1\n\
m7WRFiQGdPsOWrRvPzTg2VlBlhxzFis3L3xl2Sx8rPiK63bcUVzfdhm3woV/repZ\n\
KZzhTdjbSiK4oIYUV1tnZSBHtVTjhBldaNHu7XU8T2HyjxxQkjhZP1jSAUZrGx40\n\
mUSaRFqBndjHIMip+izSc1u+xXQ8TYMTyBIgfJ2yzIYdmx9SSmFZXIMmIhX268fm\n\
cowTPUDnoAnAy7u9tLvvDXnOsSwnJ3fEDISNYMWFNwIDAQABo2kwZzAdBgNVHQ4E\n\
FgQUPsp6aPFL8hiSXhboMwWLzamJe/QwHwYDVR0jBBgwFoAUPsp6aPFL8hiSXhbo\n\
MwWLzamJe/QwDwYDVR0TAQH/BAUwAwEB/zAUBgNVHREEDTALgglsb2NhbGhvc3Qw\n\
DQYJKoZIhvcNAQELBQADggEBABV1oRCwPvHIsJpbfqLBB7kzC/GdOyZKskKravhZ\n\
l7AD9IrooeEogPz5MJj/NHPL7hx6i+4J4D/hGCVlWJM1kvPcbH6c8ER8b40JP523\n\
GZwpnXTN9Y+v4Zr7ike3TSyTDeeApKjxKP5zeJZhyAhgyIOLE5xBweeOMxEfDmS8\n\
ees7fQUXZiaWjJ7/vLdV+odHMEwAp8zBuIgN9S7kMk+/Pv/qkfo8VtaDDuPrShhs\n\
6F+gg2FFR3HuNwgm7xBLC1d8azZ/KmaCOdNTY/7OgkOZYhbuBalN+AojGY86TMc5\n\
qojIzGJe6eJq0q1RyEH4GSEpUlCrwVW5b8YFE9Kc8zOpxuo=\n\
-----END CERTIFICATE-----\n";

    // PKCS#8 RSA private key matching CERT_PEM.
    const KEY_PEM: &str = "-----BEGIN PRIVATE KEY-----\n\
MIIEvQIBADANBgkqhkiG9w0BAQEFAASCBKcwggSjAgEAAoIBAQCrHy5vBqvdHRYH\n\
5+4BDM0Q0bqiM/I3zCApRQBwMysFjyJlw8httT40nYmWQ1hdh/0VMwWMhITdqe7K\n\
xgwBLYAr73AV991ikcs5wJckQt7+L/WbtZEWJAZ0+w5atG8/NODZWUGWHHMWKzcv\n\
fGXZLHys+IrrdtxRXN92GbfChX+t6lkpnOFN2NtKIrighhRXW2dlIEe1VOOEGV1o\n\
0e7tdTxPYfKPHFCSOFk/WNIBRmsbHjSZRJpEWoGd2McgyKn6LNJzW77FdDxNgxPI\n\
EiB8nbLMhh2bH1JKYVlcgyYiFfbrx+ZyjBM9QOegCcDLu720u+8Nec6xLCcnd8QM\n\
hI1gxYU3AgMBAAECggEANiZTWMHZL8qs9op/h6iH2cssTni2rM503OpfZaHjespc\n\
/avzKDhCu36gk8Ky20IVpZg0KM+khcpo7JS4JsVBumV16BW74h21nAxkJ47bpr8B\n\
bykJBOMYgfsSA0T2sR8oCc9TUE+nYXwCZg3s4sjPmSK7PyCpPjvv4Jzpx+3bxeoN\n\
c0G86ybE19ut8rlwfhf8nBkTJY9vlc1GlmwLCCSjd1iulZ/R2n6GKdmvGpe0IIHX\n\
kTmbz3x7resRK2pK+Am7WOZr6rtC+yNxJhLlv1zxf1hgBHMWml3CBYutjnK5i8tG\n\
eR6ViAdpODFnwvNk/Dn911Mj+nkMO7KII4aZf2nYiQKBgQDvCxfJUGcD3pxyvbID\n\
bdgcdyNKZltRDADApfxyGTCUcZM24Q0Pt6DPh9L+wjMUC9Q3jWSvpYhPd6C3iFQV\n\
iCzXJJ4cK1CDEFhb4sqZA8lxMoBZzBRI1V2aqozA9CfFnWFNWb5ys1QIOHnvfm5R\n\
4+KCeBxO1jlYtFuA8foEg9ZHjwKBgQC3Qqrjs45kxye4CLIRRqGrUwqrhThtTdM+\n\
2zsJpKq46fiOXFFJ8ppYJOgRJirXOH6K/czU+UNqLkhqd66ZV4Tnw92EK/pP+ApU\n\
bp7MdEvkRqhYglEFjPuLJ4hCVIiOiRfrRgHHifHLgSGYzFc6ukc7J0YXUsEy4pf7\n\
2jAc9ljT2QKBgCaJ/U3Bnroq+8Ir/zU6UmtUceYe1n4cl0p3+FlU4lYscXWZlwd/\n\
rXeICGZ2XNHQjmhebWs8HtvhB5JPcm7+Q2x0ROF5wkM/MV+vEsbUq3eKktLnaiXL\n\
8IltEcBSHM9pbjSQUXogm12v7UjFc3fUa8JpJvc25ov8l/wUByEPOE7VAoGAO7y0\n\
neLi8MDmlpvnB7ChdIpuOPkFKsQqrxuhhAQ0TnCeQDGhodw+KatmJmjtoBhfT4lp\n\
4qaaLhuGKYZ32K5puW7kX3hCcRzmqR0iEH18W54YtDlOleOd/39UcGLD3MqdjGjW\n\
nae+tNqUDA3YBWHBZGvp8iJcreylJEi8VESNMgkCgYEA2YGUV+fCarlTbpID+9Ny\n\
3ROTSHeOcLtvV+n5UgPw2jAob494FkaD/3AwpvOhQmV4O8tBrh7nuC5T3BZagTsG\n\
RdXmmIdQJdWWJjMsW56Cq5B2H7cGFLO5MFC1+zR+uFI/IpwanDLVytX1XEHISP3H\n\
hfN6lLm7WyuzJjqlLSU5kqs=\n\
-----END PRIVATE KEY-----\n";

    /// Write `contents` to a unique temp file for this test and return its path.
    fn temp_pem(name: &str, contents: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "cu-tls-{}-{}",
            std::process::id(),
            name.replace(['/', '\\'], "_")
        ));
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("pem");
        fs::write(&path, contents).unwrap();
        path
    }

    #[test]
    fn load_tls_config_accepts_self_signed_pem() {
        let cert = temp_pem("load-ok-cert", CERT_PEM);
        let key = temp_pem("load-ok-key", KEY_PEM);
        let config = load_tls_config(&cert, &key).expect("valid self-signed PEM must load");
        // Sanity: a ServerConfig that can be handed to axum-server.
        let _ = Arc::new(config);
    }

    #[test]
    fn load_tls_config_rejects_garbage_cert() {
        let cert = temp_pem("garbage-cert", "not a pem file\njust text\n");
        let key = temp_pem("garbage-cert-key", KEY_PEM);
        let err = load_tls_config(&cert, &key).expect_err("garbage cert must be rejected");
        assert!(!err.to_string().is_empty());
    }

    #[test]
    fn load_tls_config_rejects_garbage_key() {
        let cert = temp_pem("garbage-key-cert", CERT_PEM);
        let key = temp_pem("garbage-key", "definitely not a key\n");
        let err = load_tls_config(&cert, &key).expect_err("garbage key must be rejected");
        assert!(!err.to_string().is_empty());
    }

    #[test]
    fn load_tls_config_rejects_cert_without_key() {
        let cert = temp_pem("nokey-cert", CERT_PEM);
        let key = temp_pem(
            "nokey-key",
            "-----BEGIN CERTIFICATE-----\nAAAA\n-----END CERTIFICATE-----\n",
        );
        let err = load_tls_config(&cert, &key).expect_err("cert with no private key must fail");
        assert!(!err.to_string().is_empty());
    }

    #[test]
    fn load_tls_config_rejects_missing_file() {
        let err = load_tls_config(
            Path::new("/nonexistent/cert.pem"),
            Path::new("/nonexistent/key.pem"),
        )
        .expect_err("missing file must fail");
        assert!(!err.to_string().is_empty());
    }

    #[test]
    fn validate_certs_requires_both_or_neither() {
        assert!(validate_certs(None, None).is_ok());
        let cert = temp_pem("validate-cert-only", CERT_PEM);
        let key = temp_pem("validate-key-only", KEY_PEM);
        let err = validate_certs(Some(&cert), None).expect_err("cert without key must fail");
        assert!(err.to_string().contains("--tls-key"), "err: {err}");
        let err = validate_certs(None, Some(&key)).expect_err("key without cert must fail");
        assert!(err.to_string().contains("--tls-cert"), "err: {err}");
    }

    #[test]
    fn validate_certs_accepts_valid_pair() {
        let cert = temp_pem("validate-pair-cert", CERT_PEM);
        let key = temp_pem("validate-pair-key", KEY_PEM);
        validate_certs(Some(&cert), Some(&key)).expect("valid pair must validate");
    }

    #[test]
    fn validate_certs_rejects_garbage() {
        let cert = temp_pem("validate-garbage-cert", "garbage\n");
        let key = temp_pem("validate-garbage-key", "garbage\n");
        let err = validate_certs(Some(&cert), Some(&key)).expect_err("garbage pair must fail");
        assert!(!err.to_string().is_empty());
    }
}

#[cfg(all(test, not(feature = "tls")))]
mod tests_no_tls {
    use super::*;
    use std::fs;

    #[test]
    fn validate_certs_fails_closed_without_tls_feature() {
        let dir = std::env::temp_dir().join(format!("cu-tls-nofeature-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let cert = dir.join("cert.pem");
        let key = dir.join("key.pem");
        fs::write(&cert, "x").unwrap();
        fs::write(&key, "y").unwrap();
        let err =
            validate_certs(Some(&cert), Some(&key)).expect_err("must fail without tls feature");
        assert!(
            err.to_string().contains("TLS support not compiled in"),
            "err: {err}"
        );
    }

    #[test]
    fn validate_certs_no_certs_ok_without_tls_feature() {
        assert!(validate_certs(None, None).is_ok());
    }
}
