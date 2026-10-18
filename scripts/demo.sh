#!/usr/bin/env bash
# End-to-end walkthrough on a throwaway folder. Nothing here touches real work.
set -uo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
export CARGO_HOME="${CARGO_HOME:-$HOME/.cargo}"
export PATH="$HOME/.cargo/bin:$PATH"

DEMO="${DEMO_DIR:-/tmp/novelvault-demo}"
NV="$ROOT/target/debug/novelvault"

echo "== building =="
( cd "$ROOT" && cargo build -p novelvault-cli -q ) || exit 1

rm -rf "$DEMO" && mkdir -p "$DEMO"
echo
echo "== a novel folder as you actually keep it =="
printf 'Arc one body text.\n' > "$DEMO/Arc 1-2 v1 snippet.txt"
printf 'Azmirah kept the wooden bird on the shelf.\n' > "$DEMO/Arc 2 Revised.txt"
printf 'chapter with the picture frame in it\n' > "$DEMO/old Arc 2.txt"
printf '# development chat\nIdeas about Arc 3.\n' > "$DEMO/story_development_chat.md"
ls -1 "$DEMO"

echo
echo "== init + scan =="
"$NV" init "$DEMO" --title "Saqar"
"$NV" scan "$DEMO"

echo
echo "== import defaults to UNKNOWN; nothing is current =="
"$NV" files "$DEMO"

echo
echo "== the Inbox is the only door =="
"$NV" inbox "$DEMO" | head -30

echo
echo "== classify one file (as an author would) =="
VERSION_ID=$("$NV" files "$DEMO" --json | python3 -c 'import sys,json;print(json.load(sys.stdin)[0]["id"])')
ITEM_ID=$("$NV" inbox "$DEMO" --json | python3 -c 'import sys,json;d=json.load(sys.stdin);print([i for i in d if i["kind"]=="NEW_FILE"][0]["id"])')
"$NV" resolve "$DEMO" "$ITEM_ID" classify --status WORKING_DRAFT --arc "Arc 2" --chapter "Chapter 5"

echo
echo "== search across current and archived text =="
"$NV" search "$DEMO" "wooden bird"
"$NV" search "$DEMO" "picture frame"

echo
echo "== archive keeps it searchable and marks it =="
"$NV" archive "$DEMO" "$VERSION_ID"
"$NV" search "$DEMO" "Arc one body" | head -6

echo
echo "== a save becomes a new version; the old one stays =="
printf 'Azmirah kept the wooden bird on the shelf, and the room was quiet.\n' > "$DEMO/Arc 2 Revised.txt"
"$NV" scan "$DEMO" | head -12
"$NV" files "$DEMO" --all

echo
echo "== snapshot, then prove your files never changed =="
"$NV" snapshot "$DEMO" --name "Before Arc 3 Revision"
find "$DEMO" -type f -not -path "*.novelvault*" -exec sha256sum {} \; | sort > /tmp/demo-hashes.txt
"$NV" scan "$DEMO" >/dev/null
sha256sum -c /tmp/demo-hashes.txt && echo "OK: every source file is byte-identical after a scan"

echo
echo "== everything the app wrote lives under .novelvault/ =="
du -sh "$DEMO/.novelvault" && find "$DEMO/.novelvault" -maxdepth 2 -type d | sort
