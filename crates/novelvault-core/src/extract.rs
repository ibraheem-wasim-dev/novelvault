//! Best-effort text extraction.
//!
//! Extraction never aborts an import: a file that cannot be read is still recorded,
//! with `extraction_state = failed` (or `unsupported`), because R2 says nothing is
//! silently dropped. DOCX is read as a zip and its `word/document.xml` is scanned for
//! `w:t` runs; PDF uses `pdf-extract`; TXT/MD are read directly.

use std::fs::File;
use std::io::Read;
use std::path::Path;

pub enum Extracted {
    Text(String),
    Unsupported,
    Failed(String),
}

/// Coarse category used for the scan headline and for iconography.
pub fn category(rel_path: &str) -> &'static str {
    match extension(rel_path).as_str() {
        "docx" | "doc" => "DOCX",
        "pdf" => "PDF",
        "md" | "markdown" => "Markdown",
        "txt" => "TXT",
        "png" | "jpg" | "jpeg" | "webp" | "gif" | "bmp" | "tif" | "tiff" | "svg" => "image",
        "" => "no-extension",
        _ => "other",
    }
}

pub fn extension(rel_path: &str) -> String {
    Path::new(rel_path)
        .extension()
        .map(|e| e.to_string_lossy().to_ascii_lowercase())
        .unwrap_or_default()
}

pub fn is_text_extractable(rel_path: &str) -> bool {
    matches!(extension(rel_path).as_str(), "docx" | "pdf" | "txt" | "md" | "markdown")
}

pub fn extract_file(abs_path: &Path) -> Extracted {
    let ext = extension(&abs_path.to_string_lossy());
    match ext.as_str() {
        "txt" | "md" | "markdown" => match read_plain(abs_path) {
            Ok(text) => Extracted::Text(text),
            Err(e) => Extracted::Failed(e),
        },
        "docx" => match read_docx(abs_path) {
            Ok(text) => Extracted::Text(text),
            Err(e) => Extracted::Failed(e),
        },
        "pdf" => read_pdf(abs_path),
        _ => Extracted::Unsupported,
    }
}

fn read_plain(path: &Path) -> std::result::Result<String, String> {
    let mut bytes = Vec::new();
    File::open(path)
        .map_err(|e| e.to_string())?
        .read_to_end(&mut bytes)
        .map_err(|e| e.to_string())?;
    Ok(sanitize(&String::from_utf8_lossy(&bytes)))
}

fn read_docx(path: &Path) -> std::result::Result<String, String> {
    let file = File::open(path).map_err(|e| e.to_string())?;
    let mut archive = zip::ZipArchive::new(file).map_err(|e| e.to_string())?;

    let mut xml = String::new();
    {
        let mut entry = archive
            .by_name("word/document.xml")
            .map_err(|_| "word/document.xml missing (not a .docx?)".to_string())?;
        entry.read_to_string(&mut xml).map_err(|e| e.to_string())?;
    }
    Ok(sanitize(&docx_runs_to_text(&xml)))
}

/// Pull the text out of `w:t` runs and turn each paragraph close into a newline.
fn docx_runs_to_text(xml: &str) -> String {
    let with_breaks = xml.replace("</w:p>", "\n");
    let mut out = String::new();
    let mut rest = with_breaks.as_str();
    while let Some(start) = rest.find("<w:t") {
        let after = &rest[start..];
        let Some(gt) = after.find('>') else { break };
        let content = &after[gt + 1..];
        let Some(end) = content.find("</w:t>") else { break };
        out.push_str(&unescape_xml(&content[..end]));
        rest = &content[end + 6..];
    }
    out
}

fn unescape_xml(value: &str) -> String {
    value
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&apos;", "'")
        .replace("&amp;", "&")
}

#[cfg(feature = "pdf")]
fn read_pdf(path: &Path) -> Extracted {
    match pdf_extract::extract_text(path) {
        Ok(text) => {
            if text.trim().is_empty() {
                Extracted::Failed("no extractable text (scanned or image-only PDF)".into())
            } else {
                Extracted::Text(sanitize(&text))
            }
        }
        Err(e) => Extracted::Failed(format!("pdf extraction failed: {e}")),
    }
}

#[cfg(not(feature = "pdf"))]
fn read_pdf(_path: &Path) -> Extracted {
    Extracted::Unsupported
}

fn sanitize(text: &str) -> String {
    text.replace('\r', "\n")
        .replace('\u{0}', "")
        .lines()
        .map(|l| l.trim_end())
        .collect::<Vec<_>>()
        .join("\n")
        .trim()
        .to_string()
}
