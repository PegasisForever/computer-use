//! Streamable HTTP transport: mounts rmcp's `StreamableHttpService` behind an
//! axum router and serves it over a bound `TcpListener`.
//!
//! # LOCKED middleware chain (outer → inner)
//!
//! The full request pipeline, outermost wrapper first. Axum applies `.layer()`
//! calls so each subsequent call wraps what came before it; `build_router`
//! therefore applies the layers in reverse order (CORS first, IP allowlist
//! last) so the assembled chain is:
//!
//! 1. **IP allowlist** — ours (T2c), built on `config.ip_allowlist`. Applied
//!    last so it ends up outermost: a peer outside the CIDR list is rejected
//!    with `403` before any token or CORS processing, even with a valid
//!    Bearer token.
//! 2. **Bearer auth** — ours (T2c), OPTIONS-exempt, built on
//!    `config.auth_key`. Sits directly inside the IP allowlist: preflights can
//!    reach CORS unauthenticated while every real request needs a token.
//! 3. **CorsLayer** — tower-http, off by default. Applied in `build_router`
//!    only when `config.cors_origins` is non-empty.
//! 4. **StreamableHttpService** — rmcp 1.8. Enforces `Host` (403) and
//!    `Origin` (403) via `allowed_hosts` / `allowed_origins`.
//!
//! # Session model (LOCKED)
//!
//! Stateful mode with a per-session service factory: every MCP session gets a
//! fresh `ComputerUseServer::new()` so recording state is isolated per session
//! and never shared. Never switch to stateless mode.

use crate::auth::{AllowlistState, BearerState};
use crate::config::Config;
use crate::server::ComputerUseServer;
use anyhow::{Context, Result};
use rmcp::transport::streamable_http_server::{
    StreamableHttpServerConfig, StreamableHttpService, session::local::LocalSessionManager,
};
use std::net::SocketAddr;
use std::sync::Arc;
use tokio::net::TcpListener;

/// Loopback authorities the rmcp service accepts on inbound `Host` headers
/// (DNS-rebinding defense). Mirrors rmcp's own default; kept explicit so the
/// security posture survives upstream default changes.
const LOOPBACK_HOSTS: [&str; 3] = ["localhost", "127.0.0.1", "::1"];

/// Build the fully assembled axum router for the HTTP transport.
///
/// The rmcp service is mounted at `/mcp`. The tower-http `CorsLayer` is
/// applied only when `config.cors_origins` is non-empty (off by default).
pub fn build_router(config: &Config) -> axum::Router {
    let service = build_mcp_service(config);
    let mut router = axum::Router::new().nest_service("/mcp", service);

    // LOCKED middleware chain, outer → inner:
    //   [IP allowlist] → [Bearer, OPTIONS-exempt] → [CorsLayer, off by
    //   default] → [rmcp StreamableHttpService]
    //
    // axum applies `.layer()` so each subsequent call WRAPS the previous
    // router — the LAST `.layer()` call ends up the OUTERMOST middleware.
    // The layers must therefore be applied in reverse: CORS first (innermost
    // of ours), then Bearer, then the IP allowlist last (outermost). A peer
    // outside the allowlist is rejected before any token check, and an
    // unauthenticated request never reaches the CORS layer.
    if !config.cors_origins.is_empty() {
        router = router.layer(crate::auth::cors_layer(&config.cors_origins));
    }
    router = router.layer(crate::auth::bearer_middleware(Arc::new(BearerState::new(
        config.auth_key.as_deref(),
    ))));
    router = router.layer(crate::auth::ip_allowlist_middleware(Arc::new(
        AllowlistState::new(&config.ip_allowlist),
    )));
    router
}

/// Build the rmcp Streamable HTTP service (stateful, per-session factory).
///
/// The service factory is invoked once per MCP session, giving each session a
/// fresh `ComputerUseServer` with its own recording handle.
pub fn build_mcp_service(
    config: &Config,
) -> StreamableHttpService<ComputerUseServer, LocalSessionManager> {
    let session_manager = Arc::new(LocalSessionManager::default());
    let rmcp_config = rmcp_config(config);
    StreamableHttpService::new(
        || Ok(ComputerUseServer::new()),
        session_manager,
        rmcp_config,
    )
}

/// rmcp-native security configuration: loopback `Host` allowlist plus an
/// `Origin` allowlist derived from `config.cors_origins`.
///
/// `with_allowed_origins` is called only when the list is non-empty: per rmcp
/// semantics an empty `allowed_origins` disables Origin validation.
fn rmcp_config(config: &Config) -> StreamableHttpServerConfig {
    let mut server_config = StreamableHttpServerConfig::default()
        .with_allowed_hosts(LOOPBACK_HOSTS)
        .with_stateful_mode(true);
    if !config.cors_origins.is_empty() {
        server_config = server_config.with_allowed_origins(config.cors_origins.clone());
    }
    server_config
}

