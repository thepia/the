# Security Policy

`thepia` (`the`) is a local developer tool that nurtures and protects your local AI assistant usage on your
machine. It runs offline with no network calls and no telemetry, but it does read files you point it
at (`SKILL.md`, bundled scripts, symlinks), so input-handling bugs are in scope.

## Reporting a vulnerability

**Please do not open a public issue for security reports.** Use either:

1. **GitHub private vulnerability reporting** (preferred): the repository's **Security** tab ->
   **Report a vulnerability**. This opens a private advisory visible only to the maintainers.
2. **Email**: [info@thepia.net](mailto:info@thepia.net) with `SECURITY` in the subject.

Please include: the version or commit (`thepia --version`), your OS, a minimal reproduction (for example
the offending `SKILL.md` or directory layout), and the impact you observed.

We aim to acknowledge a report within **3 business days** and to agree a disclosure timeline with you.
We practice coordinated disclosure: we will credit you (unless you prefer otherwise) and publish a
fix and advisory together.

## Supported versions

Pre-1.0, only the **latest release** (and `main`) receives security fixes. Once `1.0` ships this
section will list the supported release lines.

| Version | Supported |
|---|---|
| latest release / `main` | yes |
| older | no |

## Scope

In scope: the `the` / `thepia` / `th` CLI and its skill parsing, resolution, link, and smoke checks
(for example a crafted `SKILL.md`, symlink, or bundled file that causes a panic, an escape outside the
skill directory, or a missed security-smoke hazard).

Out of scope: the deep security audit delegated to third-party scanners (`the skills audit` ->
`skill-lint`), vulnerabilities in dependencies themselves (report those upstream; we track them via
dependency updates), and issues requiring a already-compromised local machine.
