//! Authentication middleware stack for the Streamable HTTP transport (T2c).
//!
//! Three composable tower layers, assembled by the HTTP integration unit
//! (T2i) in this exact chain order:
//!
//! ```text
//! [ip_allowlist] -> [bearer] -> [cors_layer] -> [StreamableHttpService]
//! ```
//!
//! Request flow: the IP allowlist runs first (peer socket address only),
//! then Bearer auth (401 with `WWW-Authenticate: Bearer` on failure; OPTIONS
//! preflights are exempt so browser CORS negotiation can proceed), then CORS
//! (answers preflights itself, adds `Access-Control-*` headers on real
//! responses), then the rmcp StreamableHttpService.
//!
//! Security invariants:
//! - The token is read from the `Authorization: Bearer <token>` header only.
//!   A token in the URL is never accepted (MCP spec MUST NOT).
//! - The expected token is SHA-256 digested once at layer construction; the
//!   raw secret never sits in a per-request buffer. Both sides are hashed and
//!   compared with `subtle::ConstantTimeEq` — never with `==` on strings.
//! - Tokens and `MCP-Session-Id` are never logged.
//! - The IP allowlist matches the socket peer address only. Forwarded headers
//!   (`X-Forwarded-For`, ...) are never trusted (spoofing).
//! - IPv4-mapped IPv6 peers (`::ffff:a.b.c.d`) are normalized to `a.b.c.d`
//!   before the CIDR match. An empty list allows all peers (opt-in).
//! - CORS is off by default: an empty origin list builds a `CorsLayer` that
//!   emits no `Access-Control-Allow-Origin` header, so browsers block
//!   cross-origin access. When configured, allowed headers are
//!   `Authorization`, `MCP-Session-Id`, `MCP-Protocol-Version`,
//!   `Last-Event-ID`, `Content-Type` and allowed methods are `POST`/`OPTIONS`.

use axum::{
    extract::ConnectInfo,
    http::{HeaderMap, HeaderName, HeaderValue, Method, Request, Response, StatusCode, header},
};
use ipnet::IpNet;
use std::{
    convert::Infallible,
    future::Future,
    net::{IpAddr, SocketAddr},
    pin::Pin,
    str::FromStr,
    sync::Arc,
    task::{Context, Poll},
};
use subtle::ConstantTimeEq;
use tower::{Layer, Service};
use tower_http::cors::CorsLayer;

/// Request headers MCP clients send across origins (locked list).
const MCP_REQUEST_HEADERS: [HeaderName; 5] = [
    header::AUTHORIZATION,
    header::CONTENT_TYPE,
    HeaderName::from_static("last-event-id"),
    HeaderName::from_static("mcp-session-id"),
    HeaderName::from_static("mcp-protocol-version"),
];

/// State shared by the Bearer middleware.
///
/// Holds only the SHA-256 digest of the expected token; the raw secret is
/// dropped at construction. `enabled` is false when no auth key was
/// configured, turning the middleware into a no-op pass-through (loopback
/// default needs no token — locked decision).
#[derive(Debug)]
pub struct BearerState {
    enabled: bool,
    expected_digest: [u8; 32],
}

impl BearerState {
    /// Digest `auth_key` once. `None` or empty → auth disabled (no-op layer).
    pub fn new(auth_key: Option<&str>) -> Self {
        match auth_key {
            Some(key) if !key.is_empty() => Self {
                enabled: true,
                expected_digest: sha256(key.as_bytes()),
            },
            _ => Self {
                enabled: false,
                expected_digest: [0u8; 32],
            },
        }
    }
}

/// State shared by the IP allowlist middleware.
#[derive(Debug)]
pub struct AllowlistState {
    networks: Vec<IpNet>,
}

impl AllowlistState {
    /// Parse the configured CIDR list. An empty list allows every peer.
    ///
    /// A malformed CIDR panics: an allowlist typo must fail closed at startup,
    /// never silently widen access.
    pub fn new(entries: &[String]) -> Self {
        let networks = entries
            .iter()
            .map(|entry| {
                IpNet::from_str(entry)
                    .unwrap_or_else(|_| panic!("invalid CIDR in ip_allowlist: {entry:?}"))
            })
            .collect();
        Self { networks }
    }
}

