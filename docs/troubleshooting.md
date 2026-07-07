# Troubleshooting

## Quick diagnostic

```bash
# Server alive?
curl -k -s --max-time 3 http://127.0.0.1:8444/api/status

# Mount alive?
mount | grep -i localquark

# Recent error log
tail -50 ~/Library/Logs/LocalQuark-rust-webdav.log | grep -vE 'rustls::msgs::handshake'
```

Admin panel: <http://127.0.0.1:8444/>

## Common problems

### 1. Drag-drop creates a `0-byte` original + `MyFile(1).txt`

**Symptom**: drag a file in; Finder shows two — a 0-byte stub and the real file with `(1)` suffix.

**Cause**: macOS `webdavfs_agent` does a 0-byte PUT to "prepare" the target vnode before the body PUT. As of v1.3.10+, `do_flush` recognizes this and skips `up_pre`, so 0-byte files are no longer persisted on the cloud. If you see this on an older version, update.

**Cleanup**: drag both files to Trash, the cloud copy will follow.

### 2. Mount volume shows empty / Finder says "in use"

**Symptom**: `/Volumes/LocalQuark` appears but is unreadable or empty.

**Cause (most likely)**: stale `webdavfs_agent` vnode cache after a server restart, OR the server is bound to a different mount point.

**Fix**:
```bash
diskutil unmount force /Volumes/LocalQuark
killall -9 webdavfs_agent
/bin/launchctl asuser $(id -u) /sbin/mount_webdav -s -v LocalQuark https://127.0.0.1:8443/ /Volumes/LocalQuark
```

### 3. Server crash loop / restarts every few seconds

`tail -100 ~/Library/Logs/LocalQuark-rust-webdav.log` — look for `panicked` lines in the first 30 lines.

Likely causes: port 8443 already in use (another app bound loopback on it), or Cookie expired.

### 4. Helper complains "no Quark cookie found in browser"

Your browser is not signed in to `pan.quark.cn`. Visit <https://pan.quark.cn> in Chrome / Brave / Arc / Edge and try again.

### 5. `mount: Operation not supported` or similar

The Helper installs Quark-only; on Linux this won't work because `mount_webdav` is macOS-specific. See `docker/Dockerfile` for the Linux build path (limited — Finder integration impossible without macOS).

### 6. Quota / speed-limit errors from Quark

Quark Drive enforces per-account throttling (see Quark's terms). QuarkDrive-WebDAV cannot bypass this. If you hit speed limits, wait a few minutes or upgrade your Quark account.

### 7. "This file server will not allow any additional users"

macOS Finder's per-volume concurrent user limit. See [docs/deployment.md](deployment.md#multi-user-caveat).

## Diagnostic command cookbook

```bash
# Count active webdav connections
lsof -nP -iTCP:8443 -sTCP:LISTEN
lsof -nP -iTCP:8080 -sTCP:LISTEN

# Live tail of all requests
RUST_LOG=quarkdrive_webdav=debug,reqwest=warn ./target/release/quarkdrive-webdav --serve-only ...

# Admin reprobe
curl -X POST http://127.0.0.1:8444/api/repair

# Cookie refresh
curl -X POST http://127.0.0.1:8444/api/refresh
```

## Still stuck?

- Search [issues](https://github.com/hawkyin-hub/quarkdrive-webdav-rust/issues)
- Open a new issue with **full** log snippets (omit cookies!)
