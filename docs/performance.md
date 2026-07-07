# Performance notes

These are observations from real-world runs, not benchmarks. Numbers vary with Quark Drive's current throttling (free vs SVIP), local network, and disk. Take them as order-of-magnitude.

## Upload throughput

**Single large file (≥ 100 MB):** throughput is dominated by Quark Drive's per-file upload slot allocation. We do **sequential chunked upload** (`part_thread:1`) because Quark rejects parallel uploads to the same task.

Empirically:
- Quark free: 1–3 MB/s steady
- Quark SVIP: 8–15 MB/s steady

The server itself is not the bottleneck — `tokio::spawn` async chunks keep CPU idle above 5% and memory under 50 MB per active transfer.

**Many small files (drag-drop of a folder):** the `dir_cache` keeps PROPFINDs warm; only the first PROPFIND per directory hits Quark. Upload still serializes through Quark's per-file limits, so expect the total time = sum(individual file times).

## Download throughput

Streamed via HTTPS to the user's Quark Storage CDN. Reused `download_url` for 60s (after which we re-fetch a signed URL), avoiding Quark's signature-refresh overhead.

Hit-rates observed:
- Repeated directory listings: `dir_cache` saves ~95% of Quark PROPFIND calls.
- Repeated file reads: chunk cache saves ~80% of Quark download calls, capped at 100 MB per file.

## Latency budget per request

| Step | Typical |
|------|---------|
| TLS handshake (resume) | 5 ms |
| Proxy → backend | < 1 ms (loopback) |
| PROPFIND (cache hit) | < 5 ms |
| PROPFIND (cache miss → Quark) | 200–600 ms |
| Read chunk (4–16 MB) | depends on Quark CDN |
| Write chunk | depends on Quark |

## Tuning knobs

Most of these are clap flags; some are environment variables.

| Flag | Default | What |
|------|---------|------|
| `--upload-wait-timeout <s>` | 0 (wait indefinitely) | Maximum wait for `do_flush` to return; set lower if you want Finder to free up faster. |
| `--skip-upload-same-size` | false | Skip upload if same size as cloud copy (assumes same bytes — risky if content differs). |
| `--prefer-http-download` | false | Use signed HTTP(S) redirect to Quark CDN instead of proxy-streaming. |
| `--read-ahead-chunk-size <bytes>` | 4 MiB | Read-ahead for downloads. Smaller = less memory, more round-trips. |

For very slow links, lower `--read-ahead-chunk-size` to 1 MiB.

## Memory footprint

Roughly:
- Idle: ~15 MB (Tokio runtime, tokio-rustls, dav-server, moka cache).
- Per active upload: ~5 MB + chunk buffer (≈ chunk size, max 32 MB by Quark).
- Per active download: ~5 MB + chunk cache.

For a heavy workload (drag-drop of 100 files simultaneously), expect ~300 MB peak. Most desktop sessions don't hit that.

## Disk cache

- `~/Library/Caches/LocalQuark/` — content chunks (LRU size-capped) + tmp files.
- Tmp files cleaned after upload commit; restart-safe.
- Chunk cache persists across restarts; can be cleared via `POST /api/cache/clear` (see [api.md](api.md)).