/// Layer producing [`Bearer`] services that enforce Bearer authentication.
#[derive(Clone, Debug)]
pub struct BearerLayer {
    state: Arc<BearerState>,
}

/// Build the Bearer middleware layer around `state`. `None`/empty key makes
/// it a pass-through (no-op).
pub fn bearer_middleware(state: Arc<BearerState>) -> BearerLayer {
    BearerLayer { state }
}

/// Layer producing [`IpAllowlist`] services that enforce the peer allowlist.
#[derive(Clone, Debug)]
pub struct IpAllowlistLayer {
    state: Arc<AllowlistState>,
}

/// Build the IP allowlist middleware layer around `state`.
pub fn ip_allowlist_middleware(state: Arc<AllowlistState>) -> IpAllowlistLayer {
    IpAllowlistLayer { state }
}

/// Build the CORS layer from the configured allowed origins.
///
/// Empty list → a bare `CorsLayer::new()` that emits no
/// `Access-Control-Allow-Origin` header (browsers block). Otherwise the
/// origins are allowed with methods `POST`/`OPTIONS` and the MCP header set.
pub fn cors_layer(cors_origins: &[String]) -> CorsLayer {
    if cors_origins.is_empty() {
        return CorsLayer::new();
    }
    let origins = cors_origins
        .iter()
        .map(|origin| {
            HeaderValue::from_str(origin)
                .unwrap_or_else(|_| panic!("invalid origin in cors_origins: {origin:?}"))
        })
        .collect::<Vec<_>>();
    CorsLayer::new()
        .allow_origin(origins)
        .allow_methods([Method::POST, Method::OPTIONS])
        .allow_headers(MCP_REQUEST_HEADERS)
}

/// SHA-256 (FIPS 180-4). Implemented locally because the locked dependency
/// tree has no hash crate; only a fixed-size digest is needed to equalize the
/// constant-time comparison. Verified against FIPS test vectors.
fn sha256(input: &[u8]) -> [u8; 32] {
    /// Round constants (first 32 bits of the fractional parts of the cube
    /// roots of the first 64 primes).
    const K: [u32; 64] = [
        0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4,
        0xab1c5ed5, 0xd807aa98, 0x12835b01, 0x243185be, 0x550c7dc3, 0x72be5d74, 0x80deb1fe,
        0x9bdc06a7, 0xc19bf174, 0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc, 0x2de92c6f,
        0x4a7484aa, 0x5cb0a9dc, 0x76f988da, 0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7,
        0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967, 0x27b70a85, 0x2e1b2138, 0x4d2c6dfc,
        0x53380d13, 0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85, 0xa2bfe8a1, 0xa81a664b,
        0xc24b8b70, 0xc76c51a3, 0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070, 0x19a4c116,
        0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f, 0x682e6ff3,
        0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208, 0x90befffa, 0xa4506ceb, 0xbef9a3f7,
        0xc67178f2,
    ];
    // Initial hash values (first 32 bits of the fractional parts of the
    // square roots of the first 8 primes).
    let mut h: [u32; 8] = [
        0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a, 0x510e527f, 0x9b05688c, 0x1f83d9ab,
        0x5be0cd19,
    ];

    // Pad to a multiple of 64 bytes: 0x80, zeros, then the 64-bit big-endian
    // bit length of the original message.
    let bit_len = (input.len() as u64).wrapping_mul(8);
    let mut msg = input.to_vec();
    msg.push(0x80);
    while msg.len() % 64 != 56 {
        msg.push(0);
    }
    msg.extend_from_slice(&bit_len.to_be_bytes());

    let mut w = [0u32; 64];
    let (chunks, _) = msg.as_chunks::<64>();
    for chunk in chunks {
        for (i, word) in w[..16].iter_mut().enumerate() {
            *word = u32::from_be_bytes([
                chunk[i * 4],
                chunk[i * 4 + 1],
                chunk[i * 4 + 2],
                chunk[i * 4 + 3],
            ]);
        }
        for i in 16..64 {
            let s0 = w[i - 15].rotate_right(7) ^ w[i - 15].rotate_right(18) ^ (w[i - 15] >> 3);
            let s1 = w[i - 2].rotate_right(17) ^ w[i - 2].rotate_right(19) ^ (w[i - 2] >> 10);
            w[i] = w[i - 16]
                .wrapping_add(s0)
                .wrapping_add(w[i - 7])
                .wrapping_add(s1);
        }

        let [mut a, mut b, mut c, mut d, mut e, mut f, mut g, mut hh] = h;
        for i in 0..64 {
            let s1 = e.rotate_right(6) ^ e.rotate_right(11) ^ e.rotate_right(25);
            let ch = (e & f) ^ ((!e) & g);
            let temp1 = hh
                .wrapping_add(s1)
                .wrapping_add(ch)
                .wrapping_add(K[i])
                .wrapping_add(w[i]);
            let s0 = a.rotate_right(2) ^ a.rotate_right(13) ^ a.rotate_right(22);
            let maj = (a & b) ^ (a & c) ^ (b & c);
            let temp2 = s0.wrapping_add(maj);
            hh = g;
            g = f;
            f = e;
            e = d.wrapping_add(temp1);
            d = c;
            c = b;
            b = a;
            a = temp1.wrapping_add(temp2);
        }
        h[0] = h[0].wrapping_add(a);
        h[1] = h[1].wrapping_add(b);
        h[2] = h[2].wrapping_add(c);
        h[3] = h[3].wrapping_add(d);
        h[4] = h[4].wrapping_add(e);
        h[5] = h[5].wrapping_add(f);
        h[6] = h[6].wrapping_add(g);
        h[7] = h[7].wrapping_add(hh);
    }

    let mut out = [0u8; 32];
    for (i, &word) in h.iter().enumerate() {
        out[i * 4..i * 4 + 4].copy_from_slice(&word.to_be_bytes());
    }
    out
}

