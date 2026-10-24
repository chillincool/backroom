# Security policy

Backroom handles your IRC credentials: OAuth tokens and soju passwords. Security
reports are very welcome.

## Reporting a vulnerability

Please **don't open a public issue**. Report it privately through
[GitHub security advisories](https://github.com/chillincool/backroom/security/advisories/new).
Include the version or commit, how you connect (WebSocket or TCP/TLS, and the
login method), and steps to reproduce.

You should get a reply within a week. Once a fix is released, the advisory is
published and you're credited unless you'd rather not be.

## Supported versions

Only the latest release gets security fixes. Until the first release, that
means the `main` branch.

## Scope

In scope:

- tokens, passwords or other secrets written to logs, error messages or disk
  (other than the CLI's documented, dev-only token file);
- TLS certificate checks being skipped or weakened;
- the login flow accepting a response it shouldn't (state/PKCE mistakes);
- a malicious server or network crashing the client, or controlling the
  terminal or UI through message text.

Out of scope: vulnerabilities in soju, Keycloak or other servers themselves.
Please report those to their projects.
