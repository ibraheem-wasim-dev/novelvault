# NovelVault Desktop

A desktop command centre for a novel that already exists as files. It watches your novel
folder, keeps every version you have ever saved, and holds the canon you have decided on
— without ever rewriting one of your drafts.

The novel folder is the source of truth. Everything the app knows is a **derived index**
kept in `<novel-folder>/.novelvault/`, so the folder stays portable: copy it to another
machine, open it, and the index is rebuilt by scanning.

---

## What is in this build

**Phase 1 — the file layer**

* A folder picker for opening an existing novel folder.
* Recursive scan with sha256, DOCX / PDF / TXT / MD text extraction.
* Version families — every save is a new version, exactly one CURRENT per family.
* A folder watcher that picks up a draft the moment you save it in Word.
* The Inbox: the only door through which anything joins the structured novel.
* Snapshots, an append-only audit trail, and Safe Mode (writes confined to `.novelvault/`).

**Phase 2 — create, browse, and record**

* **Create a novel** — choose the location, name it, and the folder plus its
  `.novelvault/` index are made. A reader sentinel is created with it (rule R4).
* **Bring files in by browsing** — pick files, or pick a folder and let it expand
  recursively. You are shown a plan first; nothing is written until you confirm the
  copy or move.
* Story records: arcs, chapters, characters, locations, objects, notes (10 categories,
  linkable to any entity) and ideas (4 statuses, never confused with canon).

**Phase 3 — the Canon Engine**

* Canon items with the 8-value vocabulary (PROPOSED → WORKING CANON → CANON →
  LOCKED CANON, plus DRAFT / OUTDATED / REJECTED / UNKNOWN).
* Sources on every fact, with the R3 gate enforced in code.
* Change a canon value and the old value is **kept forever** as OUTDATED, linked
  forward with `superseded_by`.
* Conflict detection that raises a question and never answers it, with your decision
  recorded (keep current / accept change / defer).
* Impact report: what a change would touch — notes, character knowledge, open conflicts,
  files containing the value, chapter records.
* Knowledge matrix separating what the reader knows from what each character knows.

**Phase 4 — the local AI librarian and the library of novels**

* **Librarian** — ask questions about your novel and get an answer with **numbered sources**.
  Sources are ranked by authority (the *Canon Shield*): locked canon → canon → working canon →
  current files → drafts → old drafts. Old, archived, rejected and unclassified material carries
  a visible warning, and a canon value that was later changed is announced as history, with the
  current value beside it.
* **Local only.** The model is one running on your own computer (Ollama, or any OpenAI-style
  local server such as llama.cpp or LM Studio). NovelVault refuses any address that is not
  `localhost` before it opens a socket, so an unpublished novel never leaves the machine.
* **Works with no model at all.** Without one, the librarian searches a full-text index of every
  extracted file and lays out the ranked sources directly. If a model is configured but not
  running, it falls back to this instead of failing.
* **Novel memory** — a per-file digest and a ledger of every character, location and object:
  how many passages mention it in *live* files versus only in older ones (so a picture frame that
  survives only in an old draft shows up).
* **Notices** — the librarian watches for unrecorded names and unreviewed ideas. They wait for
  you; accepting creates a story record (never canon), and a dismissed notice never returns.
* **Library of novels** — the app remembers every novel you open or create, with a one-glance
  summary of each. "Remove from list" never touches the folder.
* Chat history is kept per novel, inside that novel's `.novelvault/`, so it travels with it.

**Phase 5 — the Reader: an AI that learns the novel as it reads it** (this build)

* **A crawler, not a search box.** The Reader reads every draft **one file at a time, in story order**
  (arc → chapter → name → version) and keeps what it learns: the cast and places, what the text says
  about each of them, the key moments, the questions the story leaves open, and who appears with whom.
  Open **Story Brain** to watch it fill in; the first reading starts by itself.
* **It learns side by side with the novel.** A file is read once per content hash. Save a new draft (or
  bring one in) and only that file is read and folded in. What it learned from chapter 1 helps with chapter 2:
  a name it has seen often enough is recognised even where a first-time reader could not tell it was a name.
* **Authority follows the file, not the fact.** Every fact is stored against the file version it came from,
  with the sentence. Mark a draft CURRENT or archive it and the same facts instantly count as live or as
  history — nothing is re-read.
* **It notices when drafts disagree.** "Azmirah keeps the picture frame" in an old draft versus "the wooden
  bird" in a live one becomes a notice (and a *Drafts disagree* entry). It never picks a side and never
  touches canon — you decide.