/// Constant-time digest equality: SHA-256 both sides, then `ct_eq`. The hash
/// equalizes comparison length so token length never leaks through timing.
fn digests_equal(expected: &[u8; 32], presented: &[u8]) -> bool {
    let presented_digest = sha256(presented);
    // Both sides are exactly 32 bytes, so the slice comparison always scans
    // the full digest and only ever depends on the content, not the length.
    expected
        .as_slice()
        .ct_eq(presented_digest.as_slice())
        .into()
}

/// Extract the token bytes from the `Authorization: Bearer <token>` header.
/// Only the exact `Bearer ` scheme is recognized; the token must not be
/// empty. Returns `None` when absent or malformed.
fn bearer_token_from(headers: &HeaderMap) -> Option<&[u8]> {
    let value = headers.get(header::AUTHORIZATION)?.as_bytes();
    let token = value.strip_prefix(b"Bearer ")?;
    if token.is_empty() { None } else { Some(token) }
}

/// Socket peer address from request extensions. Forwarded headers are never
/// consulted (locked: spoofing).
fn peer_ip<B>(request: &Request<B>) -> Option<IpAddr> {
    request
        .extensions()
        .get::<ConnectInfo<SocketAddr>>()
        .map(|ConnectInfo(addr)| addr.ip())
}

/// Normalize IPv4-mapped IPv6 (`::ffff:a.b.c.d`) to `a.b.c.d` so CIDR
/// matching works across socket families. Other addresses are unchanged.
fn normalize_peer(ip: IpAddr) -> IpAddr {
    match ip {
        IpAddr::V4(v4) => IpAddr::V4(v4),
        IpAddr::V6(v6) => v6.to_ipv4_mapped().map_or(IpAddr::V6(v6), IpAddr::V4),
    }
}

/// Whether `state` admits `ip`. Empty list admits everyone.
fn allowlist_allows(state: &AllowlistState, ip: IpAddr) -> bool {
    if state.networks.is_empty() {
        return true;
    }
    let ip = normalize_peer(ip);
    state.networks.iter().any(|net| net.contains(&ip))
}

