# Admin HTTP API

Admin panel listens on `127.0.0.1:8444` (loopback only). All JSON responses are plain UTF-8, no auth (the loopback-only assumption holds; see [security.md](security.md)).

## Endpoints

| Method | Path | Purpose |
|--------|------|---------|
| GET | `/` | HTML admin panel |
| GET | `/api/status` | Health snapshot |
| POST | `/api/repair` | Force a re-check (cookie + webdav + mount) |
| POST | `/api/refresh` | Re-read cookies from browser |
| GET | `/api/cookies` | List Cookie map (key names only; values redacted) |
| GET | `/api/cache/info` | Size and file count of disk cache |
| POST | `/api/cache/clear` | Wipe chunk cache and tmp files |
| GET | `/api/logs/tail?n=100` | Tail the server log |

### GET /api/status

```json
{
  "cookies_ok": true,
  "healthy": true,
  "level": "Healthy",
  "mount_point": "/Volumes/LocalQuark",
  "mounted_ok": true,
  "webdav_ok": true,
  "webdav_url": "https://127.0.0.1:8443"
}
```

Fields:
- `level`: enum `Healthy` | `Degraded` | `Unhealthy`. Anything other than `Healthy` needs attention.
- `cookies_ok`: cookie map is non-empty AND at least one expected key is present (`sl-session` / `__pus` / `__kp` / `__uid` / `isQuark` / `grey-id` / `ctoken`).
- `mounted_ok`: `/Volumes/LocalQuark` is in mount table.
- `webdav_ok`: backend port is listening.

### POST /api/repair

Triggers a re-check (cookie → webdav → mount). Idempotent. Returns the new `/api/status` snapshot.

### POST /api/refresh

Re-reads cookies from system browser (calls Helper). Useful after manual sign-in.

### GET /api/cache/info

```json
{ "bytes": 12345678, "files": 42, "cache_dir": "~/Library/Caches/LocalQuark" }
```

### POST /api/cache/clear

Best-effort purge of disk chunk cache and tmp files. Active uploads keep their temp file until commit completes.

### GET /api/logs/tail?n=100

Returns the last `n` lines of the server log as plain text. `n` defaults to 100, max 5000.

## Why no auth?

The admin endpoint binds to `127.0.0.1` and shares the user's privilege. Anything local to that user can already read the user's files, so adding a token buys nothing but friction. **Do not** expose this port to the LAN.

If you must accept that risk, run behind a reverse proxy with HTTP basic auth + TLS, and forward only `/` and `/api/*`.
