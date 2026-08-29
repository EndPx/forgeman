//! Tool abstraction (spec §36): the safe operations agents may perform on
//! the target repository. Every invocation is recorded as a `ToolExecution`
//! by the calling stage.
//!
//! Security (spec §37): all paths are confined to the repository root;
//! traversal outside it is rejected. Shell access is NOT exposed to the
//! coder — it is reserved for the test runner and sandbox phases.

// Some tools are consumed by stages landing in Phases 6–7.
#![allow(dead_code)]

use std::path::{Component, Path, PathBuf};
use std::time::Instant;

use crate::core::model::ToolExecution;

pub const FILE_READ: &str = "FileRead";
pub const FILE_WRITE: &str = "FileWrite";
pub const FILE_DELETE: &str = "FileDelete";
pub const FILE_SEARCH: &str = "FileSearch";

/// Maximum characters returned from a single file read (context budget).
pub const MAX_READ_CHARS: usize = 8_000;

/// Resolve `relative` inside `root`, rejecting traversal outside it.
pub fn safe_path(root: &Path, relative: &str) -> Result<PathBuf, String> {
    let rel = Path::new(relative);
    if rel.is_absolute() {
        return Err(format!("absolute path rejected: {relative}"));
    }
    for component in rel.components() {
        match component {
            Component::Normal(_) | Component::CurDir => {}
            Component::ParentDir | Component::Prefix(_) | Component::RootDir => {
                return Err(format!(
                    "path escapes repository root — rejected: {relative}"
                ));
            }
        }
    }
    let joined = root.join(rel);
    // Final containment check once the path exists on disk (symlinks etc.).
    if let Ok(canonical) = joined.canonicalize()
        && let Ok(root_canonical) = root.canonicalize()
        && !canonical.starts_with(&root_canonical)
    {
        return Err(format!(
            "path resolves outside repository root — rejected: {relative}"
        ));
    }
    Ok(joined)
}

pub fn read_file(root: &Path, relative: &str) -> Result<String, String> {
    let path = safe_path(root, relative)?;
    if !path.is_file() {
        return Err(format!("not a file: {relative}"));
    }
    let content = std::fs::read_to_string(&path).map_err(|err| format!("{relative}: {err}"))?;
    if content.len() > MAX_READ_CHARS {
        let mut cut = content[..MAX_READ_CHARS].to_string();
        cut.push_str("\n… [truncated by ForgeMan]");
        Ok(cut)
    } else {
        Ok(content)
    }
}

pub fn write_file(root: &Path, relative: &str, content: &str) -> Result<(), String> {
    let path = safe_path(root, relative)?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|err| format!("{relative}: {err}"))?;
    }
    std::fs::write(&path, content).map_err(|err| format!("{relative}: {err}"))
}

pub fn delete_file(root: &Path, relative: &str) -> Result<(), String> {
    let path = safe_path(root, relative)?;
    if path.is_file() {
        std::fs::remove_file(&path).map_err(|err| format!("{relative}: {err}"))
    } else {
        Err(format!("not a file (delete refused): {relative}"))
    }
}

/// Bounded grep: returns up to `MAX_MATCHES` `path:line: text` matches for
/// a literal query across text files in the repository.
pub const MAX_MATCHES: usize = 40;

pub fn search_files(root: &Path, query: &str) -> Result<Vec<String>, String> {
    if query.trim().is_empty() {
        return Err("empty search query".to_string());
    }
    let files = crate::repository::inspector::list_text_files(root)?;
    let mut matches = Vec::new();
    for rel in files {
        if matches.len() >= MAX_MATCHES {
            break;
        }
        let path = root.join(&rel);
        let Ok(content) = std::fs::read_to_string(&path) else {
            continue;
        };
        for (index, line) in content.lines().enumerate() {
            if line.contains(query) {
                matches.push(format!(
                    "{}:{}: {}",
                    rel,
                    index + 1,
                    line.trim().chars().take(200).collect::<String>()
                ));
                if matches.len() >= MAX_MATCHES {
                    break;
                }
            }
        }
    }
    Ok(matches)
}

/// Record one tool invocation into the run audit log and defer its events.
pub fn record_tool_execution(
    ctx: &mut crate::core::stage::RunContext,
    tool: &str,
    arguments: serde_json::Value,
    started: Instant,
    result: &Result<String, String>,
) {
    let duration_ms = started.elapsed().as_millis() as u64;
    let (summary, ok) = match result {
        Ok(text) => (text.clone(), true),
        Err(err) => (err.clone(), false),
    };
    ctx.run.tool_executions.push(ToolExecution {
        tool: tool.to_string(),
        arguments,
        result: summary,
        exit_code: None,
        duration_ms,
        timestamp: chrono::Utc::now(),
        iteration: ctx.current_iteration_index(),
    });
    ctx.defer_event(crate::core::events::EventKind::ToolCompleted {
        tool: tool.to_string(),
        ok,
        duration_ms,
        exit_code: None,
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn safe_path_rejects_traversal() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        assert!(safe_path(root, "src/main.rs").is_ok());
        assert!(safe_path(root, "../outside.txt").is_err());
        assert!(safe_path(root, "a/../../b").is_err());
        assert!(safe_path(root, "C:/windows/system32").is_err());
        assert!(safe_path(root, "/etc/passwd").is_err());
    }

    #[test]
    fn write_and_read_roundtrip_creates_dirs() {
        let tmp = tempfile::tempdir().unwrap();
        write_file(tmp.path(), "src/deep/mod.rs", "pub fn x() {}").unwrap();
        let content = read_file(tmp.path(), "src/deep/mod.rs").unwrap();
        assert_eq!(content, "pub fn x() {}");
    }

    #[test]
    fn read_truncates_large_files() {
        let tmp = tempfile::tempdir().unwrap();
        let big = "x".repeat(MAX_READ_CHARS + 100);
        write_file(tmp.path(), "big.txt", &big).unwrap();
        let content = read_file(tmp.path(), "big.txt").unwrap();
        assert!(content.len() < MAX_READ_CHARS + 100);
        assert!(content.contains("[truncated by ForgeMan]"));
    }

    #[test]
    fn delete_refuses_directories_and_missing() {
        let tmp = tempfile::tempdir().unwrap();
        fs::create_dir_all(tmp.path().join("dir")).unwrap();
        assert!(delete_file(tmp.path(), "dir").is_err());
        assert!(delete_file(tmp.path(), "missing.txt").is_err());
        write_file(tmp.path(), "file.txt", "x").unwrap();
        assert!(delete_file(tmp.path(), "file.txt").is_ok());
    }

    #[test]
    fn search_finds_matches_bounded() {
        let tmp = tempfile::tempdir().unwrap();
        write_file(tmp.path(), "a.rs", "let answer = 42;\n").unwrap();
        write_file(tmp.path(), "b.txt", "the answer is 42\n").unwrap();
        let matches = search_files(tmp.path(), "42").unwrap();
        assert_eq!(matches.len(), 2);
        assert!(matches[0].starts_with("a.rs:1:"));
        assert!(search_files(tmp.path(), "   ").is_err());
    }
}
