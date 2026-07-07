# Security

## Threat model in one paragraph

This repo is a **personal-use WebDAV proxy**. It runs as a process owned by the user, exposes only loopback ports (no LAN bind), and speaks to `pan.quark.cn` with the user's own Cookie. The trust boundary is the user's macOS user account. Anything running as that user can talk to the WebDAV server with the user's cookies.

## Cookie handling

- Cookies are read from the user's local browser via a privileged Helper installed under `/Library/LaunchDaemons/` (or `com.localquark.webdav-helper` LaunchAgent).
- The Helper prompts the user via macOS Authorization Services; consent is one-time per install.
- Cookies never leave the loopback WebDAV server except to `pan.quark.cn` over HTTPS with rustls validation enabled.
- Cookie value is held in memory only; we do not write it to disk in plaintext.
- `--quark-cookie` flag bypasses the Helper (for dev / CI). Treat that flag's value as a credential; do not paste it into logs or issues.

If you believe your Cookie is compromised, rotate it by signing out of Quark Drive in your browser, then signing back in. The app picks up the new Cookie on next launch (or via admin panel refresh).

## TLS

The WebDAV listener uses a self-signed cert generated on first launch and installed into the macOS System Keychain via `setup-tls.sh`. macOS `webdavfs_agent` then trusts it without prompts.

To rotate: delete `~/Library/Application Support/LocalQuark/certs/*` and `xcrun security delete-certificate -c "LocalQuark"`, then relaunch the app.

## Network exposure

By default:
- WebDAV listens on `127.0.0.1:8443` only. Not reachable from the LAN.
- Admin panel listens on `127.0.0.1:8444`. Bound to loopback.
- The Helper, when invoked, speaks to the same loopback ports.

**Do not** expose the WebDAV port to the LAN unless you have read [docs/deployment.md](deployment.md) and understand the implications (no auth beyond Basic, single-user trust model).

## Reporting vulnerabilities

Please open a **private** security advisory at https://github.com/hawkyin-hub/quarkdrive-webdav-rust/security/advisories/new instead of filing a public GitHub issue.

Include:
- reproduction steps
- server log snippet (omit Cookie values)
- Quark API behavior trace if available

We aim to acknowledge within 72 hours.
