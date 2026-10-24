# Contributing

Thanks for your interest! Bug reports, docs fixes and small focused PRs are all welcome.
For anything larger, please open an issue first so we can agree on the approach.

## Development

Install [mise](https://mise.jdx.dev); it installs the exact Rust toolchain and
cargo-deny version CI uses. The end-to-end tests also need Docker.

```sh
mise install                 # once
mise run test                # unit tests
mise run ci                  # everything CI checks, except e2e
mise run e2e                 # end-to-end tests in Docker (~3 min, most of it Keycloak starting)
```

The end-to-end tests start Keycloak, ergo and three soju instances (one each
for `auth oauth2`, `auth internal` and `auth http`), then log in every
supported way over WebSocket, `ircs://` and `irc+insecure://`, check the
Keycloak role gate, and send and read back chat history through ergo. See
[`test/e2e`](test/e2e) and [`crates/backroom-core/tests/e2e.rs`](crates/backroom-core/tests/e2e.rs).

To iterate on a change, keep the stack up instead of restarting it every time:

```sh
mise run e2e:up      # start the stack once
mise run e2e:test    # rerun the tests (repeat as needed)
mise run e2e:down    # stop it and delete its data
```

To try the CLI against a real soju:

```sh
cargo run -q -- --help
cargo run -q -- --url wss://soju.example/socket --issuer https://idp.example/realms/x login
```

Layout:

| Path | What's there |
| --- | --- |
| `crates/backroom-core` | All client logic, no platform code: protocol, transports, SASL, OIDC, soju extensions |
| `crates/backroom-cli` | The `backroom` command-line client used to exercise the core |
| `test/e2e` | Docker Compose stack, soju configs, Keycloak realm for the e2e tests |
| `scripts` | Helpers used by mise tasks and CI |

## Pull requests

- Run `mise run ci` before pushing. If you touched connecting, login or
  anything soju-specific, also run `mise run e2e`.
- Never log, print or store tokens or passwords. Secrets live in
  `secrecy::SecretString`, and `Debug` output must not reveal them.
- `backroom-core` must stay free of platform-specific code; anything that needs
  the OS (browser, keychain) goes behind a trait the front end implements.
- If you change behaviour or configuration, update the README.
- Keep dependencies to a minimum, and prefer well-maintained crates over
  hand-rolled code for anything crypto-related.