* **The librarian uses it.** Asking about a character or "recap the plot" now adds the Reader's notes as
  sources, labelled *derived, not canon*, ranked below your canon and files.
* **Works with no model.** Reading is rule-based and deterministic. With a local model connected,
  **Read deeper** has it re-read live files for richer summaries and a running "story so far" — and every
  name, fact and event it returns is **checked against the file text**; anything the text does not support is
  discarded.

Command line: `novelvault read | brain | brain-entity "Name" | brain-deepen <novel>`.

The librarian is read-only toward your work: it cannot move, rename, edit or delete a file,
promote canon, or resolve a conflict. (Tests check that file hashes and canon are identical
after it has run.)

---

## The four rules, as enforced behaviour

| Rule | What it means | Where it is enforced |
| --- | --- | --- |
| **R1** | Nothing becomes canon automatically | Imports land `UNKNOWN`; canon items are created `PROPOSED` and only a call with a reason promotes them |
| **R2** | No silent deletion or overwrite | Soft delete everywhere; changing a value inserts a new row and marks the old one OUTDATED; append-only `canon_history`; an incoming file never replaces an existing one |
| **R3** | Every canon fact has a source | `promote` refuses any canon tier while the fact has no source, and names the rule in the error |
| **R4** | Reader knowledge ≠ character knowledge | A single reader sentinel per novel (filtered unique index) with its own knowledge rows |

---

## Install and run

**To get the installer without installing any tools:** push this folder to a GitHub repository
(it must be the repository root) and run the `build` workflow. It runs the engine tests and
every CLI walkthrough, type-checks the UI, then produces a single `NovelVault_*_x64-setup.exe` for Windows
(per-user, no administrator rights needed), plus an `.msi`, a macOS `.dmg` and a Linux
`.AppImage`/`.deb`/`.rpm`, as downloadable artifacts (Actions → the run → Artifacts). The run fails if an
installer is missing. Push a tag such as `v0.2.0` and the installers are attached to a GitHub Release.
The installer bundles the app; it does **not** bundle a language model (those are several GB) —
see "Local AI" below.

