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
  redacts `AUTHENTICATE` and JOIN keys.
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
  - `proto/` message parsing, CAP (`WANTED_CAPS`), numerics, mIRC formatting
    (strip, or styled spans with the 99-colour palette)
  - `transport/` `Transport`/`Connector` traits; WebSocket (`wss://`, `ws://`)
    and TCP (`ircs://`, `irc+insecure://`) via `connector_for_url`; extra CAs
    via `TlsOptions`
  - `session/registration.rs` sans-IO CAP → SASL → `BOUNCER BIND` → `CAP END`
  - `sasl.rs`, `auth/` (OIDC + PKCE, refresh, `OidcCredentials`, `jwt_peek`)
  - `client.rs` async driver; `next_incoming()` is cancel-safe for `select!`
  - `live/` long-lived session for apps (see below); `members.rs` member
    lists and away state
  - `local/` standalone mode (no bouncer): `LocalStore` (SQLite via
    `rusqlite` bundled: networks, channels, messages, read markers),
    `NetworkConfig` (network attributes → connection), `SecretVault` trait
    (passwords; front end implements), `TextLog` (optional plain-text logs)
  - `model.rs` chat model: `route` (message → buffer), `route_reply`
    (numerics), `quit_or_nick`, `WhoisInfo`, `ChannelListing`, `BanEntry`
