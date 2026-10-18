//! Creating a novel folder, and browsing outside files into it.
//!
//! This is the one place in the application that writes *outside* `.novelvault/`, so it
//! is deliberately awkward to call:
//!
//! * Planning is free and changes nothing (`plan`).
//! * Applying requires an explicit `confirmed` flag (`apply`), and refuses without it.
//! * A copy is written to a temporary name, its sha256 is verified against the source,
//!   and only then is it renamed into place. A half-written file never becomes visible.
//! * An existing file is **never overwritten**: the plan proposes a unique name instead.
//! * A move only deletes the source *after* the copy has been verified on disk, and the
//!   deletion is logged as its own audit row.
//! * Every file operation is recorded, so the Activity view shows exactly what moved.

use std::path::{Path, PathBuf};

use crate::db::Vault;
use crate::model::*;
use crate::util::{hash_file, new_id, now_iso};
use crate::{db, scan, Result, VaultError};

/// Creating a novel: the folder is made if it does not exist, and the vault inside it.
pub fn create_novel(parent_dir: &Path, name: &str, title: Option<&str>) -> Result<(PathBuf, Vault)> {
    let clean = name.trim();
    if clean.is_empty() {
        return Err(VaultError::Other("the novel needs a name".into()));
    }
    if clean.contains(['/', '\\', ':', '*', '?', '"', '<', '>', '|']) {
        return Err(VaultError::Other(
            "a folder name cannot contain \\ / : * ? \" < > |".into(),
        ));
    }
    let root = parent_dir.join(clean);
    std::fs::create_dir_all(&root)?;
    let vault = Vault::create(&root, title.unwrap_or(clean))?;
    // R4: every novel carries exactly one reader sentinel from the moment it exists.
    crate::story::ensure_reader_sentinel(&vault)?;
    vault.audit(
        "CreateNovelFolder",
        "Novel",
        Some(&vault.novel_id),
        None,
        Some(&format!("{{\"folder\":\"{}\"}}", root.display())),
    )?;
    Ok((root, vault))
}

/// A file type the vault understands. Anything else can still be brought in, but the
/// plan says plainly that it will not be readable.
fn is_supported(rel_or_name: &str) -> bool {
    matches!(
        crate::extract::extension(rel_or_name).as_str(),
        "docx" | "pdf" | "txt" | "md" | "markdown"
    )
}

fn unique_target(root: &Path, file_name: &str) -> String {
    if !root.join(file_name).exists() {
        return file_name.to_string();
    }
    let path = Path::new(file_name);
    let stem = path
        .file_stem()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| file_name.to_string());
    let ext = path
        .extension()
        .map(|e| format!(".{}", e.to_string_lossy()))
        .unwrap_or_default();
    for index in 2..1000 {
        let candidate = format!("{stem} ({index}){ext}");
        if !root.join(&candidate).exists() {
            return candidate;
        }
    }
    format!("{stem} ({}){ext}", now_iso())
}

/// Expand whatever the author picked in the browse dialog: files, and folders
/// (recursively). Ignore rules are applied, so lock files and `.git` never come along.
pub fn expand_sources(vault: &Vault, paths: &[String]) -> Result<Vec<PathBuf>> {
    let globs = scan::ignore_globs(vault);
    let mut out: Vec<PathBuf> = Vec::new();
    let mut seen: std::collections::HashSet<PathBuf> = std::collections::HashSet::new();

    fn walk(dir: &Path, out: &mut Vec<PathBuf>, seen: &mut std::collections::HashSet<PathBuf>) {
        let Ok(entries) = std::fs::read_dir(dir) else { return };
        for entry in entries.flatten() {
            let path = entry.path();
            if entry.file_type().map(|t| t.is_dir()).unwrap_or(false) {
                walk(&path, out, seen);
            } else if seen.insert(path.clone()) {
                out.push(path);
            }
        }
    }

    for raw in paths {
        let path = PathBuf::from(raw);
        if path.is_dir() {
            walk(&path, &mut out, &mut seen);
        } else if path.is_file() {
            let name = path
                .file_name()
                .map(|s| s.to_string_lossy().to_string())
                .unwrap_or_default();
            if scan::is_ignored(&name, &globs) {
                continue;
            }
            if seen.insert(path.clone()) {
                out.push(path);
            }
        }
    }
    Ok(out)
}