/// Bind the listener and serve the router until shutdown.
///
/// TLS is behind the `tls` cargo feature and requires both `tls_cert` and
/// `tls_key`. Serving is delegated to `crate::tls::serve`, the single
/// dispatch point: `None` serves plain HTTP, `Some((cert, key))` serves HTTPS
/// when the `tls` feature is on and fails closed otherwise.
///
/// The bind address is resolved here: an IP or bracketed IPv6 parses directly
/// to a `SocketAddr`; a hostname cannot, so it is resolved through a
/// throwaway `TcpListener::bind` and its `local_addr` is passed on (dropping
/// a listening socket releases the port, so `crate::tls::serve` can bind the
/// resolved address itself).
pub async fn serve(config: Config) -> Result<()> {
    let bind_addr = format!("{}:{}", config.host, config.port);
    let addr = match bind_addr.parse::<SocketAddr>() {
        Ok(addr) => addr,
        Err(_) => {
            let listener = TcpListener::bind(&bind_addr)
                .await
                .with_context(|| format!("failed to bind {bind_addr}"))?;
            listener
                .local_addr()
                .context("failed to read listener local address")?
        }
    };
    tracing::info!(bind_addr, "http transport listening");

    let router = build_router(&config);
    let tls_opt = match (config.tls_cert, config.tls_key) {
        (Some(cert), Some(key)) => Some((cert, key)),
        _ => None,
    };
    crate::tls::serve(addr, router, tls_opt).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{CliValues, EnvMap};
    use axum::body::Body;
    use axum::extract::ConnectInfo;
    use axum::http::{Request, StatusCode};
    use tower::ServiceExt;

    fn config_with(cors_origins: &[&str]) -> Config {
        let mut values = CliValues::default();
        if !cors_origins.is_empty() {
            values.cors_origins = Some(cors_origins.join(","));
        }
        Config::from_sources(&values, &EnvMap::new(), None).expect("test config is valid")
    }

    fn get(path: &str) -> Request<Body> {
        Request::builder()
            .method("GET")
            .uri(path)
            .header("Host", "localhost")
            .body(Body::empty())
            .expect("valid test request")
    }

    #[tokio::test]
    async fn router_builds_and_unrouted_path_returns_404() {
        let router = build_router(&config_with(&[]));
        let response = router
            .clone()
            .oneshot(get("/does-not-exist"))
            .await
            .expect("router service is infallible");
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn mcp_route_is_mounted() {
        // A session-less GET on /mcp reaches rmcp, which answers 400 (session
        // id required in stateful mode) — proving the route is mounted rather
        // than falling through to axum's 404 fallback.
        let router = build_router(&config_with(&[]));
        let response = router
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri("/mcp")
                    .header("Host", "localhost")
                    .header("Accept", "text/event-stream")
                    .body(Body::empty())
                    .expect("valid test request"),
            )
            .await
            .expect("router service is infallible");
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn bearer_rejects_bad_token_before_cors_headers() {
        // Locked chain: [IP] → [Bearer] → [CORS] → [rmcp]. A bad token with
        // an ALLOWED origin must yield 401 from the Bearer layer WITHOUT any
        // CORS headers — proving Bearer sits outside CORS (an allowed origin
        // would otherwise have been echoed onto the 401 response).
        let values = CliValues {
            auth_key: Some("secret-token".into()),
            cors_origins: Some("https://app.example.com".into()),
            ..Default::default()
        };
        let config = Config::from_sources(&values, &EnvMap::new(), None).expect("valid config");
        let router = build_router(&config);
        let response = router
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/mcp")
                    .header("Host", "localhost")
                    .header("Origin", "https://app.example.com")
                    .header("Authorization", "Bearer wrong-token")
                    .body(Body::empty())
                    .expect("valid request"),
            )
            .await
            .expect("router is infallible");
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
        assert!(
            response
                .headers()
                .get("access-control-allow-origin")
                .is_none(),
            "the 401 must bypass the CORS layer entirely"
        );
    }

    #[tokio::test]
    async fn ip_allowlist_rejects_unlisted_peer_even_with_valid_token() {
        // Locked chain: [IP] → [Bearer] → [CORS] → [rmcp]. The IP allowlist
        // is the OUTERMOST layer: a peer outside the CIDR list gets 403 even
        // with a valid Bearer token, before any other layer runs.
        let values = CliValues {
            auth_key: Some("secret-token".into()),
            ip_allowlist: Some("10.0.0.0/8".into()),
            ..Default::default()
        };
        let config = Config::from_sources(&values, &EnvMap::new(), None).expect("valid config");
        let router = build_router(&config);
        let mut request = Request::builder()
            .method("POST")
            .uri("/mcp")
            .header("Host", "localhost")
            .header("Authorization", "Bearer secret-token")
            .body(Body::empty())
            .expect("valid request");
        let peer = SocketAddr::from(([192, 168, 1, 50], 1234));
        request.extensions_mut().insert(ConnectInfo(peer));
        let response = router.oneshot(request).await.expect("router is infallible");
        assert_eq!(response.status(), StatusCode::FORBIDDEN);
    }

    #[test]
    fn cors_origins_wired_into_rmcp_allowed_origins_when_present() {
        let config = config_with(&["https://app.example.com", "http://localhost:8080"]);
        let service = build_mcp_service(&config);
        assert_eq!(service.config.allowed_origins, config.cors_origins);
    }

    #[test]
    fn empty_cors_origins_disable_rmcp_origin_validation() {
        let service = build_mcp_service(&config_with(&[]));
        assert!(
            service.config.allowed_origins.is_empty(),
            "empty origin list must disable Origin validation (rmcp semantics)"
        );
    }

    #[test]
    fn allowed_hosts_default_to_loopback() {
        let service = build_mcp_service(&config_with(&[]));
        assert_eq!(service.config.allowed_hosts, LOOPBACK_HOSTS);
    }

    #[test]
    fn stateful_mode_is_locked_on() {
        let service = build_mcp_service(&config_with(&[]));
        assert!(
            service.config.stateful_mode,
            "stateful sessions with a per-session server factory are locked"
        );
    }
}
