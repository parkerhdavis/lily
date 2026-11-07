// SPDX-License-Identifier: AGPL-3.0-or-later
//! Dated snapshots of client documents.
//!
//! Snapshots live under the app's local data directory rather than inside the
//! client folder, so they neither sync nor bloat the working tree. Each
//! document keeps the newest `RETENTION` versions.

use serde::Serialize;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

/// How many snapshots to keep per document.
const RETENTION: usize = 20;

fn sanitize(value: &str) -> String {
    let mut output = String::with_capacity(value.len());
    for character in value.chars() {
        if character.is_ascii_alphanumeric() || matches!(character, ' ' | '-' | '_' | '.') {
            output.push(character);
        } else {
            output.push('-');
        }
    }
    let trimmed = output.trim().trim_matches('.');
    if trimmed.is_empty() {
        "document".to_string()
    } else {
        trimmed.to_string()
    }
}

/// A stable fingerprint of the client folder, so two clients with the same
/// folder name do not share a history directory.
fn path_fingerprint(path: &Path) -> String {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in path.to_string_lossy().as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x100_0000_01b3);
    }
    format!("{:016x}", hash)
}

pub fn history_root() -> Result<PathBuf, String> {
    dirs::data_local_dir()
        .map(|directory| directory.join("lily").join("history"))
        .ok_or_else(|| "Could not determine the local data directory".to_string())
}

fn client_history_dir(root: &Path, client_dir: &Path) -> PathBuf {
    let name = client_dir
        .file_name()
        .and_then(|value| value.to_str())
        .unwrap_or("client");
    root.join(format!(
        "{}-{}",
        sanitize(name),
        &path_fingerprint(client_dir)[..8]
    ))
}

/// Copy `source` into its history directory and prune old versions.
pub fn snapshot_into(source: &Path, root: &Path) -> Result<Option<PathBuf>, String> {
    if !source.is_file() {
        return Ok(None);
    }
    let client_dir = source
        .parent()
        .ok_or_else(|| "Document has no parent directory".to_string())?;
    let directory = client_history_dir(root, client_dir);
    fs::create_dir_all(&directory)
        .map_err(|error| format!("Failed to create history directory: {}", error))?;
    let stem = source
        .file_stem()
        .and_then(|value| value.to_str())
        .unwrap_or("document");
    let stem = sanitize(stem);
    let timestamp = chrono::Local::now().format("%Y%m%d-%H%M%S%.9f");
    let mut destination = directory.join(format!("{}.{}.docx", stem, timestamp));
    let mut attempt = 0;
    while destination.exists() {
        attempt += 1;
        destination = directory.join(format!("{}.{}.{}.docx", stem, timestamp, attempt));
    }
    fs::copy(source, &destination).map_err(|error| {
        format!(
            "Failed to snapshot {}: {}",
            source.display(),
            error
        )
    })?;
    prune(&directory, &stem)?;
    Ok(Some(destination))
}

fn prune(directory: &Path, stem: &str) -> Result<(), String> {
    let prefix = format!("{}.", stem);
    let mut snapshots: Vec<PathBuf> = fs::read_dir(directory)
        .map_err(|error| format!("Failed to read history directory: {}", error))?
        .filter_map(|entry| entry.ok().map(|entry| entry.path()))
        .filter(|path| {
            path.file_name()
                .and_then(|value| value.to_str())
                .is_some_and(|name| name.starts_with(&prefix) && name.ends_with(".docx"))
        })
        .collect();
    snapshots.sort();
    if snapshots.len() > RETENTION {
        for path in &snapshots[..snapshots.len() - RETENTION] {
            fs::remove_file(path)
                .map_err(|error| format!("Failed to prune {}: {}", path.display(), error))?;
        }
    }
    Ok(())
}

pub fn snapshot_document_path(docx_path: &str) -> Result<Option<String>, String> {
    let root = history_root()?;
    let snapshot = snapshot_into(Path::new(docx_path), &root)?;
    Ok(snapshot.map(|path| path.to_string_lossy().to_string()))
}

#[derive(Debug, Clone, Serialize)]
pub struct DocumentVersion {
    pub path: String,
    pub name: String,
    pub modified: String,
    pub size: u64,
}

