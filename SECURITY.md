# Security Policy

## Reporting a vulnerability

**Please do not file a public GitHub issue for security bugs.**

Open a **private** advisory at https://github.com/hawkyin-hub/quarkdrive-webdav-rust/security/advisories/new. Include reproduction steps and a server log snippet (omit Cookie values).

We aim to acknowledge within 72 hours.

## Supported versions

Only the most recent minor version receives security fixes. We do not backport.

## Threat model

This tool is **single-user, single-machine**. The WebDAV listener and admin panel bind to `127.0.0.1` only. The privileged Helper reads browser Cookies with macOS Authorization Services consent. See [docs/security.md](docs/security.md) for full details.

## Cookie handling

Cookies are read on demand, held in memory, and sent only to `pan.quark.cn` over HTTPS with rustls validation. They are never written to disk in plaintext.

If your Cookie leaks, sign out of Quark Drive in your browser to invalidate it; the app picks up the new Cookie on next launch.
