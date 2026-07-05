#!/usr/bin/env bash
# Show Claude-related env vars (no secrets logged if you pass --redact)
set -euo pipefail
LOG="scripts/_pending/show-claude-env-20260703-$(date +%H%M%S).log"

{
  echo "=== ANTHROPIC_* env vars ==="
  env | grep -i '^ANTHROPIC' | sort || echo "(none)"
  echo
  echo "=== Claude Code related ==="
  env | grep -iE 'CLAUDE|MODEL|API_KEY|BASE_URL' | grep -v PATH | sort || echo "(none)"
  echo
  echo "=== rtak / rtk / claude binary locations ==="
  which claude rtk 2>/dev/null || true
  echo
  echo "=== ~/.claude/settings.json (top-level keys only, values redacted) ==="
  if [ -f ~/.claude/settings.json ]; then
    python3 -c "
import json,sys
with open('$HOME/.claude/settings.json') as f:
    d=json.load(f)
def shape(o,depth=0):
    if depth>2: return '...'
    if isinstance(o,dict): return {k: shape(v,depth+1) for k,v in o.items()}
    if isinstance(o,list): return f'[list len={len(o)}]'
    if isinstance(o,str) and len(o)>20: return '<redacted-str>'
    return o
print(json.dumps(shape(d), indent=2, ensure_ascii=False))
" 2>/dev/null || cat ~/.claude/settings.json
  else
    echo "(not found)"
  fi
} | tee "$LOG"
echo
echo "Wrote: $LOG"