pub fn list_versions_into(
    docx_path: &Path,
    root: &Path,
) -> Result<Vec<DocumentVersion>, String> {
    let Some(client_dir) = docx_path.parent() else {
        return Ok(Vec::new());
    };
    let directory = client_history_dir(root, client_dir);
    if !directory.is_dir() {
        return Ok(Vec::new());
    }
    let stem = sanitize(
        docx_path
            .file_stem()
            .and_then(|value| value.to_str())
            .unwrap_or("document"),
    );
    let prefix = format!("{}.", stem);
    let mut versions = Vec::new();
    for entry in fs::read_dir(&directory)
        .map_err(|error| format!("Failed to read history directory: {}", error))?
    {
        let entry = entry.map_err(|error| format!("Failed to read history entry: {}", error))?;
        let path = entry.path();
        let name = entry.file_name().to_string_lossy().to_string();
        if !name.starts_with(&prefix) || !name.ends_with(".docx") {
            continue;
        }
        let metadata = entry
            .metadata()
            .map_err(|error| format!("Failed to read history metadata: {}", error))?;
        let modified_time = metadata.modified().unwrap_or(SystemTime::UNIX_EPOCH);
        let modified = chrono::DateTime::<chrono::Local>::from(modified_time)
            .format("%Y-%m-%d %H:%M:%S")
            .to_string();
        versions.push((
            modified_time,
            DocumentVersion {
                path: path.to_string_lossy().to_string(),
                name,
                modified,
                size: metadata.len(),
            },
        ));
    }
    versions.sort_by(|left, right| right.0.cmp(&left.0));
    Ok(versions
        .into_iter()
        .map(|(_, version)| version)
        .collect())
}

fn resolve_snapshot(
    docx_path: &Path,
    snapshot_path: &Path,
    root: &Path,
) -> Result<PathBuf, String> {
    let Some(client_dir) = docx_path.parent() else {
        return Err("Document has no parent directory".to_string());
    };
    let directory = client_history_dir(root, client_dir)
        .canonicalize()
        .map_err(|error| format!("Failed to resolve history directory: {}", error))?;
    let snapshot = snapshot_path
        .canonicalize()
        .map_err(|error| format!("Failed to resolve snapshot: {}", error))?;
    if !snapshot.starts_with(&directory) {
        return Err("Snapshot is outside the document's history".to_string());
    }
    Ok(snapshot)
}

pub fn restore_version_into(
    docx_path: &Path,
    snapshot_path: &Path,
    root: &Path,
) -> Result<(), String> {
    let snapshot = resolve_snapshot(docx_path, snapshot_path, root)?;
    let bytes = fs::read(&snapshot)
        .map_err(|error| format!("Failed to read snapshot: {}", error))?;
    snapshot_into(docx_path, root)?;
    fs::write(docx_path, bytes)
        .map_err(|error| format!("Failed to restore document: {}", error))?;
    Ok(())
}

#[derive(Debug, Clone, Serialize)]
pub struct VersionDiffLine {
    pub kind: String,
    pub text: String,
}

fn diff_lines(old: &[String], new: &[String]) -> Vec<VersionDiffLine> {
    let n = old.len();
    let m = new.len();
    let mut table = vec![vec![0usize; m + 1]; n + 1];
    for i in (0..n).rev() {
        for j in (0..m).rev() {
            table[i][j] = if old[i] == new[j] {
                table[i + 1][j + 1] + 1
            } else {
                table[i + 1][j].max(table[i][j + 1])
            };
        }
    }
    let mut output = Vec::new();
    let (mut i, mut j) = (0usize, 0usize);
    while i < n && j < m {
        if old[i] == new[j] {
            output.push(VersionDiffLine {
                kind: "same".to_string(),
                text: old[i].clone(),
            });
            i += 1;
            j += 1;
        } else if table[i + 1][j] >= table[i][j + 1] {
            output.push(VersionDiffLine {
                kind: "removed".to_string(),
                text: old[i].clone(),
            });
            i += 1;
        } else {
            output.push(VersionDiffLine {
                kind: "added".to_string(),
                text: new[j].clone(),
            });
            j += 1;
        }
    }
    while i < n {
        output.push(VersionDiffLine {
            kind: "removed".to_string(),
            text: old[i].clone(),
        });
        i += 1;
    }
    while j < m {
        output.push(VersionDiffLine {
            kind: "added".to_string(),
            text: new[j].clone(),
        });
        j += 1;
    }
    output
}

