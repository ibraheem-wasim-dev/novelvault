#!/usr/bin/env bash
# Phase 5 walkthrough AND smoke test: the Reader learns a novel file by file.
#
#   1. Chapter 1 arrives            -> the Reader learns the cast
#   2. Chapter 2 arrives            -> only the new file is read; a name learned in
#                                      chapter 1 is recognised in chapter 2
#   3. An old draft disagrees       -> noticed, never decided; the author's files are untouched
#
# Exits non-zero if any expectation fails, so CI can run it:
#   bash scripts/phase5-demo.sh
set -euo pipefail

NV="$(cd "$(dirname "$0")/.." && pwd)/target/debug/novelvault"
[ -x "$NV" ] || { echo "build first: cargo build -p novelvault-cli" >&2; exit 1; }
ID="python3 $(cd "$(dirname "$0")" && pwd)/jsonid.py"

DEMO="$(mktemp -d)"
trap 'rm -rf "$DEMO"' EXIT
mkdir -p "$DEMO/Novels" "$DEMO/Desktop"
ROOT="$DEMO/Novels/Saqar"

fail() { echo "FAIL: $*" >&2; exit 1; }
expect() { grep -q -- "$2" <<<"$1" || fail "expected to find: $2"; }

printf 'Azmirah kept the wooden bird on her windowsill. It had been her brother'"'"'s.\n\nLucifer watched Azmirah from the doorway. Nobody knew why Lucifer had come to Zanzibar.\n\nShe promised Maheen that she would return before winter. Maheen believed Azmirah, and Lucifer said nothing.\n\nAzmirah walked to the harbour with Maheen. Later Azmirah told Maheen the secret of the bird.\n' > "$DEMO/Desktop/Chapter 1.md"
printf 'Maheen, afraid and cold, sat on the harbour wall. Azmirah found a letter in the sand.\n\nLucifer killed the guard and escaped from Zanzibar. Azmirah never saw Lucifer again.\n\nAzmirah lived in Zanzibar. Maheen left the harbour at dawn.\n' > "$DEMO/Desktop/Chapter 2.md"
printf 'Azmirah kept the picture frame on her windowsill. It had been her brother'"'"'s.\n' > "$DEMO/Desktop/Old Chapter 1.md"

"$NV" new "$DEMO/Novels" --name Saqar --title Saqar >/dev/null

echo "== 1. Chapter 1 arrives =============================================="
"$NV" add "$ROOT" --from "$DEMO/Desktop/Chapter 1.md" --yes >/dev/null
C1=$("$NV" files "$ROOT" --json | $ID "file:Chapter 1")
"$NV" classify "$ROOT" "$C1" --status WORKING_DRAFT >/dev/null
OUT=$("$NV" read "$ROOT"); echo "$OUT"
expect "$OUT" "1 file(s) read"
expect "$OUT" "Azmirah"

echo; echo "== 2. Chapter 2 arrives — only the new file is read ==================="
"$NV" add "$ROOT" --from "$DEMO/Desktop/Chapter 2.md" --yes >/dev/null
C2=$("$NV" files "$ROOT" --json | $ID "file:Chapter 2")
"$NV" classify "$ROOT" "$C2" --status WORKING_DRAFT >/dev/null
OUT=$("$NV" read "$ROOT"); echo "$OUT"
expect "$OUT" "1 file(s) read"
OUT=$("$NV" read "$ROOT"); expect "$OUT" "0 file(s) read"
echo "(a second run reads nothing — nothing is ever re-read)"
OUT=$("$NV" brain-entity "$ROOT" Maheen); echo "$OUT"
expect "$OUT" "Maheen"
BRAIN=$("$NV" brain "$ROOT"); echo "$BRAIN"
expect "$BRAIN" "Zanzibar"
expect "$BRAIN" "Nobody knew"
OUT=$("$NV" brain-entity "$ROOT" Azmirah)
expect "$OUT" "keeps"
expect "$OUT" "lives in"

echo; echo "== 3. An old draft disagrees — noticed, not decided ===================="
before=$(sha256sum "$DEMO"/Novels/Saqar/*.md | sort)
"$NV" add "$ROOT" --from "$DEMO/Desktop/Old Chapter 1.md" --yes >/dev/null
OLD=$("$NV" files "$ROOT" --json | $ID "file:Old Chapter")
"$NV" classify "$ROOT" "$OLD" --status HISTORICAL >/dev/null
OUT=$("$NV" read "$ROOT"); echo "$OUT"
BRAIN=$("$NV" brain "$ROOT")
expect "$BRAIN" "picture frame"
expect "$BRAIN" "revision"
NOTICES=$("$NV" ai-notice "$ROOT"); expect "$NOTICES" "story_change"
echo "(the notice waits for the author; no canon was created)"
after=$(sha256sum "$DEMO"/Novels/Saqar/Chapter*.md | sort)
grep -q . <<<"$after" || fail "no chapter files found"
[ "$(grep -c 'Chapter' <<<"$before")" = "$(grep -c 'Chapter' <<<"$after")" ] || fail "file list changed"
diff <(grep 'Chapter [12]' <<<"$before") <(grep 'Chapter [12]' <<<"$after") >/dev/null || fail "a draft was modified"

echo; echo "Phase 5 demo passed: the Reader learned the novel file by file and changed nothing."
