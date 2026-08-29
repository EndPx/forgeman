use anyhow::{Context, Result};
use std::path::{Path, PathBuf};

use crate::config::FORGEMAN_DIR;
use crate::core::model::Run;

/// Filesystem-backed store for run records and event logs.
///
/// Layout:
/// ```text
/// <repo>/.forgeman/runs/<run_id>/run.json       — full run record
/// <repo>/.forgeman/runs/<run_id>/events.jsonl   — streamed event log
/// ```
pub struct RunStore {
    /// `<repo>/.forgeman/runs`
    pub root: PathBuf,
}

impl RunStore {
    pub fn new(repo_root: &Path) -> Self {
        Self {
            root: repo_root.join(FORGEMAN_DIR).join("runs"),
        }
    }

    fn run_dir(&self, run_id: &str) -> PathBuf {
        self.root.join(run_id)
    }

    pub fn events_path(&self, run_id: &str) -> PathBuf {
        self.run_dir(run_id).join("events.jsonl")
    }

    pub fn save_run(&self, run: &Run) -> Result<PathBuf> {
        let dir = self.run_dir(&run.id);
        std::fs::create_dir_all(&dir)
            .with_context(|| format!("failed to create {}", dir.display()))?;
        let path = dir.join("run.json");
        let json = serde_json::to_string_pretty(run)
            .context("failed to serialize run record")?;
        std::fs::write(&path, json)
            .with_context(|| format!("failed to write {}", path.display()))?;
        Ok(path)
    }

    pub fn load_run(&self, run_id: &str) -> Result<Run> {
        let path = self.run_dir(run_id).join("run.json");
        let raw = std::fs::read_to_string(&path)
            .with_context(|| format!("failed to read {}", path.display()))?;
        serde_json::from_str(&raw).with_context(|| format!("invalid run record {path:?}"))
    }

    /// Run ids sort lexicographically == chronologically (`run_YYYYMMDD_HHMMSS_xx`).
    pub fn list_run_ids(&self) -> Result<Vec<String>> {
        if !self.root.exists() {
            return Ok(Vec::new());
        }
        let mut ids = Vec::new();
        for entry in std::fs::read_dir(&self.root)? {
            let entry = entry?;
            if entry.file_type()?.is_dir() {
                if let Some(name) = entry.file_name().to_str() {
                    if name.starts_with("run_") && self.run_dir(name).join("run.json").exists() {
                        ids.push(name.to_string());
                    }
                }
            }
        }
        ids.sort();
        Ok(ids)
    }

    pub fn latest_run_id(&self) -> Result<Option<String>> {
        Ok(self.list_run_ids()?.pop())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;
    use crate::core::model::{new_run_id, new_task_id, Run, RunStatus, Task};
    use chrono::Utc;
    use std::path::PathBuf;

    fn make_run(id: &str) -> Run {
        let task = Task {
            id: new_task_id(),
            description: "Fix the auth bug".into(),
            repo_root: PathBuf::from("/tmp/repo"),
            created_at: Utc::now(),
        };
        let mut run = Run::starting(task, Config::default());
        run.id = id.to_string();
        run
    }

    #[test]
    fn save_and_load_roundtrip() {
        let tmp = tempfile::tempdir().unwrap();
        let store = RunStore::new(tmp.path());
        let run = make_run("run_20260829_120000_abc123");
        store.save_run(&run).unwrap();
        let loaded = store.load_run(&run.id).unwrap();
        assert_eq!(loaded.task.description, "Fix the auth bug");
        assert_eq!(loaded.status, RunStatus::Running);
    }

    #[test]
    fn list_and_latest_are_chronological() {
        let tmp = tempfile::tempdir().unwrap();
        let store = RunStore::new(tmp.path());
        assert_eq!(store.list_run_ids().unwrap(), Vec::<String>::new());

        store.save_run(&make_run("run_20260829_120000_abc123")).unwrap();
        store.save_run(&make_run("run_20260829_120001_def456")).unwrap();
        store.save_run(&make_run("run_20260828_235900_000aaa")).unwrap();
        // Directory without run.json is ignored.
        std::fs::create_dir_all(tmp.path().join(".forgeman/runs/run_junk")).unwrap();

        let ids = store.list_run_ids().unwrap();
        assert_eq!(ids.len(), 3);
        assert_eq!(ids[0], "run_20260828_235900_000aaa");
        assert_eq!(store.latest_run_id().unwrap().unwrap(), "run_20260829_120001_def456");
    }

    #[test]
    fn new_run_ids_have_sortable_format() {
        let a = new_run_id();
        // Lexicographic order == chronological for ids generated in the same
        // second too, because the random suffix is uniform hex — the property
        // that matters is the `run_YYYYMMDD_HHMMSS_xxxxxx` shape.
        assert!(a.starts_with("run_"));
        let parts: Vec<&str> = a.split('_').collect();
        assert_eq!(parts.len(), 4);
        assert_eq!(parts[1].len(), 8);
        assert!(parts[1].chars().all(|c| c.is_ascii_digit()));
        assert_eq!(parts[2].len(), 6);
        assert!(parts[2].chars().all(|c| c.is_ascii_digit()));
        assert_eq!(parts[3].len(), 6);
    }
}
