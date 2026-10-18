#!/usr/bin/env bash
# Phase 2 + Phase 3 walkthrough, on a throwaway folder.
#
# Demonstrates, in order:
#   * creating a novel folder (with its reader sentinel)
#   * browsing files in: a plan that changes nothing, then a confirmed copy
#   * an existing file never being overwritten
#   * story records (arcs, chapters, characters)
#   * canon: propose → source → promote, the R3 gate, and R2 on a value change
#   * conflict detection that raises a question instead of resolving it
#   * the knowledge matrix, and the impact report
#
#   bash scripts/phase3-demo.sh

set -euo pipefail

NV="$(cd "$(dirname "$0")/.." && pwd)/target/debug/novelvault"
if [ ! -x "$NV" ]; then
  echo "build first: cargo build -p novelvault-cli" >&2
  exit 1
fi

# Pull an id out of --json output: obj | list | file:<substring>
idof() {
  python3 "$(cd "$(dirname "$0")" && pwd)/jsonid.py" "$1"
}

DEMO=/tmp/nv-phase3-demo
rm -rf "$DEMO"
mkdir -p "$DEMO/Novels" "$DEMO/Desktop/old drafts"

# The author's drafts live on the Desktop, outside the novel folder.
printf 'Azmirah turned the picture frame over in her hands.\n' > "$DEMO/Desktop/Arc 2 Revised v2.md"
printf 'Earlier plan: the keepsake was a picture frame.\n' > "$DEMO/Desktop/old drafts/Old Arc 2 notes.txt"
# A Word lock file, to show the ignore rules at work.
printf 'Old note.\n' > "$DEMO/Desktop/old drafts/"'~$Arc 2.docx'

ROOT="$DEMO/Novels/Saqar"

echo "=============================================================="
echo "1. Create the novel folder"
echo "=============================================================="
"$NV" new "$DEMO/Novels" --name "Saqar" --title "Saqar"

echo
echo "=============================================================="
echo "2. Browse files in — the plan first (changes nothing)"
echo "=============================================================="
"$NV" add "$ROOT" --from "$DEMO/Desktop/Arc 2 Revised v2.md,$DEMO/Desktop/old drafts"
echo
echo "novel folder contents right now:"
ls -1 "$ROOT" | grep -v '^\.novelvault$' | sed 's/^/  /' || echo "  (nothing)"

echo
echo "=============================================================="
echo "3. Confirm the copy"
echo "=============================================================="
"$NV" add "$ROOT" --from "$DEMO/Desktop/Arc 2 Revised v2.md,$DEMO/Desktop/old drafts" --yes
echo
echo "source still on the Desktop?  $(test -f "$DEMO/Desktop/Arc 2 Revised v2.md" && echo yes || echo no)"
echo "novel folder now:"
ls -1 "$ROOT" | grep -v '^\.novelvault$' | sed 's/^/  /'

echo
echo "=============================================================="
echo "4. An existing file is never overwritten"
echo "=============================================================="
printf 'the author version, do not touch\n' > "$ROOT/Arc 2 Revised v2.md"
"$NV" add "$ROOT" --from "$DEMO/Desktop/Arc 2 Revised v2.md" --yes
echo "after bringing the same name in again:"
ls -1 "$ROOT" | grep -v '^\.novelvault$' | sed 's/^/  /'
echo "content of the untouched file: $(cat "$ROOT/Arc 2 Revised v2.md")"

echo
echo "=============================================================="
echo "5. Story records"
echo "=============================================================="
"$NV" scan "$ROOT" | tail -2
"$NV" arc-add "$ROOT" 1 --title "Arc One" --status Developing
"$NV" chapter-add "$ROOT" 1 1 --title "Wooden Bird"
"$NV" character-add "$ROOT" "Azmirah" --role protagonist
echo
"$NV" structure "$ROOT"

echo
echo "=============================================================="
echo "6. Mark one version CURRENT — an explicit act, never automatic"
echo "=============================================================="
echo "The draft brought in above is the one that still says \"picture frame\"."
VID=$("$NV" files "$ROOT" --json | idof "file:Arc 2 Revised v2 (2).md")
"$NV" classify "$ROOT" "$VID" --status CURRENT | head -3

echo
echo "=============================================================="
echo "7. Canon: propose → attach a source → promote"
echo "=============================================================="
CANON=$("$NV" canon-propose "$ROOT" "Keepsake" "picture frame" --category object --json | idof obj)
echo "proposed: $CANON"
"$NV" canon-source "$ROOT" "$CANON" FileVersion "$VID"
"$NV" canon-promote "$ROOT" "$CANON" CANON --reason "author confirmed after review"

echo
echo "=============================================================="
echo "8. R3 — a fact with no source cannot be promoted"
echo "=============================================================="
NOSRC=$("$NV" canon-propose "$ROOT" "Zohan age" "19" --json | idof obj)
if "$NV" canon-promote "$ROOT" "$NOSRC" CANON --reason "author said so"; then
  echo "PROBLEM: this should have been refused"
else
  echo "(refused, as intended)"
fi

echo
echo "=============================================================="
echo "9. R2 — changing the value keeps the old one forever"
echo "=============================================================="
"$NV" canon-change "$ROOT" "$CANON" "wooden bird" --reason "revised: the keepsake is a wooden bird"
echo
"$NV" canon "$ROOT" --all

echo
echo "=============================================================="
echo "10. The conflict detector raises a question, it does not answer it"
echo "=============================================================="
"$NV" probe "$ROOT"
echo
"$NV" conflicts "$ROOT"
CONFLICT=$("$NV" conflicts "$ROOT" --json | idof obj)

echo
echo "=============================================================="
echo "11. Resolving is a human act with a written reason"
echo "=============================================================="
"$NV" resolve-conflict "$ROOT" "$CONFLICT" keep_current --reason "the draft is outdated"
"$NV" conflicts "$ROOT"
echo "canon is unchanged — the wooden bird is still the current truth:"
"$NV" canon "$ROOT"

echo
echo "=============================================================="
echo "12. Knowledge matrix (R4)"
echo "=============================================================="
"$NV" knowledge-set "$ROOT" "Azmirah" "$CANON" KNOWS
"$NV" knowledge "$ROOT"

echo
echo "=============================================================="
echo "13. Impact of changing this fact"
echo "=============================================================="
"$NV" canon-impact "$ROOT" "$CANON"

echo
echo "=============================================================="
echo "14. History — append-only"
echo "=============================================================="
"$NV" canon-history "$ROOT" "$CANON"

echo
echo "Demo finished. The vault is at $ROOT/.novelvault/vault.db"