/// What bringing these files in would do. Changes nothing on disk.
pub fn plan(vault: &Vault, sources: &[String], move_files: bool) -> Result<AddPlan> {
    let root = vault.root.clone();
    let globs = scan::ignore_globs(vault);
    let expanded = expand_sources(vault, sources)?;
    let mut entries = Vec::new();
    let mut skipped = Vec::new();
    let mut total: i64 = 0;

    for path in expanded {
        let file_name = path
            .file_name()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_default();

        // Already inside the novel folder → nothing to do.
        if path.starts_with(&root) {
            skipped.push(AddEntry {
                source_path: path.display().to_string(),
                file_name: file_name.clone(),
                rel_target: String::new(),
                abs_target: String::new(),
                size_bytes: 0,
                action: AddAction::Skip.as_str().to_string(),
                note: Some("already inside this novel folder".into()),
            });
            continue;
        }

        if scan::is_ignored(&file_name, &globs) {
            skipped.push(AddEntry {
                source_path: path.display().to_string(),
                file_name: file_name.clone(),
                rel_target: String::new(),
                abs_target: String::new(),
                size_bytes: 0,
                action: AddAction::Skip.as_str().to_string(),
                note: Some("matches an ignore rule (temporary or lock file)".into()),
            });
            continue;
        }

        let size = std::fs::metadata(&path).map(|m| m.len() as i64).unwrap_or(0);
        let target_name = unique_target(&root, &file_name);
        let renamed = target_name != file_name;
        total += size;

        entries.push(AddEntry {
            source_path: path.display().to_string(),
            file_name,
            rel_target: target_name.clone(),
            abs_target: root.join(&target_name).display().to_string(),
            size_bytes: size,
            action: if move_files { AddAction::Move } else { AddAction::Copy }
                .as_str()
                .to_string(),
            note: if renamed {
                Some("a file with this name already exists — bringing it in as a new name rather than overwriting".into())
            } else if !is_supported(&target_name) {
                Some("this format has no text extractor, so it will be stored but not readable".into())
            } else {
                None
            },
        });
    }

    let verb = if move_files { "moved" } else { "copied" };
    let headline = if entries.is_empty() {
        format!("Nothing to bring in. {} file(s) were skipped.", skipped.len())
    } else {
        format!(
            "{} file(s) will be {verb} into the novel folder ({}){}.",
            entries.len(),
            crate::util::human_bytes(total.max(0) as u64),
            if skipped.is_empty() {
                String::new()
            } else {
                format!(", {} skipped", skipped.len())
            }
        )
    };

    Ok(AddPlan {
        novel_root: root.display().to_string(),
        move_files,
        entries,
        skipped,
        total_bytes: total,
        headline,
    })
}

