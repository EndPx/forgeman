//! Minimal .env loader. Loads KEY=VALUE pairs into the process environment
//! without adding a dependency. Existing environment variables always win.

use std::path::Path;

/// Load a `.env` file. Returns the number of variables actually set.
/// Never overwrites variables already present in the environment.
pub fn load_dotenv(path: &Path) -> usize {
    let Ok(content) = std::fs::read_to_string(path) else {
        return 0;
    };
    let mut set = 0;
    for line in content.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        let key = key.trim();
        if key.is_empty() {
            continue;
        }
        let mut value = value.trim().to_string();
        if value.len() >= 2
            && ((value.starts_with('"') && value.ends_with('"'))
                || (value.starts_with('\'') && value.ends_with('\'')))
        {
            value = value[1..value.len() - 1].to_string();
        }
        // Safe: called from main before threads spawn; in tests single-threaded
        // per process. No concurrent access to the environment here.
        if std::env::var_os(key).is_none() {
            unsafe { std::env::set_var(key, value) };
            set += 1;
        }
    }
    set
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_and_skips_comments() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join(".env");
        std::fs::write(
            &path,
            "# comment\nFOO_TEST=bar\n\nQUOTED_TEST=\"hello world\"\nBAD LINE\n=empty\n",
        )
        .unwrap();

        load_dotenv(&path);
        assert_eq!(std::env::var("FOO_TEST").unwrap(), "bar");
        assert_eq!(std::env::var("QUOTED_TEST").unwrap(), "hello world");
    }
}
