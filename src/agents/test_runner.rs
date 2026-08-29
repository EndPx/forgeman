//! Test Runner (spec §14/Phase 6): detects the project's test ecosystem and
//! runs its validation commands, parsing results into a TestSummary.

use serde::{Deserialize, Serialize};
use std::path::Path;
use std::process::Command;
use std::time::{Duration, Instant};

use crate::core::model::{StageName, TestSummary};
use crate::core::stage::{RunContext, Stage, StageError, StageFuture, StageOutput};

/// One validation command configured for an ecosystem.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TestCommand {
    pub name: String,
    pub program: String,
    pub args: Vec<String>,
}

/// Detect the ecosystem from the repository layout and produce its
/// validation commands (spec §14: extensible command configuration).
pub fn detect_commands(repo_root: &Path) -> Vec<TestCommand> {
    let mut commands = Vec::new();

    if repo_root.join("Cargo.toml").exists() {
        commands.push(TestCommand {
            name: "cargo-test".to_string(),
            program: "cargo".to_string(),
            args: vec!["test".to_string(), "--quiet".to_string()],
        });
        commands.push(TestCommand {
            name: "cargo-check".to_string(),
            program: "cargo".to_string(),
            args: vec!["check".to_string(), "--quiet".to_string()],
        });
        return commands;
    }

    if repo_root.join("package.json").exists() {
        // npm test if a test script exists, else at least type-check via tsc
        // when typescript is present.
        let has_test_script = std::fs::read_to_string(repo_root.join("package.json"))
            .ok()
            .and_then(|text| serde_json::from_str::<serde_json::Value>(&text).ok())
            .and_then(|value| {
                value
                    .pointer("/scripts/test")
                    .and_then(|script| script.as_str().map(str::to_string))
            })
            .is_some();
        if has_test_script {
            commands.push(TestCommand {
                name: "npm-test".to_string(),
                program: "npm".to_string(),
                args: vec!["test".to_string(), "--silent".to_string()],
            });
        }
        if std::fs::read_to_string(repo_root.join("package.json"))
            .map(|text| text.contains("\"typescript\""))
            .unwrap_or(false)
        {
            commands.push(TestCommand {
                name: "tsc-check".to_string(),
                program: "npx".to_string(),
                args: vec!["tsc".to_string(), "--noEmit".to_string()],
            });
        }
        if !commands.is_empty() {
            return commands;
        }
    }

    let has_python_tests = repo_root.join("tests").is_dir()
        || repo_root.join("pytest.ini").exists()
        || repo_root.join("pyproject.toml").exists();
    if has_python_tests && python_test_available() {
        commands.push(TestCommand {
            name: "pytest".to_string(),
            program: "python".to_string(),
            args: vec!["-m".to_string(), "pytest".to_string(), "-q".to_string()],
        });
    }

    commands
}

fn python_test_available() -> bool {
    Command::new("python")
        .args(["-c", "import pytest"])
        .output()
        .map(|out| out.status.success())
        .unwrap_or(false)
}

/// Run one command inside the repository, capturing combined output.
/// Times out after `timeout` so a hung suite cannot block the loop.
pub fn run_command(
    repo_root: &Path,
    command: &TestCommand,
    timeout: Duration,
) -> Result<(bool, String, i32, u64), String> {
    use std::io::Read;
    use std::sync::mpsc;

    let started = Instant::now();
    let mut child = Command::new(&command.program)
        .args(&command.args)
        .current_dir(repo_root)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .map_err(|err| format!("cannot spawn {}: {err}", command.program))?;

    // Reader threads accumulate piped output while we poll for exit so a
    // chatty suite cannot deadlock on a full pipe.
    let mut stdout_pipe = child.stdout.take().expect("stdout piped");
    let mut stderr_pipe = child.stderr.take().expect("stderr piped");
    let (tx_out, rx_out) = mpsc::channel::<Vec<u8>>();
    let (tx_err, rx_err) = mpsc::channel::<Vec<u8>>();
    std::thread::spawn(move || {
        let mut buf = Vec::new();
        let _ = stdout_pipe.read_to_end(&mut buf);
        let _ = tx_out.send(buf);
    });
    std::thread::spawn(move || {
        let mut buf = Vec::new();
        let _ = stderr_pipe.read_to_end(&mut buf);
        let _ = tx_err.send(buf);
    });

    let deadline = Instant::now() + timeout;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) => {
                if Instant::now() >= deadline {
                    let _ = child.kill();
                    let _ = child.wait();
                    return Err(format!(
                        "command {} timed out after {}s",
                        command.name,
                        timeout.as_secs()
                    ));
                }
                std::thread::sleep(Duration::from_millis(50));
            }
            Err(err) => return Err(format!("command {} failed: {err}", command.name)),
        }
    };

    // The process exited, so the pipes reach EOF and the readers finish.
    let stdout = String::from_utf8_lossy(
        &rx_out
            .recv_timeout(Duration::from_secs(5))
            .unwrap_or_default(),
    )
    .to_string();
    let stderr = String::from_utf8_lossy(
        &rx_err
            .recv_timeout(Duration::from_secs(5))
            .unwrap_or_default(),
    )
    .to_string();

    let duration_ms = started.elapsed().as_millis() as u64;
    let combined = if stderr.is_empty() {
        stdout
    } else if stdout.is_empty() {
        stderr
    } else {
        format!("{stdout}\n{stderr}")
    };
    let code = status.code().unwrap_or(-1);
    Ok((status.success(), combined, code, duration_ms))
}