pub fn diff_versions_into(
    docx_path: &Path,
    snapshot_path: &Path,
    root: &Path,
) -> Result<Vec<VersionDiffLine>, String> {
    let snapshot = resolve_snapshot(docx_path, snapshot_path, root)?;
    let old = crate::docx_ops::document_lines(&snapshot)?;
    let new = crate::docx_ops::document_lines(docx_path)?;
    Ok(diff_lines(&old, &new))
}

#[tauri::command]
pub fn diff_document_versions(
    docx_path: String,
    snapshot_path: String,
) -> Result<Vec<VersionDiffLine>, String> {
    let root = history_root()?;
    diff_versions_into(Path::new(&docx_path), Path::new(&snapshot_path), &root)
}

/// Copy the current document into the local history before a render overwrites
/// it. Returns the snapshot path when the document already existed.
#[tauri::command]
pub fn snapshot_document(docx_path: String) -> Result<Option<String>, String> {
    snapshot_document_path(&docx_path)
}

#[tauri::command]
pub fn list_document_versions(docx_path: String) -> Result<Vec<DocumentVersion>, String> {
    let root = history_root()?;
    list_versions_into(Path::new(&docx_path), &root)
}

#[tauri::command]
pub fn restore_document_version(
    docx_path: String,
    snapshot_path: String,
) -> Result<(), String> {
    let root = history_root()?;
    restore_version_into(
        Path::new(&docx_path),
        Path::new(&snapshot_path),
        &root,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn temp_dir(label: &str) -> PathBuf {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let directory = std::env::temp_dir().join(format!(
            "lily-history-{}-{}",
            label, unique
        ));
        fs::create_dir_all(&directory).expect("temp directory");
        directory
    }

    #[test]
    fn snapshots_and_prunes_to_the_retention_limit() {
        let source_dir = temp_dir("source");
        let history = temp_dir("history");
        let source = source_dir.join("Will.docx");
        fs::write(&source, b"first").expect("source document");
        for _ in 0..RETENTION + 3 {
            snapshot_into(&source, &history).expect("snapshot");
        }
        let client_dir = client_history_dir(&history, &source_dir);
        let count = fs::read_dir(&client_dir).expect("history directory").count();
        assert_eq!(count, RETENTION);
        fs::remove_dir_all(&source_dir).ok();
        fs::remove_dir_all(&history).ok();
    }

    #[test]
    fn missing_documents_do_not_create_history() {
        let history = temp_dir("empty-history");
        let missing = history.join("missing.docx");
        assert_eq!(snapshot_into(&missing, &history).expect("snapshot"), None);
        fs::remove_dir_all(&history).ok();
    }

    #[test]
    fn lists_and_restores_versions() {
        let source_dir = temp_dir("restore-source");
        let history = temp_dir("restore-history");
        let source = source_dir.join("Will.docx");
        fs::write(&source, b"first").expect("first version");
        snapshot_into(&source, &history).expect("first snapshot");
        fs::write(&source, b"second").expect("second version");
        snapshot_into(&source, &history).expect("second snapshot");
        let versions = list_versions_into(&source, &history).expect("versions");
        assert_eq!(versions.len(), 2);
        let oldest = versions.last().expect("oldest").path.clone();
        restore_version_into(&source, Path::new(&oldest), &history).expect("restore");
        assert_eq!(fs::read(&source).expect("restored"), b"first");
        let after = list_versions_into(&source, &history).expect("versions after restore");
        assert!(after.len() >= 2);
        fs::remove_dir_all(&source_dir).ok();
        fs::remove_dir_all(&history).ok();
    }

    #[test]
    fn diff_lines_marks_added_and_removed() {
        let old = vec!["a".to_string(), "b".to_string(), "c".to_string()];
        let new = vec!["a".to_string(), "x".to_string(), "c".to_string()];
        let diff = diff_lines(&old, &new);
        assert!(diff
            .iter()
            .any(|line| line.kind == "removed" && line.text == "b"));
        assert!(diff
            .iter()
            .any(|line| line.kind == "added" && line.text == "x"));
        assert!(diff
            .iter()
            .any(|line| line.kind == "same" && line.text == "a"));
    }
}
