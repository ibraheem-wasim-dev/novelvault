#!/usr/bin/env bash
# Phase 4 walkthrough: the local AI librarian, on a throwaway folder.
#
#   1. Offline librarian (no model at all): ranked, sourced answers
#   2. Canon Shield: a changed canon value is announced as history
#   3. Notices: an unrecorded name is suggested, nothing is created
#   4. A remote AI address is refused
#   5. A (pretend) local model answers from the numbered sources
#
#   bash scripts/phase4-demo.sh
set -euo pipefail

NV="$(cd "$(dirname "$0")/.." && pwd)/target/debug/novelvault"
[ -x "$NV" ] || { echo "build first: cargo build -p novelvault-cli" >&2; exit 1; }
ID="python3 $(cd "$(dirname "$0")" && pwd)/jsonid.py"

DEMO=/tmp/nv-phase4-demo
rm -rf "$DEMO"; mkdir -p "$DEMO/Novels" "$DEMO/Desktop"
printf 'Azmirah met Maheen at the gate. She asked Maheen about the river.\n\nAzmirah kept the wooden bird on her windowsill. Later, Azmirah told Maheen everything. Maheen only nodded.\n' > "$DEMO/Desktop/Arc 2 Revised v2.md"
printf 'Azmirah kept the picture frame on her windowsill. It had been her brother'"'"'s.\n' > "$DEMO/Desktop/Old Arc 2.md"
ROOT="$DEMO/Novels/Saqar"

"$NV" new "$DEMO/Novels" --name Saqar --title Saqar >/dev/null
"$NV" add "$ROOT" --from "$DEMO/Desktop/Arc 2 Revised v2.md,$DEMO/Desktop/Old Arc 2.md" --yes >/dev/null
CUR=$("$NV" files "$ROOT" --json | $ID file:Revised)
OLD=$("$NV" files "$ROOT" --json | $ID file:Old)
"$NV" classify "$ROOT" "$CUR" --status CURRENT >/dev/null
"$NV" classify "$ROOT" "$OLD" --status HISTORICAL >/dev/null
"$NV" character-add "$ROOT" Azmirah --role protagonist >/dev/null
before=$(sha256sum "$DEMO"/Novels/Saqar/*.md | sort)

echo "=============================================================="
echo "1. Offline librarian — no model connected"
echo "=============================================================="
"$NV" ai-index "$ROOT"
"$NV" ai-ask "$ROOT" "What is on Azmirah's windowsill?"

echo; echo "=============================================================="
echo "2. Canon Shield — the keepsake changes; the old value becomes history"
echo "=============================================================="
C=$("$NV" canon-propose "$ROOT" Keepsake "picture frame" --category object --json | $ID obj)
"$NV" canon-source "$ROOT" "$C" FileVersion "$CUR" >/dev/null
"$NV" canon-promote "$ROOT" "$C" CANON --reason "author confirmed" >/dev/null
"$NV" canon-change "$ROOT" "$C" "wooden bird" --reason "revised: it is a wooden bird" >/dev/null
"$NV" ai-ask "$ROOT" "What is the keepsake?"

echo; echo "=============================================================="
echo "3. Notices — the librarian notices Maheen; it does NOT add her"
echo "=============================================================="
"$NV" ai-notice "$ROOT"
echo "characters recorded now:"; "$NV" structure "$ROOT" | grep -i -A5 character || true

echo; echo "=============================================================="
echo "4. A remote AI address is refused"
echo "=============================================================="
"$NV" ai-setup "$ROOT" ollama --host api.example.com --model x 2>&1 | head -3 || true

echo; echo "=============================================================="
echo "5. A pretend local model answers from the numbered sources"
echo "=============================================================="
python3 - "$ROOT" "$NV" <<'PY'
import http.server, json, subprocess, sys, threading
root, nv = sys.argv[1], sys.argv[2]
seen = []
class H(http.server.BaseHTTPRequestHandler):
    def log_message(self, *a): pass
    def do_GET(self):
        b = json.dumps({"models": [{"name": "demo-model"}]}).encode()
        self.send_response(200); self.send_header("Content-Length", str(len(b))); self.end_headers(); self.wfile.write(b)
    def do_POST(self):
        body = json.loads(self.rfile.read(int(self.headers["Content-Length"])))
        seen.append(body)
        b = json.dumps({"message": {"role": "assistant", "content": "The keepsake is a wooden bird [S1]; an earlier value was a picture frame [S2]."}}).encode()
        self.send_response(200); self.send_header("Content-Length", str(len(b))); self.end_headers(); self.wfile.write(b)
srv = http.server.HTTPServer(("127.0.0.1", 0), H)
threading.Thread(target=srv.serve_forever, daemon=True).start()
port = srv.server_address[1]
print(subprocess.run([nv, "ai-setup", root, "ollama", "--model", "demo-model", "--port", str(port)], capture_output=True, text=True).stdout.strip())
print(subprocess.run([nv, "ai-ask", root, "What is the keepsake?"], capture_output=True, text=True).stdout)
print("What the model was actually sent (first 420 chars of the user message):")
print("  " + seen[-1]["messages"][-1]["content"][:420].replace("\n", "\n  "))
PY

echo; echo "=============================================================="
echo "Your files after all of that"
echo "=============================================================="
after=$(sha256sum "$DEMO"/Novels/Saqar/*.md | sort)
[ "$before" = "$after" ] && echo "unchanged — every draft is byte-for-byte identical." || { echo "CHANGED (bug!)"; exit 1; }
