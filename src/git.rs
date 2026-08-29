//! Git integration (spec §22): Git is ForgeMan's state and memory system.
//! Every iteration becomes a logical checkpoint; diffs and history trace
//! every change back to its evidence.

use std::path::Path;
use std::process::Command;

fn git(root: &Path, args: &[&str]) -> Result<String, String> {
    let output = Command::new("git")
        .args(args)
        .current_dir(root)
        .output()
        .map_err(|err| format!("cannot run git (is it installed?): {err}"))?;
    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();
    if !output.status.success() {
        return Err(stderr.trim().to_string());
    }
    Ok(stdout)
}

/// Is the target repository under git version control?
pub fn is_repo(root: &Path) -> bool {
    root.join(".git").exists() || git(root, &["rev-parse", "--git-dir"]).is_ok()
}

/// Current HEAD commit hash (short).
pub fn current_commit(root: &Path) -> Result<Option<String>, String> {
    let out = git(root, &["rev-parse", "--short", "HEAD"])?;
    let hash = out.trim().to_string();
    if hash.is_empty() {
        Ok(None)
    } else {
        Ok(Some(hash))
    }
}

/// Stage all changes and create a checkpoint commit. Returns `Ok(None)`
/// when there was nothing to commit. The target repository's own
/// .gitignore governs what gets staged.
pub fn commit_all(root: &Path, message: &str) -> Result<Option<String>, String> {
    git(root, &["add", "-A"])?;
    let commit = Command::new("git")
        .args(["commit", "-m", message])
        .current_dir(root)
        .output()
        .map_err(|err| format!("cannot run git commit: {err}"))?;
    if !commit.status.success() {
        let stdout = String::from_utf8_lossy(&commit.stdout);
        let stderr = String::from_utf8_lossy(&commit.stderr);
        // "nothing to commit" lands on stdout; detect it on both streams.
        let combined = format!("{stdout}{stderr}");
        if combined.contains("nothing to commit") {
            return Ok(None);
        }
        return Err(stderr.trim().to_string());
    }
    current_commit(root)
}

/// Diff (with stats) between two revisions.
pub fn diff_between(root: &Path, base: &str, head: &str) -> Result<String, String> {
    let mut out = git(root, &["diff", "--stat", base, head])?;
    out.push_str(&git(root, &["diff", base, head])?);
    Ok(out)
}

/// List of commit hashes between base and head (oldest first).
pub fn commits_between(root: &Path, base: &str, head: &str) -> Result<Vec<String>, String> {
    let out = git(root, &["log", "--format=%h %s", &format!("{base}..{head}")])?;
    Ok(out.lines().map(str::to_string).collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn init_repo() -> tempfile::TempDir {
        let tmp = tempfile::tempdir().unwrap();
        git(tmp.path(), &["init", "-b", "main"]).unwrap();
        git(tmp.path(), &["config", "user.email", "forge@test.local"]).unwrap();
        git(tmp.path(), &["config", "user.name", "ForgeMan Test"]).unwrap();
        std::fs::write(tmp.path().join("file.txt"), "baseline\n").unwrap();
        commit_all(tmp.path(), "baseline commit").unwrap();
        tmp
    }

    #[test]
    fn commit_all_creates_checkpoint_and_detects_noop() {
        let tmp = init_repo();
        assert!(is_repo(tmp.path()));
        let hash = current_commit(tmp.path()).unwrap();
        assert!(hash.is_some());

        // New change → checkpoint with a hash.
        std::fs::write(tmp.path().join("file.txt"), "changed\n").unwrap();
        let after = commit_all(tmp.path(), "forgeman: iteration 0").unwrap();
        assert!(after.is_some());
        assert_ne!(hash, after);

        // No change → Ok(None), no infinite retries.
        let noop = commit_all(tmp.path(), "forgeman: empty iteration").unwrap();
        assert!(noop.is_none());
    }

    #[test]
    fn diff_between_shows_changes() {
        let tmp = init_repo();
        let base = current_commit(tmp.path()).unwrap().unwrap();
        std::fs::write(tmp.path().join("file.txt"), "changed\n").unwrap();
        commit_all(tmp.path(), "iteration").unwrap();
        let head = current_commit(tmp.path()).unwrap().unwrap();

        let diff = diff_between(tmp.path(), &base, &head).unwrap();
        assert!(
            diff.contains("file.txt"),
            "diff stat must name the file: {diff}"
        );
        assert!(diff.contains("baseline"));

        let commits = commits_between(tmp.path(), &base, &head).unwrap();
        assert_eq!(commits.len(), 1);
        assert!(commits[0].contains("iteration"));
    }

    #[test]
    fn non_repo_reports_gracefully() {
        let tmp = tempfile::tempdir().unwrap();
        assert!(!is_repo(tmp.path()));
        assert!(current_commit(tmp.path()).is_err());
    }
}
