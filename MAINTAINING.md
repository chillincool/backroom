# Maintaining Backroom

Notes for whoever looks after the repository. The setup mirrors
[tilegate](https://github.com/chillincool/tilegate).

## Releasing

Not set up yet. There's nothing to ship until the macOS app exists; the release
workflow will be added with it.

## Making changes

`main` is protected, so every change, including your own, goes through a pull request:

```sh
git switch -c fix/something
# ...edit, then: mise run ci   (and mise run e2e for connecting, login or soju features)
git commit -am "fix: something"
git push -u origin fix/something
gh pr create --fill
gh pr merge --squash --auto     # merges by itself once the checks pass
```

CI runs `checks` (fmt, clippy, unit tests, cargo-deny), then `e2e` (~5 min on
GitHub's runners, most of it Keycloak starting). Batch related changes into one
PR rather than many small ones.

Renovate runs hourly from [`.github/workflows/renovate.yaml`](.github/workflows/renovate.yaml)
and opens grouped update PRs every Monday morning (US Central), and security
fixes immediately. CI tests each one, so merge the green ones. soju and Keycloak
image bumps come as their own PRs: those versions should match production in
chillincool-infra, so update both together. The **Dependency Dashboard** issue
lists everything pending. To run Renovate right away, or as a dry run:
Actions → Renovate → *Run workflow*.

## Repository settings

These are the settings this repository should have, so you know what's enforced
and where to change it.

| Setting | What it does | Where |
| --- | --- | --- |
| `main` ruleset | Changes need a PR; the `checks` and `e2e` checks must pass; CodeQL must report no high or critical alerts; no force-pushes, deletion or merge commits. No bypass, not even for admins. | Settings → Rules → Rulesets |
| `release tags` ruleset | `v*` tags can't be moved, overwritten or deleted. | Settings → Rules → Rulesets |
| Merging | Squash only, with the PR title as the commit message. Auto-merge on; branches deleted after merge. | Settings → General → Pull Requests |
| Private vulnerability reporting | The "Report a vulnerability" button that [SECURITY.md](SECURITY.md) links to. | Settings → Security |
| Secret scanning + push protection | Blocks pushes containing known credential formats, and alerts on any already committed. | Settings → Security |
| Renovate | Runs hourly as this repo's own GitHub App, `chillincool-backroom-renovate` (installed on backroom only). Config: [`.github/renovate.json5`](.github/renovate.json5). | `.github/workflows/renovate.yaml`; app: Org settings → Developer settings → GitHub Apps |
| `renovate` environment | Holds the app's private key (`RENOVATE_APP_PRIVATE_KEY` secret) and client ID (`RENOVATE_APP_CLIENT_ID` variable). Only the `main` branch can use it, so no branch or PR can read the key. | Settings → Environments |
| Dependabot alerts | Alerts on vulnerable dependencies; Renovate reads them and opens the fix PRs. Dependabot's own update and security PRs are off, to avoid duplicates. | Settings → Security |
| CodeQL (default setup, extended queries) | Scans the Rust code, the Python helper and the workflow files on every PR, on every push to `main`, and weekly. | Security → Code scanning |
| Actions: read-only token by default | Workflows must ask for write access explicitly. | Settings → Actions → General |
| Actions: allowed actions | Only GitHub's own actions, verified creators, `jdx/mise-action`, `Swatinem/rust-cache` and `renovatebot/github-action`. Adding a new action means adding it here. | Settings → Actions → General |
| Actions: SHA pinning required | GitHub refuses to run any action not pinned to a full commit SHA; Renovate keeps the pins updated. | Settings → Actions → General |
| Actions: outside contributors | Workflows from outside contributors' PRs wait for your approval before running. | Settings → Actions → General |

Not enabled, because they need a paid GitHub licence: secret scanning for
generic (non-vendor) patterns, and secret validity checks.

## Why the repository is set up this way

| Choice | Reason |
| --- | --- |
| **Rust core, front ends on top** | All IRC, soju and login logic lives in `backroom-core`, with no platform code. The macOS/iOS app (SwiftUI, via UniFFI) and the CLI only supply platform services: opening a browser, storing tokens. Another front end later needs no core changes. |
| **rustls with ring, no OpenSSL** | One TLS stack on every platform, no system library to match, and the same certificate checks everywhere. |
| **End-to-end tests against real soju and Keycloak** | Backroom's hard parts are logging in and talking to soju, so the tests that matter run against the same soju and Keycloak versions as production, over every transport and every login method soju offers (except PAM, which the soju image can't run). The Keycloak realm mirrors production's role gate, so a Keycloak change that would lock users out, like 26.6.2's audience check, fails CI first. |
| **cargo-deny in CI** | A client that holds people's credentials should hear about vulnerable dependencies before its users do. It also keeps every dependency on a permissive licence. |
| **Renovate for updates, GitHub for security alerts** | Renovate runs from this repo as its own narrowly scoped GitHub App, and its key can only be used from `main`. It tracks every pinned version: crates, Actions (kept pinned to SHAs), `mise.toml` tools and the e2e images. Updates arrive grouped, once a week; security fixes come straight away. |
| **Rulesets with no bypass, and SHA-pinned actions** | A solo maintainer is the easiest account to compromise and the likeliest to cut corners at 1am. Requiring PRs and green checks even for the owner means nothing reaches `main` untested. Pinning actions to commits means an upstream tag being moved can't change what runs in CI. |
| **CI runs `mise` tasks** | Each check is defined once, in `mise.toml`. CI calls the same tasks you run locally, with the same tool versions. |
| **MIT licence** | Short and permissive, the same as tilegate. Anyone can use or fork Backroom, as long as the copyright notice stays. |
| **No `CHANGELOG.md`** | Generated release notes come from PR titles. Write clear PR titles and the changelog writes itself. |

## Keycloak test realm

`test/e2e/keycloak/realm.json` is generated: edit
[`build-realm.sh`](test/e2e/keycloak/build-realm.sh), then run
`mise run e2e:realm`. It starts a scratch Keycloak, builds the realm with
Keycloak's admin CLI and exports it. That takes several minutes, but only when
the realm changes; CI just imports the file.

## Setting up Renovate

Renovate runs as a GitHub App that only this repo can use. Do this once, after
the repository exists on GitHub. You need to be an owner of the `chillincool`
organization.

### 1. Create the app

Open <https://github.com/organizations/chillincool/settings/apps/new> and fill in:

| Field | Value |
| --- | --- |
| GitHub App name | `chillincool-backroom-renovate` |
| Homepage URL | `https://github.com/chillincool/backroom` |
| Webhook → Active | **Unchecked**. The workflow polls on a schedule, so nothing needs to receive events. |
| Repository permissions, read and write | Checks, Commit statuses, Contents, Issues, Pull requests, Workflows |
| Repository permissions, read-only | Dependabot alerts (Metadata is set to read-only automatically) |
| Organization and account permissions | None |
| Subscribe to events | None |
| Where can this GitHub App be installed? | **Only on this account** |

Click **Create GitHub App**. These are the same permissions tilegate's app has.
*Workflows* is needed because Renovate updates the pinned action SHAs in
`.github/workflows/`; *Dependabot alerts* lets it open security fixes straight away.

### 2. Note the client ID and create a private key

On the app's page (it opens after creating it):

1. Under **About**, note the **Client ID** (looks like `Iv23li…`). It isn't secret; you'll paste it in step 4.
2. Scroll to **Private keys** → **Generate a private key**. A `.pem` file
   downloads. This is the secret; don't move or copy it anywhere else.

### 3. Install the app on this repo only

On the app's page → **Install App** → `chillincool` → **Only select repositories**
→ `backroom` → **Install**.

### 4. Store the ID and key where only `main` can use them

Create the `renovate` environment, usable only from `main`:

```sh
gh api -X PUT repos/chillincool/backroom/environments/renovate -F 'deployment_branch_policy[protected_branches]=false' -F 'deployment_branch_policy[custom_branch_policies]=true'
```

```sh
gh api -X POST repos/chillincool/backroom/environments/renovate/deployment-branch-policies -f name=main -f type=branch
```

Store the client ID (it prompts; paste the `Iv23li…` value):

```sh
gh variable set RENOVATE_APP_CLIENT_ID --env renovate -R chillincool/backroom
```

Store the private key, then delete the file:

```sh
gh secret set RENOVATE_APP_PRIVATE_KEY --env renovate -R chillincool/backroom < ~/Downloads/chillincool-backroom-renovate.*.pem
```

```sh
rm ~/Downloads/chillincool-backroom-renovate.*.pem
```

### 5. Try it

```sh
gh workflow run renovate.yaml --repo chillincool/backroom -f dryRun=true
```

Then watch it:

```sh
gh run watch --repo chillincool/backroom
```

A dry run logs what Renovate would change without opening anything. If it
passes, run it again without `dryRun`. Because `.github/renovate.json5` is
already in the repo, Renovate skips its onboarding PR; the first real run
opens the **Dependency Dashboard** issue, and update PRs follow on the Monday
schedule (tick a box in the dashboard to get one sooner). Make sure Dependabot
alerts are on (Settings → Security) so security fixes show up.

## Rotating the Renovate app key

1. Org settings → Developer settings → GitHub Apps → `chillincool-backroom-renovate` → **Generate a private key**.
2. `gh secret set RENOVATE_APP_PRIVATE_KEY --env renovate --repo chillincool/backroom < path/to/new-key.pem`
3. Actions → Renovate → *Run workflow* to check it works, then delete the old key on the app page and the `.pem` file from your machine.