**To build it yourself** requires **Node 20+** and a **Rust toolchain** (`rustup`), plus the
[Tauri 2 system prerequisites](https://v2.tauri.app/start/prerequisites/) for your OS.

```bash
npm install
npm run app:dev      # run the desktop app
npm run app:build    # produce a native installer
```

Headless checks, no GUI needed:

```bash
cargo build -p novelvault-core -p novelvault-cli
cargo test  -p novelvault-core     # 18 unit + 24 file-layer + 22 phase 2/3 + 16 phase 4 + 10 phase 5 tests
npm run build                     # tsc --noEmit && vite build
bash scripts/demo.sh              # scan → inbox → classify → current → archive → snapshot
bash scripts/phase3-demo.sh       # create → browse in → canon → change → conflict → probe
bash scripts/phase4-demo.sh       # offline librarian → canon shield → notices → local-only → model
bash scripts/phase5-demo.sh       # the Reader learns chapter by chapter → drafts disagree (self-checking)
```

---

## Local AI

1. Install [Ollama](https://ollama.com) (free; runs on your PC).
2. Download a model, e.g. `ollama pull llama3.1:8b` (bigger = better answers, slower).
3. In NovelVault: **Librarian → Local AI**, pick the model, **Save**.

From a terminal: `novelvault ai-setup <novel> ollama --model llama3.1:8b`, then
`novelvault ai-ask <novel> "What is the keepsake?"`.

The model is only ever shown the numbered sources retrieved for a question, with instructions to
answer from them, cite them, and say so when the answer is not there. A model that cites nothing
is flagged in the UI.

---

## Trying Phase 2 and 3 from the command line

```bash
NV=./target/debug/novelvault

$NV new  ~/Novels --name "Saqar" --title "Saqar"

# Planning is free and changes nothing:
$NV add  ~/Novels/Saqar --from "~/Desktop/Arc 2 Revised v2.docx,~/Desktop/Old Arc 2.pdf"

# Applying needs --yes; a copy is verified by hash, an existing file is never replaced:
$NV add  ~/Novels/Saqar --from "~/Desktop/Arc 2 Revised v2.docx" --yes

$NV arc-add     ~/Novels/Saqar 1 --title "Arc One" --status Developing
$NV chapter-add ~/Novels/Saqar 1 1 --title "Wooden Bird"
$NV character-add ~/Novels/Saqar "Azmirah" --role protagonist

CANON=$($NV canon-propose ~/Novels/Saqar "Keepsake" "picture frame" --category object --json | python3 -c 'import sys,json,re;m=re.search(r"\{.*\}",sys.stdin.read(),re.S);print(json.loads(m.group(0))["id"])')
$NV canon-source  ~/Novels/Saqar "$CANON" FileVersion "<version-id>"
$NV canon-promote ~/Novels/Saqar "$CANON" CANON --reason "author confirmed after review"

# The revision. The old value is not destroyed — it becomes OUTDATED and points forward.
$NV canon-change  ~/Novels/Saqar "$CANON" "wooden bird" --reason "revised: the keepsake is a wooden bird"

$NV canon        ~/Novels/Saqar --all       # both values, with statuses
$NV probe        ~/Novels/Saqar             # the active draft still says "picture frame" → 1 conflict
$NV conflicts    ~/Novels/Saqar             # shown, not resolved
$NV canon-impact ~/Novels/Saqar "$CANON"    # what a change would touch
```

---

## Project layout

```
crates/novelvault-core/     the whole engine — no UI, no network
  db.rs                     vault, schema, audit, Safe Mode writes
  model.rs                  read models and status vocabularies
  scan.rs                   folder scan, hashing, import
  store.rs                  versions, families, search, snapshots, dashboard
  inbox.rs                  the Inbox
  watch.rs                  folder watcher with debounce
  story.rs                  arcs, chapters, scenes, characters, notes, ideas   (Phase 2)
  addfiles.rs               create a novel, browse files in, verify, move      (Phase 2)
  canon.rs                  canon items, sources, history, conflicts, knowledge (Phase 3)
  ai.rs                     index, Canon-Shield retrieval, librarian, memory, notices (Phase 4)
  llm.rs                    loopback-only local model client (Ollama / OpenAI-compatible)
  reader.rs                 the Reader: crawl drafts in story order, entities, facts, events, threads (Phase 5)
  library.rs                the list of novels the app knows about
crates/novelvault-cli/      the same engine from a terminal
src-tauri/                  Tauri shell — every command is a thin core wrapper
src/                        React + TypeScript UI
  pages/SetupPage.tsx       create a novel, or open an existing folder
  components/AddFilesPanel.tsx   browse → plan → confirm
  pages/StoryPage.tsx       arcs, chapters, ideas, notes
  pages/CanonPage.tsx       canon, conflicts, impact, history, knowledge
  pages/AiPage.tsx          librarian: ask, novel memory, notices, local AI setup
  pages/ReaderPage.tsx      Story Brain: story so far, cast, timeline, open threads, drafts that disagree
```

---

## Guarantees worth checking yourself

```bash
# Your drafts are untouched by a scan:
sha256sum ~/Novels/Saqar/*.docx > /tmp/before
./target/debug/novelvault scan ~/Novels/Saqar
sha256sum ~/Novels/Saqar/*.docx > /tmp/after
diff /tmp/before /tmp/after        # silent

# Canon keeps its history:
./target/debug/novelvault canon-history ~/Novels/Saqar <canon-id>
```

Two counters read zero in a fresh vault on purpose: `conflicts` and `ideas`. They are
filled by Phase 3's conflict probe and by your own captured ideas.

---

## Known limits

* Unsigned Windows build is untested — the development environment here is Linux.
* Watcher behaviour has been verified on Linux only.
* Legacy `.doc` is not extractable (`.docx` is).
* SQLite allows a single writer; two NovelVault windows on one folder should be avoided.
* The only network traffic is to a model server **on this computer**; there is no internet access.
* The installer does not include a language model. Without one you get the offline librarian.
* Retrieval is keyword (BM25) based, not semantic: "wooden bird" is found, but a paraphrase such
  as "carved sparrow" is not. A local-embeddings upgrade is the planned next step.
* Notices find names by capitalisation, so a character who is only ever mentioned at the start of
  a sentence may be missed, and a capitalised common word may be suggested. You decide each one.
* The Reader's rule-based reading understands plain declarative prose ("X kept Y", "X lived in Z"). It
  does not follow figurative language, long chains of pronouns, or events told out of order; "he/she" is
  guessed from the last person named and such facts are marked as guessed. A local model reads deeper,
  and its output is checked against the text, but real quality depends on the model you choose.
* The Reader reads text, so a scanned PDF with no extractable text is skipped (the app already reports it).
* Not yet built: a full timeline you can edit, clue tracking you can confirm, and the writing environment.
* Tested against a *pretend* Ollama (a scripted local server), not a real model. Real answer
  quality depends on the model you choose.