/// `401 Unauthorized` with the MCP-spec challenge header.
fn unauthorized<B: Default>() -> Response<B> {
    let mut response = Response::new(B::default());
    *response.status_mut() = StatusCode::UNAUTHORIZED;
    response
        .headers_mut()
        .insert(header::WWW_AUTHENTICATE, HeaderValue::from_static("Bearer"));
    response
}

/// `403 Forbidden` (peer not allowed).
fn forbidden<B: Default>() -> Response<B> {
    let mut response = Response::new(B::default());
    *response.status_mut() = StatusCode::FORBIDDEN;
    response
}

/// Bearer-authenticated service wrapping `S`. Requests without a matching
/// `Authorization: Bearer` header get `401`; OPTIONS preflights pass through.
#[derive(Clone)]
pub struct Bearer<S> {
    inner: S,
    state: Arc<BearerState>,
}

impl<S, B> Service<Request<B>> for Bearer<S>
where
    S: Service<Request<B>, Response = Response<B>, Error = Infallible>
        + Clone
        + Send
        + Sync
        + 'static,
    S::Future: Send + 'static,
    B: Default + Send + 'static,
{
    type Response = Response<B>;
    type Error = Infallible;
    type Future = Pin<Box<dyn Future<Output = Result<Response<B>, Infallible>> + Send + 'static>>;

    fn poll_ready(&mut self, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        self.inner.poll_ready(cx)
    }

    fn call(&mut self, req: Request<B>) -> Self::Future {
        if !self.state.enabled || req.method() == Method::OPTIONS {
            let mut inner = self.inner.clone();
            return Box::pin(async move { inner.call(req).await });
        }
        match bearer_token_from(req.headers()) {
            Some(token) if digests_equal(&self.state.expected_digest, token) => {
                let mut inner = self.inner.clone();
                Box::pin(async move { inner.call(req).await })
            }
            _ => Box::pin(async { Ok(unauthorized()) }),
        }
    }
}

impl<S> Layer<S> for BearerLayer {
    type Service = Bearer<S>;

    fn layer(&self, inner: S) -> Self::Service {
        Bearer {
            inner,
            state: self.state.clone(),
        }
    }
}

/// Peer-allowlisted service wrapping `S`. Peers outside the configured CIDR
/// list get `403`; an empty list allows every peer.
#[derive(Clone)]
pub struct IpAllowlist<S> {
    inner: S,
    state: Arc<AllowlistState>,
}

impl<S, B> Service<Request<B>> for IpAllowlist<S>
where
    S: Service<Request<B>, Response = Response<B>, Error = Infallible>
        + Clone
        + Send
        + Sync
        + 'static,
    S::Future: Send + 'static,
    B: Default + Send + 'static,
{
    type Response = Response<B>;
    type Error = Infallible;
    type Future = Pin<Box<dyn Future<Output = Result<Response<B>, Infallible>> + Send + 'static>>;

    fn poll_ready(&mut self, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        self.inner.poll_ready(cx)
    }

    fn call(&mut self, req: Request<B>) -> Self::Future {
        if self.state.networks.is_empty() {
            let mut inner = self.inner.clone();
            return Box::pin(async move { inner.call(req).await });
        }
        match peer_ip(&req) {
            Some(ip) if allowlist_allows(&self.state, ip) => {
                let mut inner = self.inner.clone();
                Box::pin(async move { inner.call(req).await })
            }
            _ => Box::pin(async { Ok(forbidden()) }),
        }
    }
}

impl<S> Layer<S> for IpAllowlistLayer {
    type Service = IpAllowlist<S>;

