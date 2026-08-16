# Security

computer-use gives an AI agent control of your desktop: mouse, keyboard, screenshots, and screen recording. Treat it as remote control of your machine, because that is what it is. Run it only on machines and accounts you control, and only when you need it. The default mode is stdio, byte-identical to previous behavior; HTTP mode is opt-in via `--transport http`.

> **Warning.** Bearer auth guards the channel, not the agent.
> this authenticates the channel; a compromised agent can still type passwords and click destructive buttons.

## Threat model

The server defends against three kinds of attacker. Each attacker gets a defense aimed at the way that attacker reaches the server.

### Attacker A: DNS rebinding, or a malicious website reaching the server through your browser

The server binds to loopback by default, so no other host can connect at all. When a request does arrive, the server validates two headers:

- Origin header validation. A present Origin that is not allowed gets a 403. This is a MUST in the MCP spec. The default allowlist is loopback only.
- Host header validation against an allowed-hosts list. The default list is loopback only.

### Attacker B: another local process, or a host on your LAN

- Bearer token authentication. The token goes in the Authorization header only, never in the query string. Comparison is constant-time. A missing or invalid token gets a 401 with `WWW-Authenticate: Bearer`.
- Startup refusal for non-loopback binds. The server refuses to bind a non-loopback address unless an auth key is present AND either TLS is configured or `--allow-insecure-remote` is passed. Loopback binds need no token by default.

### Attacker C: a compromised or careless agent that holds valid credentials

No token check can stop this attacker, because the attacker is already inside. The only control that works here is the tool allow and deny lists. This defense is a headline feature, not an afterthought.

- Denied tools are hidden from `tools/list` and error when called.
- The lists are enforced on both transports, including stdio.
- `--allow-tools` (allow list) and `--block-tools` (deny list) select the tools. An empty allow list allows all tools.

The honest limit: this authenticates the channel; a compromised agent can still type passwords and click destructive buttons.

## Authentication

- Header only. The token is read from the `Authorization: Bearer` header. The server never accepts a token from the query string.
- Constant-time comparison. Timing cannot leak the token.
- Missing or invalid token: 401 with `WWW-Authenticate: Bearer`.
- OPTIONS preflight requests are exempt, so browser clients can negotiate CORS before sending the token.
- Loopback exemption. When the server binds loopback and no auth key is set, connections need no token. This keeps local use and MCP Inspector simple. Set `--auth-key` to require a token everywhere.

## Network hardening

- Loopback-only default bind. Out of the box the server listens only on loopback.
- Non-loopback startup refusal. Binding a non-loopback address requires `--auth-key` AND (TLS configured OR `--allow-insecure-remote`).
- IP allowlist. Optional `--ip-allowlist`. It matches the socket peer address only. The server never trusts `X-Forwarded-For` or any forwarded header. IPv4-mapped IPv6 addresses are normalized before the CIDR match. Default is empty, which allows all peers.
- CORS off by default. The server emits no `Access-Control-Allow-Origin` headers until you pass `--cors-origins`. When enabled, `Access-Control-Allow-Headers` includes `Authorization`, `MCP-Session-Id`, `MCP-Protocol-Version`, `Last-Event-ID`, and `Content-Type`.
- Origin and Host validation. See Attacker A above.

## TLS

TLS is optional and compiled behind a cargo feature `tls`, off by default. Loopback binds need no TLS. A non-loopback bind needs an auth key AND either a TLS cert and key or `--allow-insecure-remote`.

Phase-2 hardening will require TLS for all non-loopback binds.

## Tool access control

The allow and deny lists are the primary control against a compromised agent (Attacker C). They apply on both transports, including stdio. `--allow-tools` restricts the server to the listed tools; `--block-tools` removes the listed tools. Denied tools do not appear in `tools/list` and return an error when called.

## Deployment recommendations

- Run in a VM or sandbox, not on your daily driver.
- Run as a least-privilege OS user, not as root or your main account.
- Snapshot the machine before sessions, so you can roll back.
- Disable high-risk tools when you do not need them.
- Never expose the server over an untrusted network without authentication AND TLS.

## Documented as unsupported in v1

- OAuth2 + PKCE (phase 2).
- Rate limiting.
- mTLS.
- Per-tool approval UI.
- Screen-recording redaction.
- Multi-user accounts.

The auth-key mode returns 404 on `/.well-known/oauth-*` endpoints. This is a compatibility workaround for clients such as Cursor that force the OAuth flow whenever the discovery endpoints respond, ignoring static headers. A future oauth mode would serve discovery from those endpoints.

## Known limitations

- Per-session recording state. The server runs rmcp in stateful mode with one server instance per session. Recording state is per-session, so `start_recording` and `stop_recording` must share one MCP session.

## Reporting

Report security issues through the upstream issue tracker: https://github.com/PegasisForever/computer-use/issues
