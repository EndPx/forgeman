//! Sandbox layer (spec §23/§37): coding-agent and validation commands run
//! with bounded privileges.
//!
//! - `Process` (default): commands run on the host but confined to the
//!   repository working directory with timeouts.
//! - `Docker` (`sandbox.enabled = true`): commands run in a throwaway
//!   container with the workspace mounted read-write, network restricted,
//!   and memory/CPU/PID limits applied. If Docker is unavailable, ForgeMan
//!   fails informatively instead of silently falling back to the host.

use std::path::Path;
use std::process::Command;

use crate::config::SandboxConfig;

/// Safety cap: runaway processes cannot spawn unbounded children.
const PIDS_LIMIT: u32 = 128;

pub fn detect_docker() -> bool {
    Command::new("docker")
        .arg("info")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|status| status.success())
        .unwrap_or(false)
}

/// Sensible images per ecosystem when `sandbox.image` is not configured.
pub fn default_image_for_ecosystem(repo_root: &Path) -> Option<&'static str> {
    if repo_root.join("Cargo.toml").exists() {
        Some("rust:1")
    } else if repo_root.join("package.json").exists() {
        Some("node:22-alpine")
    } else if repo_root.join("requirements.txt").exists()
        || repo_root.join("pyproject.toml").exists()
    {
        Some("python:3.12-slim")
    } else {
        None
    }
}

/// Pure builder for the docker argument vector (unit-testable).
pub fn docker_args(
    image: &str,
    memory: &str,
    cpus: f64,
    network: &str,
    mount: &Path,
    program: &str,
    args: &[String],
) -> Vec<String> {
    let network_arg = match network {
        "open" | "host" => "bridge".to_string(),
        _ => "none".to_string(),
    };
    let mut argv = vec![
        "run".to_string(),
        "--rm".to_string(),
        "-v".to_string(),
        format!("{}:/workspace", mount.display()),
        "-w".to_string(),
        "/workspace".to_string(),
        "--network".to_string(),
        network_arg,
        "--memory".to_string(),
        memory.to_string(),
        "--cpus".to_string(),
        format!("{cpus}"),
        "--pids-limit".to_string(),
        PIDS_LIMIT.to_string(),
        image.to_string(),
        program.to_string(),
    ];
    argv.extend(args.iter().cloned());
    argv
}

/// Build the `Command` for a validation step under the configured sandbox.
/// `docker_available` is injected so callers (and tests) control detection.
pub fn prepare(
    repo_root: &Path,
    command: &crate::agents::test_runner::TestCommand,
    sandbox: &SandboxConfig,
    docker_available: bool,
) -> Result<Command, String> {
    if !sandbox.enabled {
        let mut cmd = Command::new(&command.program);
        cmd.args(&command.args).current_dir(repo_root);
        return Ok(cmd);
    }

    if !docker_available {
        return Err(
            "sandbox.enabled=true but Docker is not available — refusing to run \
             on the host. Install/start Docker Desktop or set sandbox.enabled=false."
                .to_string(),
        );
    }

    let image = sandbox
        .image
        .clone()
        .or_else(|| default_image_for_ecosystem(repo_root).map(str::to_string))
        .ok_or_else(|| {
            "sandbox.enabled but no container image known for this ecosystem — \
                        set sandbox.image in the config"
                .to_string()
        })?;

    let mut cmd = Command::new("docker");
    cmd.args(docker_args(
        &image,
        &sandbox.memory,
        sandbox.cpus,
        &sandbox.network,
        repo_root,
        &command.program,
        &command.args,
    ));
    Ok(cmd)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn docker_args_apply_security_limits() {
        let args = docker_args(
            "rust:1",
            "1g",
            1.5,
            "restricted",
            Path::new("/repo"),
            "cargo",
            &["test".to_string()],
        );
        assert_eq!(
            args,
            vec![
                "run",
                "--rm",
                "-v",
                "/repo:/workspace",
                "-w",
                "/workspace",
                "--network",
                "none",
                "--memory",
                "1g",
                "--cpus",
                "1.5",
                "--pids-limit",
                "128",
                "rust:1",
                "cargo",
                "test",
            ]
        );
    }

    #[test]
    fn docker_network_open_maps_to_bridge() {
        let args = docker_args(
            "node:22-alpine",
            "512m",
            0.5,
            "open",
            Path::new("/repo"),
            "npm",
            &["test".to_string()],
        );
        assert!(args.contains(&"bridge".to_string()));
        assert!(args.contains(&"512m".to_string()));
        assert!(args.contains(&"0.5".to_string()));
    }

    #[test]
    fn process_sandbox_when_disabled() {
        let tmp = tempfile::tempdir().unwrap();
        let command = crate::agents::test_runner::TestCommand {
            name: "cargo-test".into(),
            program: "cargo".into(),
            args: vec!["test".into()],
        };
        let sandbox = SandboxConfig::default();
        let prepared = prepare(tmp.path(), &command, &sandbox, false).unwrap();
        assert_eq!(prepared.get_program(), "cargo");
    }

    #[test]
    fn enabled_without_docker_fails_informatively() {
        let tmp = tempfile::tempdir().unwrap();
        let command = crate::agents::test_runner::TestCommand {
            name: "cargo-test".into(),
            program: "cargo".into(),
            args: vec!["test".into()],
        };
        let mut sandbox = SandboxConfig::default();
        sandbox.enabled = true;
        let err = prepare(tmp.path(), &command, &sandbox, false).unwrap_err();
        assert!(err.contains("Docker is not available"));
    }

    #[test]
    fn enabled_uses_ecosystem_image() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join("Cargo.toml"), "[package]\nname=\"x\"\n").unwrap();
        let command = crate::agents::test_runner::TestCommand {
            name: "cargo-test".into(),
            program: "cargo".into(),
            args: vec!["test".into()],
        };
        let mut sandbox = SandboxConfig::default();
        sandbox.enabled = true;
        let prepared = prepare(tmp.path(), &command, &sandbox, true).unwrap();
        let program = prepared.get_program().to_string_lossy().to_string();
        assert!(program.ends_with("docker") || program == "docker");
    }
}
