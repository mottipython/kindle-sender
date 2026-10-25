# Security policy

## Reporting a vulnerability

Please report security problems privately through GitHub:
**Security tab → Report a vulnerability** (private vulnerability reporting is
enabled). Please do not open a public issue for something exploitable.

You can expect an acknowledgement within a week.

## Supported versions

Only the latest release receives fixes.

## How the project is checked

Ebook files are untrusted input, so the parsers are tested accordingly. These
run automatically on GitHub (see the badges in the README):

| Check | When | Workflow |
| --- | --- | --- |
| Build, tests, `clippy -D warnings`, `rustfmt` | every push and pull request | [CI](.github/workflows/ci.yml) |
| `cargo audit` against the RustSec advisory database | every push, every pull request, and weekly | [Security](.github/workflows/security.yml) |
| Fuzzing of every parser with AddressSanitizer (EPUB, MOBI/AZW/AZW3, FB2, PDF, TXT, HTML, metadata) | weekly and on demand | [Security](.github/workflows/security.yml) |
| Dependency update pull requests | weekly | [Dependabot](.github/dependabot.yml) |
| Secret scanning with push protection | always on | GitHub |

Snyk Code static analysis is also run locally before releases (0 issues at
v1.1.0).

## Design notes

- Nothing inside a book is executed; markup is rebuilt from an allow-list of
  tags and attributes, and scripts and stylesheets are dropped.
- Archives are read in memory, never extracted to disk, so path-traversal and
  symlink tricks have nothing to act on.
- XML is parsed with DTDs disabled; nesting depth and PDF page failures are
  contained.
- DRM-protected books are rejected, not bypassed.
- Mail is sent over TLS only; the SMTP password is kept in the system wallet
  when one is available.
