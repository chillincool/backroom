# Backroom: notes for Claude

Native IRC client for the [soju](https://soju.im) bouncer. Rust core with all
logic; front ends (SwiftUI on macOS first, iOS later) only supply platform
services. Owner: chillincool. Public repo: github.com/chillincool/backroom.

## Rules

- **Plan first.** Show the plan (layout, dependencies, boundaries) and wait for
  an OK before writing code for a new phase. Ask rather than guess about soju
  behaviour or the owner's config; check soju's source/docs and the IRCv3 specs
  instead of relying on memory.
- **Never log, print or store tokens or passwords.** Secrets are
  `secrecy::SecretString`; `Debug` must not reveal them; raw IRC tracing
  redacts `AUTHENTICATE`.
- **`backroom-core` has no platform code.** Browser login and token storage go
  behind traits the front end implements (`TokenStore`, OIDC `begin`/`complete`).
- **Prefer well-maintained crates** over hand-rolling anything crypto-related.
  TLS is rustls + ring only (no OpenSSL).
- **`main` is protected** (ruleset, no bypass): work on a branch, open a PR,
  `gh pr merge --squash --auto`. Each PR runs ~3 min of CI, so batch changes.
- **Instructions for the owner:** short, complete, one command per block; say
  when a command prompts; use the browser only where there's no CLI/API.

## Layout

- `crates/backroom-core`: the library
  - `proto/` message parsing, CAP, numerics, mIRC formatting (strip only)
  - `transport/` `Transport`/`Connector` traits; WebSocket (`wss://`, `ws://`)
    and TCP (`ircs://`, `irc+insecure://`) via `connector_for_url`; extra CAs
    via `TlsOptions`
  - `session/registration.rs` sans-IO CAP → SASL → `BOUNCER BIND` → `CAP END`
  - `sasl.rs`, `auth/` (OIDC + PKCE, refresh, `OidcCredentials`, `jwt_peek`)
  - `client.rs` async driver; `next_incoming()` is cancel-safe for `select!`
  - `live/` long-lived session for apps (see below); `model.rs` chat model
- `crates/backroom-ffi`: UniFFI 0.32 bindings. One `BackroomClient` object
  (owns a tokio runtime; async methods spawn onto it). Swift implements
  `SecretStore` (`#[uniffi::export(foreign)]`): one opaque Keychain value,
  the token JSON from `TokenSet::to_json`. FFI copies of model/events use
  `SystemTime`/`Duration`. `--features bindgen` builds `uniffi-bindgen`.
- `apple/BackroomCore`: Swift package (XCFramework + generated bindings, both
  git-ignored, from `scripts/build-apple.sh`) plus `KeychainSecretStore`.
  Its tests use Swift Testing, which needs Xcode (not in the CLT).
- `crates/backroom-cli`: `backroom` test CLI (`login`, `networks`, `history`,
  `token`, `logout`; `--sasl plain`, `--raw`, `--ca-file`). Dev-only token file
  at `~/Library/Application Support/backroom/dev-tokens.json`.
- `test/e2e/`: Docker Compose stack; `scripts/e2e.sh up|test|down|all|realm`
- `MAINTAINING.md`: repo settings, Renovate app setup, key rotation

## Commands

```sh
mise run ci        # fmt, clippy -D warnings, unit tests, cargo-deny
mise run e2e       # full e2e in Docker (~2 min on CI; Keycloak dominates)
mise run e2e:up    # then e2e:test repeatedly, then e2e:down

mise run apple:core # build backroom-ffi + regenerate apple/BackroomCore (needs Xcode)
```

The repo is mise-trusted; Rust comes from mise (rustup in `~/.cargo`), not
Homebrew. Install tools with mise, not brew (`mise use -g` for personal tools).

## soju and Keycloak facts (verified)

- Production soju v0.11.1, only reachable at `wss://irc.chillincool.net/socket`
  (behind `envoy-internal`); config in `~/chillincool-infra/kubernetes/apps/util/soju/`.
  `message-store db`, `auth oauth2` introspecting as confidential client `soju`.
- soju's oauth2 auth only checks introspection `active` + `username`; no roles
  or scopes. Under `auth oauth2`, SASL PLAIN means username + access token as
  the password. Its 904 text is generic; real reasons are in soju's log.
- Keycloak 26.8.1. Since 26.6.2, introspection returns `active: false` unless
  the introspecting client (`soju`) is in the token's `aud`.
- Access gate: client scope `soju_roles` maps `soju.access` (Scope tab), a
  User Client Role mapper (claim `soju_roles`) and **Audience Resolve**, so
  only users with the role get `aud: soju`. The browser flow also denies them
  ("gate soju" conditional subflow). The e2e realm mirrors this
  (`test/e2e/keycloak/build-realm.sh`).
- Keycloak client `backroom`: public, PKCE S256, redirect
  `http://127.0.0.1:47615/callback`, full scope off, default scope `soju_roles`.
- Names: `draft/chathistory`, `draft/read-marker` (`MARKREAD`),
  `soju.im/bouncer-networks(-notify)`. Chat history needs a connection bound
  with `BOUNCER BIND`; passthrough caps depend on the upstream.

## `live` session (core)

`live::start(SessionConfig)` → `(SessionHandle, Receiver<Event>)`. A
coordinator task owns all state; one task per connection (control +
one per network) does I/O with backoff (1 s doubling to 60 s), keepalive PINGs,
and stops after a rejected login (`ConnectionState::AuthFailed`). Events:
connection state, networks, buffers, live messages, history pages, read
markers, errors, stopped. Commands: send, join, part, load history, mark read,
shutdown. Coordinator is tested with an injected spawner; the connection task
with `MockConnector::concurrent`.

## Status (2026-11-13)

- Phase 1 (core + CLI) merged as `cece7a5`; Phase 2 step 1 (`live`) merged
  as `4d40543` (PR #8).
- Phase 2 step 2 (`backroom-ffi` + `apple/BackroomCore`) on branch
  `feat/ffi`. Verified without Xcode: Swift bindings compile and a swiftc
  smoke test passes; `KeychainSecretStore` round-trips; the package builds
  against a hand-assembled XCFramework. `build-apple.sh` itself is untested
  until Xcode is installed.
- CodeQL flags e2e test passwords (`rust/hard-coded-cryptographic-value`) and
  the ruleset blocks merges on them: dismiss as "used in tests".
- Codeberg can rebuild a soju tag, deleting the pinned digest ("manifest
  unknown" in e2e); Renovate doesn't notice. Re-pin from
  `docker buildx imagetools inspect`.

## Decisions

- Bundle ID `net.chillincool.backroom`; login redirect
  `backroom://oauth/callback` (in the e2e realm; add to production Keycloak's
  `backroom` client when the app is used for real).
- Intel-only builds while developing; `--universal` when packaging.
- Develop the app against the e2e stack (`mise run e2e:up`, Keycloak on
  `127.0.0.1:18080`).

## Next: Phase 2 (macOS app), agreed plan

3. SwiftUI app in `apple/`, project generated by XcodeGen (`apple/project.yml`),
   macOS 14+, `@Observable` `AppModel`, `ASWebAuthenticationSession` login,
   Keychain token store. First UI: login, `NavigationSplitView` (networks →
   buffers with unread), message list with scroll-back, composer. Needs
   Xcode 26.x installed.
4. macOS CI job (`xcodebuild build test`), then add it to the `main` ruleset.

Open: first-milestone UI scope. `connect` needs OIDC discovery, so it fails
offline even with valid tokens; revisit if that matters.

## Environment constraints

- Owner's Mac is **Intel** (i9), macOS 26.3. Xcode 27 is Apple-silicon only,
  so use **Xcode 26.x** from developer.apple.com/download/all (not installed
  yet; only Command Line Tools, which include `swiftc` but not
  `xcodebuild` or Swift Testing).
- No paid Apple Developer Program, and the owner doesn't want to pay $99/yr:
  the app is for personal use first; free sharing means unsigned builds
  ("Open Anyway"). Homebrew casks now require notarization. The core stays
  platform-neutral so a Linux/other front end remains an option.
- Docker Desktop is slow on this machine; Keycloak can take minutes to start.