    fn layer(&self, inner: S) -> Self::Service {
        IpAllowlist {
            inner,
            state: self.state.clone(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::Router;
    use axum::body::Body;
    use axum::routing::any;
    use std::net::{Ipv4Addr, Ipv6Addr};
    use tower::ServiceExt;

    /// A router that answers every method at `/mcp`, wrapped in one layer.
    fn bearer_app(auth_key: Option<&str>) -> axum::Router {
        Router::new()
            .route("/mcp", any(|| async { "ok" }))
            .layer(bearer_middleware(Arc::new(BearerState::new(auth_key))))
    }

    fn ip_app(allowlist: &[&str]) -> axum::Router {
        let entries: Vec<String> = allowlist.iter().map(|s| s.to_string()).collect();
        Router::new()
            .route("/mcp", any(|| async { "ok" }))
            .layer(ip_allowlist_middleware(Arc::new(AllowlistState::new(
                &entries,
            ))))
    }

    fn cors_app(origins: &[&str]) -> axum::Router {
        let origins: Vec<String> = origins.iter().map(|s| s.to_string()).collect();
        Router::new()
            .route("/mcp", any(|| async { "ok" }))
            .layer(cors_layer(&origins))
    }

    fn post(path: &str) -> Request<Body> {
        Request::builder()
            .method("POST")
            .uri(path)
            .body(Body::empty())
            .unwrap()
    }

    fn with_token(req: Request<Body>, token: &str) -> Request<Body> {
        Request::builder()
            .method(req.method().clone())
            .uri(req.uri().clone())
            .header(header::AUTHORIZATION, format!("Bearer {token}"))
            .body(req.into_body())
            .unwrap()
    }

    fn with_peer(req: Request<Body>, peer: SocketAddr) -> Request<Body> {
        let mut req = req;
        req.extensions_mut().insert(ConnectInfo(peer));
        req
    }

    async fn call(app: axum::Router, req: Request<Body>) -> Response<Body> {
        app.oneshot(req).await.expect("router is infallible")
    }

    fn hex(s: &str) -> [u8; 32] {
        let mut out = [0u8; 32];
        for i in 0..32 {
            out[i] = u8::from_str_radix(&s[i * 2..i * 2 + 2], 16).unwrap();
        }
        out
    }

    // -------------------------------------------------------------- sha-256

    #[test]
    fn sha256_fips_empty_vector() {
        assert_eq!(
            sha256(b""),
            hex("e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855")
        );
    }

    #[test]
    fn sha256_fips_abc_vector() {
        assert_eq!(
            sha256(b"abc"),
            hex("ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad")
        );
    }

    #[test]
    fn sha256_fips_two_block_vector() {
        assert_eq!(
            sha256(b"abcdbcdecdefdefgefghfghighijhijkijkljklmklmnlmnomnopnopq"),
            hex("248d6a61d20638b8e5c026930c3e6039a33ce45964ff2167f6ecedd419db06c1")
        );
    }

    #[test]
    fn sha256_fips_million_a_vector() {
        assert_eq!(
            sha256(&vec![b'a'; 1_000_000]),
            hex("cdc76e5c9914fb9281a1c7e284d73e67f1809a48a497200e046d39ccc7112cd0")
        );
    }

    // ---------------------------------------------------------------- bearer

    #[tokio::test]
    async fn bearer_good_token_returns_200() {
        let resp = call(
            bearer_app(Some("secret-token")),
            with_token(post("/mcp"), "secret-token"),
        )
        .await;
        assert_eq!(resp.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn bearer_wrong_token_returns_401_with_challenge() {
        let resp = call(
            bearer_app(Some("secret-token")),
            with_token(post("/mcp"), "wrong-token"),
        )
        .await;
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
        assert_eq!(
            resp.headers().get(header::WWW_AUTHENTICATE),
            Some(&HeaderValue::from_static("Bearer"))
        );
    }

    #[tokio::test]
    async fn bearer_missing_token_returns_401_with_challenge() {
        let resp = call(bearer_app(Some("secret-token")), post("/mcp")).await;
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
        assert_eq!(
            resp.headers().get(header::WWW_AUTHENTICATE),
            Some(&HeaderValue::from_static("Bearer"))
        );
    }

    #[tokio::test]
    async fn bearer_options_preflight_passes_without_token() {
        let req = Request::builder()
            .method("OPTIONS")
            .uri("/mcp")
            .header(header::ORIGIN, "https://client.example")
            .body(Body::empty())
            .unwrap();
        let resp = call(bearer_app(Some("secret-token")), req).await;
        assert_eq!(
            resp.status(),
            StatusCode::OK,
            "OPTIONS must reach the handler without a token"
        );
    }

    #[tokio::test]
    async fn bearer_disabled_when_auth_key_is_none() {
        let resp = call(bearer_app(None), post("/mcp")).await;
        assert_eq!(resp.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn bearer_disabled_when_auth_key_is_empty() {
        let resp = call(bearer_app(Some("")), post("/mcp")).await;
        assert_eq!(resp.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn bearer_rejects_token_in_query_string() {
        // `?access_token=...` in the URL is never accepted (MCP spec MUST NOT).
        let resp = call(
            bearer_app(Some("secret-token")),
            post("/mcp?access_token=secret-token"),
        )
        .await;
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn bearer_rejects_other_auth_schemes() {
        let req = Request::builder()
            .method("POST")
            .uri("/mcp")
            .header(header::AUTHORIZATION, "Basic dXNlcjpwYXNz")
            .body(Body::empty())
            .unwrap();
        let resp = call(bearer_app(Some("secret-token")), req).await;
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn bearer_rejects_lowercase_scheme() {
        let req = Request::builder()
            .method("POST")
            .uri("/mcp")
            .header(header::AUTHORIZATION, "bearer secret-token")
            .body(Body::empty())
            .unwrap();
        let resp = call(bearer_app(Some("secret-token")), req).await;
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn bearer_rejects_empty_token() {
        let req = Request::builder()
            .method("POST")
            .uri("/mcp")
            .header(header::AUTHORIZATION, "Bearer ")
            .body(Body::empty())
            .unwrap();
        let resp = call(bearer_app(Some("secret-token")), req).await;
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    }

    #[test]
    fn bearer_token_extraction_accepts_exact_prefix() {
        let mut headers = HeaderMap::new();
        headers.insert(header::AUTHORIZATION, "Bearer tok123".parse().unwrap());
        assert_eq!(bearer_token_from(&headers), Some(&b"tok123"[..]));
    }

    #[test]
    fn bearer_token_extraction_rejects_malformed() {
        let cases: [&str; 4] = ["", "tok123", "Basic tok123", "bearer tok123"];
        for value in cases {
            let mut headers = HeaderMap::new();
            if !value.is_empty() {
                headers.insert(header::AUTHORIZATION, value.parse().unwrap());
            }
            assert!(
                bearer_token_from(&headers).is_none(),
                "header {value:?} must not yield a token"
            );
        }
    }

    #[test]
    fn digests_equal_true_only_for_identical_token() {
        assert!(digests_equal(&sha256(b"tok"), b"tok"));
        assert!(!digests_equal(&sha256(b"tok"), b"tok2"));
        assert!(!digests_equal(&sha256(b"tok"), b""));
    }

    // ------------------------------------------------------------- allowlist

    #[tokio::test]
    async fn ip_allowlist_admits_peer_in_list() {
        let peer = SocketAddr::from(([192, 168, 122, 5], 1234));
        let resp = call(ip_app(&["192.168.122.0/24"]), with_peer(post("/mcp"), peer)).await;
        assert_eq!(resp.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn ip_allowlist_denies_peer_not_in_list() {
        let peer = SocketAddr::from(([10, 0, 0, 1], 1234));
        let resp = call(ip_app(&["192.168.122.0/24"]), with_peer(post("/mcp"), peer)).await;
        assert_eq!(resp.status(), StatusCode::FORBIDDEN);
    }

    #[tokio::test]
    async fn ip_allowlist_matches_ipv4_mapped_ipv6_peer() {
        // `::ffff:192.168.122.5` must match the v4 CIDR after normalization.
        let peer = SocketAddr::new(
            IpAddr::V6(Ipv6Addr::new(0, 0, 0, 0, 0, 0xffff, 0xc0a8, 0x7a05)),
            1234,
        );
        assert_eq!(peer.ip().to_string(), "::ffff:192.168.122.5");
        let resp = call(ip_app(&["192.168.122.0/24"]), with_peer(post("/mcp"), peer)).await;
        assert_eq!(resp.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn ip_allowlist_denies_ipv4_mapped_ipv6_outside_list() {
        let peer = SocketAddr::new(
            IpAddr::V6(Ipv6Addr::new(0, 0, 0, 0, 0, 0xffff, 0x0a00, 0x0001)),
            1234,
        );
        let resp = call(ip_app(&["192.168.122.0/24"]), with_peer(post("/mcp"), peer)).await;
        assert_eq!(resp.status(), StatusCode::FORBIDDEN);
    }

    #[tokio::test]
    async fn ip_allowlist_empty_allows_all_peers() {
        let peer = SocketAddr::from(([203, 0, 113, 7], 1234));
        let resp = call(ip_app(&[]), with_peer(post("/mcp"), peer)).await;
        assert_eq!(resp.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn ip_allowlist_denies_when_peer_unknown() {
        // Fail closed: no ConnectInfo present and a non-empty allowlist → 403.
        let resp = call(ip_app(&["192.168.122.0/24"]), post("/mcp")).await;
        assert_eq!(resp.status(), StatusCode::FORBIDDEN);
    }

    #[tokio::test]
    async fn ip_allowlist_ignores_forwarded_for_spoofing() {
        // Peer is denied, X-Forwarded-For claims an allowed address. The
        // forwarded header must never be trusted (locked).
        let peer = SocketAddr::from(([10, 0, 0, 1], 1234));
        let mut req = post("/mcp");
        req.headers_mut()
            .insert("x-forwarded-for", "192.168.122.5".parse().unwrap());
        let resp = call(ip_app(&["192.168.122.0/24"]), with_peer(req, peer)).await;
        assert_eq!(resp.status(), StatusCode::FORBIDDEN);
    }

    #[tokio::test]
    async fn ip_allowlist_with_real_socket_address() {
        // Exercise a genuine OS-provided socket address through the
        // middleware: 127.0.0.1 from a bound loopback listener.
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let peer = listener.local_addr().unwrap();
        let resp = call(ip_app(&["127.0.0.1/32"]), with_peer(post("/mcp"), peer)).await;
        assert_eq!(resp.status(), StatusCode::OK);
    }

    #[test]
    fn normalize_peer_maps_ipv4_mapped_ipv6() {
        let mapped = IpAddr::V6(Ipv6Addr::new(0, 0, 0, 0, 0, 0xffff, 0xc0a8, 0x7a05));
        assert_eq!(
            normalize_peer(mapped),
            IpAddr::V4(Ipv4Addr::new(192, 168, 122, 5))
        );
    }

    #[test]
    fn normalize_peer_leaves_plain_addresses_alone() {
        let v4 = IpAddr::V4(Ipv4Addr::new(192, 168, 122, 5));
        assert_eq!(normalize_peer(v4), v4);
        let v6 = IpAddr::V6(Ipv6Addr::LOCALHOST);
        assert_eq!(normalize_peer(v6), v6);
    }

    #[test]
    fn peer_ip_reads_connect_info_extension_only() {
        let mut req = post("/mcp");
        assert_eq!(peer_ip(&req), None);
        let peer = SocketAddr::from(([10, 0, 0, 1], 9));
        req.extensions_mut().insert(ConnectInfo(peer));
        assert_eq!(peer_ip(&req), Some(peer.ip()));
    }

    #[test]
    fn allowlist_state_rejects_invalid_cidr() {
        let entries = vec!["not-a-cidr".to_string()];
        let result = std::panic::catch_unwind(|| AllowlistState::new(&entries));
        assert!(result.is_err(), "invalid CIDR must panic (fail closed)");
    }

    // ------------------------------------------------------------------ cors

    #[tokio::test]
    async fn cors_empty_origins_emit_no_acao() {
        let app = cors_app(&[]);
        let req = Request::builder()
            .method("POST")
            .uri("/mcp")
            .header(header::ORIGIN, "https://client.example")
            .body(Body::empty())
            .unwrap();
        let resp = call(app, req).await;
        assert_eq!(resp.status(), StatusCode::OK);
        assert!(
            resp.headers()
                .get(header::ACCESS_CONTROL_ALLOW_ORIGIN)
                .is_none(),
            "no configured origins must emit no ACAO header"
        );
    }

    #[tokio::test]
    async fn cors_allowed_origin_is_echoed() {
        let app = cors_app(&["https://good.example"]);
        let req = Request::builder()
            .method("POST")
            .uri("/mcp")
            .header(header::ORIGIN, "https://good.example")
            .body(Body::empty())
            .unwrap();
        let resp = call(app, req).await;
        assert_eq!(resp.status(), StatusCode::OK);
        assert_eq!(
            resp.headers().get(header::ACCESS_CONTROL_ALLOW_ORIGIN),
            Some(&HeaderValue::from_static("https://good.example"))
        );
    }

    #[tokio::test]
    async fn cors_disallowed_origin_gets_no_acao() {
        // tower-http CorsLayer does not reject the request; it simply omits
        // the ACAO header so the browser blocks the response. The 403 for a
        // disallowed Origin comes from the rmcp-native Origin validation
        // (owned by the http unit), not from the CORS layer.
        let app = cors_app(&["https://good.example"]);
        let req = Request::builder()
            .method("POST")
            .uri("/mcp")
            .header(header::ORIGIN, "https://evil.example")
            .body(Body::empty())
            .unwrap();
        let resp = call(app, req).await;
        assert_eq!(resp.status(), StatusCode::OK);
        assert!(
            resp.headers()
                .get(header::ACCESS_CONTROL_ALLOW_ORIGIN)
                .is_none(),
            "disallowed origin must not be echoed"
        );
    }

    #[tokio::test]
    async fn cors_preflight_allowed_origin_answers_with_headers() {
        let app = cors_app(&["https://good.example"]);
        let req = Request::builder()
            .method("OPTIONS")
            .uri("/mcp")
            .header(header::ORIGIN, "https://good.example")
            .header("access-control-request-method", "POST")
            .header(
                "access-control-request-headers",
                "authorization, content-type",
            )
            .body(Body::empty())
            .unwrap();
        let resp = call(app, req).await;
        assert_eq!(resp.status(), StatusCode::OK);
        assert_eq!(
            resp.headers().get(header::ACCESS_CONTROL_ALLOW_ORIGIN),
            Some(&HeaderValue::from_static("https://good.example"))
        );
        let allow_methods = resp
            .headers()
            .get(header::ACCESS_CONTROL_ALLOW_METHODS)
            .and_then(|v| v.to_str().ok())
            .unwrap_or("");
        assert!(
            allow_methods.contains("POST") && allow_methods.contains("OPTIONS"),
            "allow-methods was {allow_methods:?}"
        );
        let allow_headers = resp
            .headers()
            .get(header::ACCESS_CONTROL_ALLOW_HEADERS)
            .and_then(|v| v.to_str().ok())
            .unwrap_or("");
        for expected in [
            "authorization",
            "mcp-session-id",
            "mcp-protocol-version",
            "last-event-id",
            "content-type",
        ] {
            assert!(
                allow_headers.to_ascii_lowercase().contains(expected),
                "allow-headers {allow_headers:?} missing {expected}"
            );
        }
    }

    #[tokio::test]
    async fn cors_preflight_disallowed_origin_gets_no_acao() {
        let app = cors_app(&["https://good.example"]);
        let req = Request::builder()
            .method("OPTIONS")
            .uri("/mcp")
            .header(header::ORIGIN, "https://evil.example")
            .header("access-control-request-method", "POST")
            .body(Body::empty())
            .unwrap();
        let resp = call(app, req).await;
        assert!(
            resp.headers()
                .get(header::ACCESS_CONTROL_ALLOW_ORIGIN)
                .is_none(),
            "disallowed preflight origin must not be echoed"
        );
    }
}