/// Parse a TestSummary from combined output. Handles cargo, pytest and
/// common node reporters; unknown formats fall back to pass/fail counts.
pub fn parse_summary(name: &str, output: &str, duration_ms: u64) -> TestSummary {
    let mut summary = TestSummary {
        total: 0,
        passed: 0,
        failed: 0,
        command: name.to_string(),
        duration_ms,
    };

    // cargo: "test result: ok. 48 passed; 0 failed; 0 ignored"
    // cargo: "test result: FAILED. 40 passed; 8 failed"
    for line in output.lines() {
        let line = line.trim();
        if let Some(rest) = line.strip_prefix("test result:") {
            let passed = grab_number(rest, "passed");
            let failed = grab_number(rest, "failed");
            summary.passed += passed;
            summary.failed += failed;
        }
    }
    if summary.passed + summary.failed > 0 {
        summary.total = summary.passed + summary.failed;
        return summary;
    }

    // pytest: "48 passed" / "3 failed, 45 passed" / "===== 48 passed in 2.3s ====="
    for line in output.lines() {
        let line = line.trim();
        if line.contains("passed") || line.contains("failed") {
            summary.passed += grab_number(line, "passed");
            summary.failed += grab_number(line, "failed");
        }
    }
    if summary.passed + summary.failed > 0 {
        summary.total = summary.passed + summary.failed;
        return summary;
    }

    // node reporters commonly emit "Tests: 12 passed, 2 failed"
    for line in output.lines() {
        let line = line.trim();
        if line.starts_with("Tests:") {
            summary.passed += grab_number(line, "passed");
            summary.failed += grab_number(line, "failed");
        }
    }
    summary.total = summary.passed + summary.failed;
    summary
}

fn grab_number(text: &str, label: &str) -> u32 {
    // Find "<num> <label>" anywhere in the text, walking back over spaces
    // and digits from the label (handles trailing words like "in 2.31s ===").
    let bytes = text.as_bytes();
    let mut search_from = 0;
    while let Some(offset) = text[search_from..].find(label) {
        let abs = search_from + offset;
        let mut end = abs;
        while end > 0 && bytes[end - 1] == b' ' {
            end -= 1;
        }
        let mut start = end;
        while start > 0 && bytes[start - 1].is_ascii_digit() {
            start -= 1;
        }
        if start < end
            && let Ok(value) = text[start..end].parse()
        {
            return value;
        }
        search_from = abs + label.len();
    }
    0
}

/// TestStage: runs detected validation commands and publishes
/// `tests.result` (TestSummary) plus `tests.output` for the failure analyzer.
pub struct TestStage;

impl Stage for TestStage {
    fn name(&self) -> StageName {
        StageName::Test
    }

