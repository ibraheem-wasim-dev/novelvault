//! NovelVault V1 core.
//!
//! The novel folder is the source of truth. Everything in this crate is a *derived*
//! index stored inside `<novel>/.novelvault/`. Two properties are load-bearing and
//! are enforced here, not in the UI:
//!
//! 1. **Nothing outside `.novelvault/` is ever written, moved, renamed or deleted**
//!    (Safe Mode — see `db::write_vault_file`).
//! 2. **No file becomes canon and no file becomes CURRENT on its own.** Imports land
//!    as `UNKNOWN` and every import produces an Inbox item (R1).

pub mod addfiles;
pub mod ai;
pub mod canon;
pub mod db;
pub mod extract;
pub mod inbox;
pub mod library;
pub mod llm;
pub mod model;
pub mod reader;
pub mod scan;
pub mod store;
pub mod story;
pub mod util;
pub mod watch;

use thiserror::Error;

#[derive(Debug, Error)]
pub enum VaultError {
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("database error: {0}")]
    Db(#[from] rusqlite::Error),
    #[error("archive error: {0}")]
    Zip(#[from] zip::result::ZipError),
    #[error("json error: {0}")]
    Json(#[from] serde_json::Error),
    #[error("SAFE MODE: refused to write outside .novelvault/ ({0})")]
    SafeMode(String),
    #[error("not found: {0}")]
    NotFound(String),
    #[error("{0}")]
    Other(String),
}

pub type Result<T> = std::result::Result<T, VaultError>;
