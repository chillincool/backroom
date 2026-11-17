# Backroom

A native IRC client for the [soju](https://soju.im) bouncer, or for IRC
networks directly. A Rust core holds the logic; front ends supply only platform
services: SwiftUI on macOS (via UniFFI) and GTK 4 + libadwaita on Linux.

> **Early development.** The macOS and Linux apps work against soju and
> without a bouncer; expect rough edges.

## Linux

Runs on any desktop (GNOME, KDE, Xfce, …) with GTK ≥ 4.14 and libadwaita ≥ 1.5
(Ubuntu 24.04, Debian 13, Fedora 40, RHEL 10 or later), or anywhere as a
Flatpak. Logins are kept in your keyring (GNOME Keyring, KWallet, KeePassXC).
From a checkout, with [mise](https://mise.jdx.dev) and the GTK development
packages (see [CONTRIBUTING](CONTRIBUTING.md)):

```sh
mise run linux:run        # run from source
mise run linux:install    # install for your user: app menu entry, login callback
mise run linux:flatpak    # or build and install the Flatpak
mise run linux:deb        # or a .deb (target/debian/), linux:rpm for an .rpm
```

## Layout

- `crates/backroom-core`: platform-independent library
  - `proto/`: IRC message parsing and serialization, CAP, numerics, formatting
  - `transport/`: line-oriented `Transport`/`Connector` traits; WebSocket
    (`wss://`, `ws://`) and TCP (`ircs://`, `irc+insecure://`), picked by URL
  - `session/`: sans-IO registration (CAP → SASL → BOUNCER BIND → CAP END)
  - `sasl.rs`: OAUTHBEARER / PLAIN encoding and chunking
  - `auth/`: OIDC + PKCE, refresh, `TokenStore` / `CredentialProvider` traits
  - `batch.rs`, `bouncer.rs`, `client.rs`: batches, bouncer-networks, async driver
  - `live/`: long-lived session for apps: control + per-network connections,
    reconnect with backoff, events out (messages, buffers, history, read
    markers) and commands in
  - `model.rs`: platform-neutral chat model and message routing
- `crates/backroom-ffi`: UniFFI bindings for the Swift app
- `crates/backroom-app`: app logic shared by native front ends (chat state,
  composer commands, Tab completion, timeline, panes and tabs, settings, themes)
- `crates/backroom-gtk`: the Linux app (GTK 4 + libadwaita)
- `apple/`: the macOS app (SwiftUI)
- `crates/backroom-cli`: the `backroom` spike CLI

## CLI

```sh
cargo run -q -- login                         # browser login, stores tokens (dev only)
cargo run -q -- token                         # show stored token claims
cargo run -q -- networks                      # list upstream networks
cargo run -q -- history -N libera '#channel'  # recent history with timestamps
cargo run -q -- networks --sasl plain --user chris
cargo run -q -- --raw networks                # raw IRC traffic, AUTHENTICATE redacted
cargo run -q -- --url ircs://soju.example networks --ca-file ca.pem   # raw TLS port, private CA
cargo run -q -- logout
```

Tokens are stored in `~/Library/Application Support/backroom/dev-tokens.json`
(mode 0600). This is for testing only; the app will use the Keychain.

If SASL fails, the CLI prints soju's exact reply and the token's claims, then
exits with code 2. soju's 904 text is generic; the actual reason is in soju's
log (`kubectl -n util logs deploy/soju | grep -i sasl`).

## Keycloak client

Public client `backroom`: standard flow, PKCE `S256`, redirect URI
`http://127.0.0.1:47615/callback`, full scope off, default client scope
`soju_roles`.

soju only checks that introspection says `active` and returns a username. Since
Keycloak 26.6.2, introspection also requires the introspecting client (`soju`)
to be in the token's `aud`. Access is therefore gated in Keycloak:

- `soju_roles` maps the `soju.access` client role (Scope tab), puts it in the
  token (User Client Role mapper) and adds `aud: soju` via **Audience Resolve**,
  only for users who have that role. No hard-coded Audience mapper.
- The browser flow denies login to users without `soju.access` when the client
  has `soju_roles`.

## Tests

Tasks are defined in `mise.toml` and are the same locally and in CI.

```sh
mise run ci        # fmt, clippy -D warnings, unit tests
mise run e2e       # end-to-end, needs Docker (start, test, stop)
mise run e2e:up    # or keep the stack running: e2e:up, then e2e:test repeatedly, e2e:down
cargo test -p backroom-core --test live -- --ignored   # CAP LS against irc.chillincool.net (manual)
```

The e2e stack (`test/e2e/compose.yaml`) runs the production soju and Keycloak
versions, plus ergo as an upstream IRC server:

| soju instance | auth | tested logins |
| --- | --- | --- |
| `soju-oauth2` | `oauth2` (Keycloak) | OAUTHBEARER, PLAIN with the token, refresh, role gate |
| `soju-internal` | `internal` | PLAIN with a soju password |
| `soju-http` | `http` (Caddy stub) | PLAIN checked over HTTP |

Each is tested over WebSocket, `ircs://` and `irc+insecure://`, plus network
listing, `BOUNCER ADDNETWORK` and chat history through ergo. `auth pam` is not
tested: the soju image has no PAM setup.

The Keycloak realm (`test/e2e/keycloak/realm.json`) mirrors production: the
`soju_roles` scope with Audience Resolve, and a browser flow that denies users
without `soju.access`. It is generated by `test/e2e/keycloak/build-realm.sh`;
rerun `mise run e2e:realm` after changing that script.