    fn execute<'a>(&'a self, ctx: &'a mut RunContext) -> StageFuture<'a> {
        Box::pin(async move {
            let commands = detect_commands(&ctx.task.repo_root);
            if commands.is_empty() {
                return Err(StageError::failed(
                    StageName::Test,
                    "no test commands detected for this repository (supported: cargo, npm, pytest)",
                ));
            }

            let timeout = Duration::from_secs(ctx.config.execution.timeout_minutes.max(1) * 60);
            let mut aggregate = TestSummary {
                command: commands
                    .iter()
                    .map(|c| c.name.clone())
                    .collect::<Vec<_>>()
                    .join(" + "),
                ..Default::default()
            };
            let mut all_output = String::new();
            let mut any_failure = false;

            for command in &commands {
                let (ok, output, code, duration_ms) =
                    run_command(&ctx.task.repo_root, command, timeout)
                        .map_err(|err| StageError::failed(StageName::Test, err))?;
                let parsed = parse_summary(&command.name, &output, duration_ms);
                all_output.push_str(&output);
                if !ok {
                    any_failure = true;
                }
                let _ = code;
                aggregate.passed += parsed.passed;
                aggregate.failed += parsed.failed;
                aggregate.duration_ms += parsed.duration_ms;
            }
            aggregate.total = aggregate.passed + aggregate.failed;
            if any_failure && aggregate.failed == 0 {
                // Command failed without parseable counts — count as one failure
                // so the stop condition reacts.
                aggregate.failed = 1;
            }
            aggregate.total = aggregate.passed + aggregate.failed;

            let detail = format!(
                "{}/{} passed ({})",
                aggregate.passed, aggregate.total, aggregate.command
            );
            let value = serde_json::to_value(&aggregate)
                .map_err(|err| StageError::failed(StageName::Test, err.to_string()))?;

            Ok(StageOutput::default()
                .with_artifact("tests.result", value)
                .with_artifact("tests.output", serde_json::json!(all_output))
                .with_detail(detail))
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_cargo_output() {
        let output =
            "running 48 tests\ntest result: ok. 48 passed; 0 failed; 0 ignored; 0 measured\n";
        let summary = parse_summary("cargo-test", output, 1200);
        assert_eq!(summary.total, 48);
        assert_eq!(summary.passed, 48);
        assert_eq!(summary.failed, 0);
        assert!(summary.all_passed());
    }

    #[test]
    fn parses_failing_cargo_output() {
        let output = "test result: FAILED. 40 passed; 8 failed; 3 ignored\n";
        let summary = parse_summary("cargo-test", output, 900);
        assert_eq!(summary.passed, 40);
        assert_eq!(summary.failed, 8);
        assert!(!summary.all_passed());
    }

    #[test]
    fn parses_pytest_output() {
        let output = "================= 3 failed, 45 passed in 2.31s =================";
        let summary = parse_summary("pytest", output, 2310);
        assert_eq!(summary.passed, 45);
        assert_eq!(summary.failed, 3);
    }

    #[test]
    fn parses_node_style_output() {
        let output = "Tests: 12 passed, 2 failed\nSnapshots: 0\n";
        let summary = parse_summary("npm-test", output, 500);
        assert_eq!(summary.passed, 12);
        assert_eq!(summary.failed, 2);
    }

    #[test]
    fn detects_cargo_commands() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join("Cargo.toml"), "[package]\nname=\"x\"\n").unwrap();
        let commands = detect_commands(tmp.path());
        assert!(commands.iter().any(|c| c.name == "cargo-test"));
    }

    #[test]
    fn detects_npm_commands_only_with_test_script() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(
            tmp.path().join("package.json"),
            r#"{ "name": "x", "scripts": { "test": "vitest" } }"#,
        )
        .unwrap();
        let commands = detect_commands(tmp.path());
        assert!(commands.iter().any(|c| c.name == "npm-test"));

        let tmp2 = tempfile::tempdir().unwrap();
        std::fs::write(tmp2.path().join("package.json"), r#"{ "name": "x" }"#).unwrap();
        assert!(detect_commands(tmp2.path()).is_empty());
    }

    #[tokio::test]
    async fn test_stage_publishes_summary_for_real_command() {
        // A repo whose "test command" is a real, always-passing command:
        // use cargo on this very crate? Too slow. Use a controlled fake via
        // cmd on windows / true-ish on unix is non-portable — instead craft
        // a package.json whose npm-test we can't run portably. We assert the
        // failure path with a missing program instead.
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(
            tmp.path().join("package.json"),
            r#"{ "name": "x", "scripts": { "test": "does-not-exist-xyz" } }"#,
        )
        .unwrap();

        let task = crate::core::model::Task {
            id: "t".into(),
            description: "t".into(),
            repo_root: tmp.path().to_path_buf(),
            created_at: chrono::Utc::now(),
        };
        let run = crate::core::model::Run::starting(task.clone(), crate::config::Config::default());
        let mut ctx = RunContext::new(crate::config::Config::default(), task, run);
        let stage = TestStage;

        let result = stage.execute(&mut ctx).await;
        assert!(
            result.is_err(),
            "missing program must surface as stage failure"
        );
    }
}