- `crates/backroom-ffi`: UniFFI 0.32 bindings. One `BackroomClient` object
  (owns a tokio runtime; async methods spawn onto it). `ClientConfig.login`
  is a `LoginMethod`: `Oidc` (browser login, OAUTHBEARER), `Password`
  (soju's own accounts, SASL PLAIN; `login_with_password`) or `Standalone`
  (no bouncer: SQLite path, optional text log folder; `set_text_logs`
  switches logs live). Swift implements `SecretStore`
  (`#[uniffi::export(foreign)]`): one opaque Keychain value per login, the
  token JSON from `TokenSet::to_json` or the password; standalone keeps all
  network passwords and channel keys as one JSON object in one item
  (`ForeignVault`, account `standalone`), so at most one Keychain prompt. FFI copies of model/events use
  `SystemTime`/`Duration`. `--features bindgen` builds `uniffi-bindgen`.
- `apple/BackroomCore`: Swift package (XCFramework + generated bindings, both
  git-ignored, from `scripts/build-apple.sh`) plus `KeychainSecretStore`.
  Its tests use Swift Testing, which needs Xcode (not in the CLT).
- `apple/Backroom`: SwiftUI app (`project.yml` → XcodeGen; project, build dir
  and Info.plist are git-ignored). `AppModel` (`@Observable`, main actor)
  owns the client via `BackroomClientProtocol` (faked in tests) and applies
  events; `Preferences` (`@Observable`, UserDefaults) and `Theme` reach
  views through the environment (`\.theme`). `Model/`: `AppModel`,
  `ChatState` (`BufferModel` per buffer), `Composer` (slash commands, Tab
  `Completion`), `ServerSettings` (`ServerList`: saved servers, one in use;
  each a password or single sign-on login with its own Keychain item, plus
  the built-in "No bouncer" entry, always last; `NetworkLogin` for
  standalone networks' SASL / server password / NickServ),
  `Timeline` (folding, day separators, New line), `Panes`
  (`PaneLayout` split tree, `WorkspaceTab`), `Mentions`, `Notifier`,
  `Preferences`, `Theme`. `Views/`: `MainView` (sidebar | panes | members,
  fixed widths via `SplitDivider`), `PaneViews`, `BufferView` (rows,
  composer, typing), `TabStrip`, sheets (`JoinViews`, `SearchView`,
  `ChannelSettingsView`, `WhoisView`), `Settings/` (one file per tab).
  Login opens the default browser and receives
  `backroom://oauth/callback` via `onOpenURL` (`BrowserLogin`), not
  `ASWebAuthenticationSession`: that delegates to the default browser, and
  Firefox fails it instantly as "cancelled". Signing: `Signing.xcconfig`
  (ad hoc) plus optional git-ignored `Signing.local.xcconfig`. Under XCTest
  the app doesn't start a session.
- `crates/backroom-cli`: `backroom` test CLI (`login`, `networks`, `history`,
  `token`, `logout`; `--sasl plain`, `--raw`, `--ca-file`). Dev-only token file
  at `~/Library/Application Support/backroom/dev-tokens.json`.
- `test/e2e/`: Docker Compose stack; `scripts/e2e.sh up|test|down|all|realm`.
  ergo is on `127.0.0.1:46667` for standalone tests.
- `MAINTAINING.md`: repo settings, Renovate app setup, key rotation

## Commands

```sh
mise run ci        # fmt, clippy -D warnings, unit tests, cargo-deny
mise run e2e       # full e2e in Docker (~2 min on CI; Keycloak dominates)
mise run e2e:up    # then e2e:test repeatedly, then e2e:down

mise run apple:core # build backroom-ffi + regenerate apple/BackroomCore (needs Xcode)
mise run apple:app  # generate apple/Backroom.xcodeproj (XcodeGen), build, unit tests
```

Run the app: `open apple/build/Build/Products/Debug/Backroom.app`. Against the
e2e stack, log in as alice/alice (single sign-on), or with a password as
dave/dave-secret on soju-internal (`ws://127.0.0.1:28081/`); Settings →
Account → Add Test Servers (debug builds) adds both; give her a network with
`docker compose -p backroom-e2e exec -T soju-oauth2 sojuctl -config /soju-config user run alice network create -addr irc+insecure://ergo:6667 -name ergo-test`
(after her first login).

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
and stops after a rejected login (`ConnectionState::AuthFailed`), including a
server that doesn't offer our SASL mechanism. Each connection gets a
`ConnSpec` (connector, login, `ConnectOptions`).

- `SessionConfig.backend`: `Bouncer` (soju) or `Standalone` (`local/`).
  Standalone: no control connection (control state is `Connected` at once);
  networks come from the store and connect directly (SASL PLAIN, `PASS`, or
  NickServ IDENTIFY when SASL didn't log in); network state goes into the
  `state`/`error` attributes like soju's. History, read markers, search
  and recent conversations are answered from SQLite; every `Event::Message`
  except replies/errors is recorded (one hook in `emit`). Own JOINs save
  the channel (keys go to the vault on success), PART/KICK forget it;
  rejoins on connect are hidden like soju's replayed JOINs. Network
  passwords (`pass`, `sasl-password`, `nickserv-password`) never reach the
  database; events carry `<name>-set=1`.

- Events: connection state, networks, buffers, members, topics, live
  messages, history pages, read markers, channel list chunks, own nick,
  away, WHOIS, search results, typing, presence (MONITOR), channel modes,
  ban lists, errors, stopped.
- Commands: send, join (with key), part, load history, mark read, add /
  change / remove network, raw line, close buffer, list channels, search,
  typing, load channel settings, shutdown.
- Routing rules worth knowing: channel buffers only come from JOINs;
  CHATHISTORY TARGETS correspondents get a buffer only if their recent
  history has a real PRIVMSG; notices to us go to the server buffer unless
  a conversation exists; QUIT/NICK fan out to shared channels; errors about
  a channel go there; CTCP requests to us are answered here (VERSION string
  from `SessionConfig::client_version`); a quiet WHO (WHOX) after joining
  channels of up to 500 members fills away state.
- The coordinator is tested with an injected spawner (`Harness`); the
  connection task with `MockConnector::concurrent`.

## Status (2026-11-14)

- Merged: Phase 1 (`cece7a5`), `live` session (#8), `backroom-ffi` +
  `apple/BackroomCore` (#9), macOS app + `macos` CI job (#10), members,
  topics, network management, `offline_access` (#11).
- Merged PR 1 (#12): join/browse/new conversation, Settings window, themes
  (Rex-style cards), fonts, message layouts, New line, plain split layout.
- Merged PR 2 (#13): server replies, QUIT/NICK/MODE/INVITE, notices to the
  server buffer, WHOIS card, away, more caps, mIRC spans, commands, Tab
  completion, mentions/highlights/ignore, moderation menu, split panes and
  title-bar tabs.
- Merged PR 3 (#14): macOS notifications (mentions,
  PMs, optionally all; click opens; Dock badge), `soju.im/search` (⌘F),
  channel settings sheet (modes, key, limit, bans), `+typing` both ways,
  MONITOR online dots, CTCP VERSION/PING/TIME/CLIENTINFO answered.
- Merged #15: soju password logins (`auth internal`) and saved servers
  (login screen server menu, Settings → Account editor).
- Merged #16 + #17: standalone mode ("No bouncer"): core `local/`, FFI
  `LoginMethod::Standalone`, app entry with Connect, network login section
  (SASL / server password / NickServ), optional plain-text logs. Owner
  tested it against ergo: works. Database
  `~/Library/Application Support/Backroom/standalone.sqlite`, logs default
  to `…/Backroom/logs`; "No bouncer" reconnects at launch only if it was
  connected at quit (UserDefaults `standaloneConnected`).
- Verified soju v0.11.1 facts: SEARCH needs `in=` (max 100 results, batch
  `soju.im/search`); soju forwards CTCP to every client and answers none
  (two clients both answering means duplicate replies); TAGMSG reaches only
  clients with `message-tags` and `+typing` isn't stored; MONITOR needs the
  upstream's ISUPPORT MONITOR, lists merged across clients.
- Notifications: permission is asked at launch; asking on the first message
  lost it (macOS shows the request as its own notification). The Dock badge
  needs "Badge application icon" allowed.
- `macos` is deliberately **not** a required check yet (owner: wait until the
  app is further along; it adds ~6 min per PR).
- Dev testing uses the e2e stack's soju with real networks (Libera,
  P2P-Network) added for alice; production Keycloak still lacks the
  `backroom://oauth/callback` redirect.
- CodeQL flags e2e test passwords (`rust/hard-coded-cryptographic-value`) and
  the ruleset blocks merges on them: dismiss as "used in tests".
- Codeberg can rebuild a soju tag, deleting the pinned digest ("manifest
  unknown" in e2e); Renovate doesn't notice. Re-pin from
  `docker buildx imagetools inspect`.
- P2P-Network's servers partly offer only TLS 1.2 RSA key exchange, which
  Go (soju) refuses. Production pins `irc.p2p-network.net` to working
  servers via hostAliases; the e2e soju-oauth2 does the same (`extra_hosts`).
- SwiftUI performance: each buffer is its own `@Observable` (`BufferModel`)
  and the member list is a `LazyVStack`; a sectioned `List` with ~1,600 rows
  made channel switching take seconds.
- macOS 26 UI notes: `NavigationSplitView`'s sidebar floats around the window
  buttons (owner dislikes it; hence `HSplitView`); toolbar `.status` items
  lose their text; items get glass bubbles unless
  `sharedBackgroundVisibility(.hidden)` (`withoutGlass()`). Owner runs with
  transparency off; a Glass/Classic option was considered and skipped.
- Imported Keycloak users get no realm roles; the e2e realm gives test users
  `default-roles-e2e` (carries `offline_access`). Production: confirm the
  `backroom` client has `offline_access` as an optional client scope.

## Pick up here (2026-11-14)

- All of the owner's latest asks are done (#15 soju password login, #16/#17
  standalone mode). **Next work is the owner's call.** Remaining roadmap:
  ZNC support (low priority; bigger than standalone). One account per
  client: the owner sees no use for several accounts at once. A soju
  installer was rejected (standalone mode lowers the entry bar instead).
- A soju uses one login method (`auth internal`: `sasl=PLAIN`; `auth
  oauth2`: `sasl=PLAIN,OAUTHBEARER`, PLAIN taking a token). The method and
  issuer are set per saved server; no detection. Login screen: server menu,
  then that server's form ("Add soju Server…" opens Settings → Account);
  "No bouncer" is always the last entry, with a Connect button.
- Local stack quirk: soju-internal's dave/erin have piled up ergo networks
  from test runs, so ergo answers "Too many clients from your network" and
  `history_round_trip_through_ergo` / `live_session_…` fail locally (fine on
  CI's fresh stack). Fix by recreating the stack (`e2e.sh down`, which
  wipes alice's dev data too: ask first).
- Dev setup: e2e stack up (`mise run e2e:up`); soju-oauth2 data lives on
  volume `soju-oauth2-data` (survives recreation; `e2e.sh down` deletes it,
  so warn first). alice has networks libera (nick backroom-dev),
  P2P-Network (nick chillin-dev) and ergo-test (no nick set, so IRC nick
  `alice`, the soju username; re-added 2026-11-14).
- ergo on `127.0.0.1:46667`: `compose.yaml` publishes it, but the running
  local stack predates that, so the port is closed until ergo is recreated.
  Without recreating (ergo state survives), forward it temporarily:
  `docker run -d --rm --name backroom-ergo-forward --network backroom-e2e_default -p 127.0.0.1:46667:6667 alpine/socat:1.8.0.3 tcp-listen:6667,fork,reuseaddr tcp-connect:ergo:6667`
  (remove with `docker rm -f backroom-ergo-forward`). In the app: No
  bouncer → Connect → add network `127.0.0.1`, port 46667, TLS off.
- Test user "bob" on ergo (16667 is soju, not ergo):
  `docker run --rm -i --network backroom-e2e_default python:3.13-alpine`
  with a small socket script (stdin lines sent raw; the last one lived in
  the session scratchpad and is gone after it). Run it in a free Rex pane.
- Ideas not planned: UI polish (owner isn't fully happy with the look but
  wants to come back to it later), production use (add
  `backroom://oauth/callback` to production Keycloak's `backroom` client;
  confirm its `offline_access` optional scope), offline
  start without Keycloak, reactions, redaction, a CTCP off switch, `macos`
  as a required check (deferred by owner).

## Decisions

- Bundle ID `net.chillincool.backroom`; login redirect
  `backroom://oauth/callback` (in the e2e realm; add to production Keycloak's
  `backroom` client when the app is used for real).
- Intel-only builds while developing; `--universal` when packaging.
- Develop the app against the e2e stack (`mise run e2e:up`, Keycloak on
  `127.0.0.1:18080`).
- Themes: built-in ids start `builtin.`; nil colours mean the system colour
  (so Default looks native). Preferences live in UserDefaults under `prefs.`;
  tabs and pane layouts under `tabs` (JSON).
- Owner's UI taste: Rex is the reference (theme cards, light + dark theme
  per appearance, title-bar tabs left-aligned and only from 2 tabs); flat
  title bar, no glass; columns message layout by default; side panels keep
  the width they're dragged to.

## Environment constraints

- Owner's Mac is **Intel** (i9), macOS 26.3. Xcode 27 is Apple-silicon only,
  so use **Xcode 26.x** from developer.apple.com/download/all (installed
  in `/Applications`, 26.6).
- No paid Apple Developer Program, and the owner doesn't want to pay $99/yr:
  the app is for personal use first; free sharing means unsigned builds
  ("Open Anyway"). Homebrew casks now require notarization. The core stays
  platform-neutral so a Linux/other front end remains an option.
- Docker Desktop is slow on this machine; Keycloak can take minutes to start.
- **Temporary dev signing cert (remove when done):** "Backroom Development",
  a self-signed code-signing certificate in the owner's login keychain
  (created 2026-11-14, expires 2028-11-13), used through the git-ignored
  `apple/Signing.local.xcconfig` so Keychain "Always Allow" survives rebuilds.
  Without that file the build signs ad hoc. To remove it:
  `security delete-identity -c "Backroom Development" ~/Library/Keychains/login.keychain-db`,
  then delete `apple/Signing.local.xcconfig`.
