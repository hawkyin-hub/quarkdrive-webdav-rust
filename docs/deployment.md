# Deployment

This is a **single-user, single-machine** tool by design. The model is:

> One macOS user owns the process. The process listens on loopback only. macOS `webdavfs_agent` connects to it. The user accesses the mounted volume via Finder.

For anything beyond that — multiple users, remote access, headless servers — read the [§Multi-user caveat](#multi-user-caveat) before going further.

## Run modes

| Mode | Flag | What |
|------|------|------|
| Foreground (default) | — | server runs in current TTY |
| Background (auto-detach) | `--serve-only` | runs detached, parent exits |
| Health check | `quarkdrive-webdav health` | prints status, exits |

### Foreground

```bash
./quarkdrive-webdav --serve-only --host 127.0.0.1 --port 8443 --quark-cookie "..."
```

### Background

A user launchd plist is recommended for automatic restart. Example:

`~/Library/LaunchAgents/com.localquark.webdav.plist`:
```xml
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
  <dict>
    <key>Label</key><string>com.localquark.webdav</string>
    <key>ProgramArguments</key>
    <array>
      <string>/Applications/LocalQuark-rust.app/Contents/Resources/bin/quarkdrive-webdav</string>
      <string>--serve-only</string>
      <string>--host</string><string>127.0.0.1</string>
      <string>--port</string><string>8443</string>
    </array>
    <key>RunAtLoad</key><true/>
    <key>KeepAlive</key><true/>
    <key>ThrottleInterval</key><integer>10</integer>
    <key>StandardOutPath</key><string>/tmp/localquark.out.log</string>
    <key>StandardErrorPath</key><string>/tmp/localquark.err.log</string>
  </dict>
</plist>
```

```bash
launchctl load -w ~/Library/LaunchAgents/com.localquark.webdav.plist
```

> macOS webdavfs_agent requires a **GUI session** (Aqua) to mount. The LaunchAgent above runs in your Aqua session automatically because `RunAtLoad` is true.

## Cooked-in defaults

| Setting | Default | Override |
|---------|---------|----------|
| Host | `127.0.0.1` | `--host` |
| Port | `8443` | `--port` |
| Backend port | `8080` | `--backend-port` |
| Cookie source | Browser (auto) | `--quark-cookie "<cookie-string>"` |
| Cert dir | `~/Library/Application Support/LocalQuark/certs/` | `--tls-cert`, `--tls-key` |
| Mount point | `/Volumes/LocalQuark` | `--mount-point` (admin / CLI only) |

## Health checks

```bash
curl -s http://127.0.0.1:8444/api/status
# {"cookies_ok":true,"healthy":true,"level":"Healthy","mount_point":"/Volumes/LocalQuark","mounted_ok":true,"webdav_ok":true,"webdav_url":"https://127.0.0.1:8443"}
```

`healthy=true` is the only thing your monitoring needs.

## Auto-recovery

The server runs an internal `mount_supervisor_loop` that polls every 3 seconds:

1. Checks if `/Volumes/LocalQuark` is mounted.
2. If lost, sleeps 2 seconds (gives Finder time to release vnodes).
3. Re-checks; if still gone, asks the Helper to remount.

User-visible notification appears via `osascript display notification` when a recovery happens.

## Multi-user caveat

macOS Finder's webdavfs allows **at most one user per mount**. Multiple Finder sessions on the same machine pointing at the same server will see the second one rejected with:

> This file server will not allow any additional users to log on.

The supported path is one-user-one-machine. Headless / multi-user setups are out of scope — open an issue if you need them.

## Production notes

- Cookie refresh: every 30 days. The Helper triggers macOS Authorization each refresh.
- Log rotation: rotate `~/Library/Logs/LocalQuark-rust-*.log` monthly.
- Disk usage: the chunk cache (`~/Library/Caches/LocalQuark/`) auto-evicts by size; monitor if you have an unusual workload.