/// Bring the files in. `confirmed` must be true — planning is free, applying is not.
pub fn apply(vault: &Vault, sources: &[String], move_files: bool, confirmed: bool) -> Result<Vec<AddOutcome>> {
    if !confirmed {
        return Err(VaultError::Other(
            "refusing to change files without confirmation: review the plan and confirm the copy/move first"
                .into(),
        ));
    }
    let root = vault.root.clone();
    let the_plan = plan(vault, sources, move_files)?;
    let mut outcomes = Vec::new();

    for entry in &the_plan.entries {
        let source = PathBuf::from(&entry.source_path);
        let final_target = PathBuf::from(&entry.abs_target);
        // The novel folder itself is the author's space: writing here happens only
        // because the author asked for it, through this confirmed path.
        let temp_target = final_target.with_extension(format!(
            "{}novelvault-partial",
            final_target
                .extension()
                .map(|e| format!("{}.", e.to_string_lossy()))
                .unwrap_or_default()
        ));

        let outcome = (|| -> Result<AddOutcome> {
            if final_target.exists() {
                return Err(VaultError::Other(format!(
                    "{} appeared while the plan was being applied — refusing to overwrite it",
                    entry.rel_target
                )));
            }
            if let Some(parent) = final_target.parent() {
                std::fs::create_dir_all(parent)?;
            }
            std::fs::copy(&source, &temp_target)?;

            // Verify before the file is allowed to exist under its real name.
            let (source_hash, source_size) = hash_file(&source)?;
            let (copy_hash, copy_size) = hash_file(&temp_target)?;
            if source_hash != copy_hash || source_size != copy_size {
                let _ = std::fs::remove_file(&temp_target);
                return Err(VaultError::Other(format!(
                    "the copy of {} did not match the original byte for byte; it was discarded and nothing was changed",
                    entry.file_name
                )));
            }

            std::fs::rename(&temp_target, &final_target)?;

            let relative = scan::rel_path(&root, &final_target);
            let imported = scan::import_file(vault, &relative)?;

            let mut source_removed = false;
            if move_files {
                // Only now, with a verified copy on disk, is the original removed.
                std::fs::remove_file(&source)?;
                source_removed = true;
                vault.audit(
                    "SourceRemovedAfterMove",
                    "File",
                    None,
                    Some(&entry.source_path),
                    Some(&relative),
                )?;
            }

            vault.conn.execute(
                "INSERT INTO file_events (id, novel_id, kind, rel_path, detail, at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                rusqlite::params![
                    new_id(),
                    vault.novel_id,
                    "BROUGHT_IN",
                    relative,
                    format!(
                        "{} from {} (sha256 {})",
                        if move_files { "Moved" } else { "Copied" },
                        entry.source_path,
                        &source_hash[..12]
                    ),
                    now_iso()
                ],
            )?;
            vault.audit(
                if move_files { "AddFileMove" } else { "AddFileCopy" },
                "FileVersion",
                imported.version_id.as_deref(),
                Some(&entry.source_path),
                Some(&relative),
            )?;

            Ok(AddOutcome {
                source_path: entry.source_path.clone(),
                rel_target: relative,
                action: entry.action.clone(),
                sha256: source_hash,
                verified: true,
                version_id: imported.version_id,
                source_removed,
                error: None,
            })
        })()
        .unwrap_or_else(|e| AddOutcome {
            source_path: entry.source_path.clone(),
            rel_target: entry.rel_target.clone(),
            action: entry.action.clone(),
            sha256: String::new(),
            verified: false,
            version_id: None,
            source_removed: false,
            error: Some(e.to_string()),
        });

        outcomes.push(outcome);
    }

    let failed = outcomes.iter().filter(|o| o.error.is_some()).count();
    vault.audit(
        "AddFiles",
        "Novel",
        Some(&vault.novel_id),
        None,
        Some(&format!(
            "{{\"requested\":{},\"failed\":{},\"mode\":\"{}\"}}",
            outcomes.len(),
            failed,
            if move_files { "move" } else { "copy" }
        )),
    )?;
    Ok(outcomes)
}

/// Used by the UI once a novel exists: the browse dialog hands back a folder.
pub fn describe_folder(path: &Path) -> String {
    path.display().to_string()
}

/// Sanity check used before creating a novel in a chosen parent folder.
pub fn parent_is_usable(parent: &Path) -> Result<()> {
    if !parent.is_dir() {
        return Err(VaultError::Other(format!(
            "{} is not a folder",
            parent.display()
        )));
    }
    let probe = parent.join(".novelvault-write-probe");
    std::fs::write(&probe, b"probe").map_err(|e| {
        VaultError::Other(format!("cannot write in {}: {e}", parent.display()))
    })?;
    let _ = std::fs::remove_file(&probe);
    Ok(())
}

/// Where the app keeps its own state for a folder it has opened.
pub fn vault_available(root: &Path) -> bool {
    db::db_path(root).exists()
}
