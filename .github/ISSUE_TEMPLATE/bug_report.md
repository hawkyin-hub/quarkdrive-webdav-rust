---
name: Bug report
about: Reproduce a problem with QuarkDrive-WebDAV
title: "[bug] "
labels: bug
---

## What happened

<!-- Short, factual description of the symptom (1–3 sentences). -->

## Steps to reproduce

1.
2.
3.

## Expected

<!-- What you expected to happen. -->

## Actual

<!-- What actually happened, including any error messages / screenshots. -->

## Environment

- macOS version:
- App version (run `curl -s http://127.0.0.1:8444/api/status`):
- Browser used for cookies (Chrome, Brave, etc.):
- Quark account type (free / SVIP):

## Logs

```bash
tail -100 ~/Library/Logs/LocalQuark-rust-webdav.log | grep -v 'rustls::msgs::handshake'
```

Paste relevant lines here. **Strip Cookie values** before pasting.

```text
PASTE HERE
```
