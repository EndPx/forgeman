use anyhow::{Context, Result};
use std::collections::BTreeMap;
use std::path::Path;

use super::profile::{Dependency, LanguageShare, RepositoryProfile, RiskArea, TreeEntry};

/// Directories that never contribute to repository intelligence.
const SKIP_DIRS: &[&str] = &[
    ".git",
    ".forgeman",
    "node_modules",
    "target",
    "dist",
    "build",
    "out",
    ".next",
    "venv",
    ".venv",
    "__pycache__",
    ".idea",
    ".vscode",
    "vendor",
];

const MAX_DEPTH: usize = 8;
const MAX_FILES: usize = 20_000;
const MAX_TREE_ENTRIES: usize = 200;

/// Inspect a repository and build its intelligence profile using
/// deterministic filesystem analysis (no LLM involved).
pub fn inspect(root: &Path) -> Result<RepositoryProfile> {
    if !root.exists() {
        anyhow::bail!("repository path does not exist: {}", root.display());
    }

    let files = walk(root)?;
    if files.is_empty() {
        anyhow::bail!(
            "no source files found in {} — is this a repository?",
            root.display()
        );
    }

    let languages = count_languages(&files);
    let primary_language = languages
        .first()
        .map(|l| l.language.clone())
        .unwrap_or_else(|| "unknown".to_string());

    let manifests = read_manifests(root);
    let file_set: Vec<String> = files.iter().map(|(rel, _)| rel.clone()).collect();

    let entrypoints = detect_entrypoints(&file_set);
    let test_frameworks = detect_test_frameworks(root, &manifests, &file_set);
    let databases = detect_databases(&manifests);
    let external_services = detect_external_services(&manifests);
    let risky_areas = detect_risky_areas(&file_set);
    let tree = build_tree(&file_set);

    Ok(RepositoryProfile {
        root: root.to_path_buf(),
        languages,
        primary_language: primary_language.clone(),
        framework: detect_framework(&manifests),
        package_manager: detect_package_manager(root, &primary_language, &manifests),
        entrypoints,
        test_frameworks,
        dependencies: manifests.dependencies.clone(),
        config_files: detect_config_files(&file_set),
        databases,
        external_services,
        risky_areas,
        file_count: files.len(),
        tree,
    })
}

type FileList = Vec<(String, usize)>;

/// Bounded recursive walk returning (relative path with forward slashes, size).
fn walk(root: &Path) -> Result<FileList> {
    let mut files = Vec::new();
    walk_dir(root, root, 0, &mut files)?;
    files.sort();
    Ok(files)
}

fn walk_dir(root: &Path, dir: &Path, depth: usize, files: &mut FileList) -> Result<()> {
    if depth > MAX_DEPTH || files.len() >= MAX_FILES {
        return Ok(());
    }
    for entry in std::fs::read_dir(dir).with_context(|| format!("reading {}", dir.display()))? {
        let entry = entry?;
        let path = entry.path();
        let name = entry.file_name().to_string_lossy().to_string();

        if path.is_dir() {
            // Hidden dirs are noise except .github, which carries CI config.
            let allow_hidden = name == ".github";
            if allow_hidden || (!SKIP_DIRS.contains(&name.as_str()) && !name.starts_with('.')) {
                walk_dir(root, &path, depth + 1, files)?;
            }
        } else {
            let size = entry.metadata().map(|m| m.len() as usize).unwrap_or(0);
            let rel = path
                .strip_prefix(root)
                .unwrap_or(&path)
                .to_string_lossy()
                .replace('\\', "/");
            files.push((rel, size));
            if files.len() >= MAX_FILES {
                return Ok(());
            }
        }
    }
    Ok(())
}

fn extension_of(path: &str) -> Option<&str> {
    let name = path.rsplit('/').next().unwrap_or(path);
    let (stem_dot, ext) = name.rsplit_once('.')?;
    // Hidden dotfiles like ".gitignore" have no meaningful extension.
    if stem_dot.is_empty() {
        return None;
    }
    Some(ext)
}

fn language_for(ext: &str) -> Option<&'static str> {
    Some(match ext {
        "rs" => "Rust",
        "js" | "jsx" | "mjs" | "cjs" => "JavaScript",
        "ts" | "tsx" => "TypeScript",
        "py" => "Python",
        "go" => "Go",
        "java" => "Java",
        "c" | "h" => "C",
        "cpp" | "cc" | "hpp" => "C++",
        "cs" => "C#",
        "rb" => "Ruby",
        "php" => "PHP",
        "swift" => "Swift",
        "kt" => "Kotlin",
        _ => return None,
    })
}

fn count_languages(files: &FileList) -> Vec<LanguageShare> {
    let mut counts: BTreeMap<&'static str, usize> = BTreeMap::new();
    for (rel, _) in files {
        if let Some(ext) = extension_of(rel)
            && let Some(lang) = language_for(ext)
        {
            *counts.entry(lang).or_insert(0) += 1;
        }
    }
    let mut shares: Vec<LanguageShare> = counts
        .into_iter()
        .map(|(language, files)| LanguageShare {
            language: language.to_string(),
            files,
        })
        .collect();
    shares.sort_by(|a, b| b.files.cmp(&a.files).then(a.language.cmp(&b.language)));
    shares
}

#[derive(Debug, Default, Clone)]
struct Manifests {
    /// (ecosystem, name, version) flattened dependency list.
    dependencies: Vec<Dependency>,
    /// Raw dependency names per ecosystem for quick matching.
    names: Vec<String>,
    /// Raw text of the primary manifest, for framework grep.
    cargo_toml: Option<String>,
    package_json: Option<serde_json::Value>,
    pyproject: Option<String>,
    requirements: Option<String>,
}

impl Manifests {
    fn has_dep(&self, name: &str) -> bool {
        self.names.iter().any(|n| n == name)
    }
}

fn read_manifests(root: &Path) -> Manifests {
    let mut m = Manifests::default();

    let cargo_path = root.join("Cargo.toml");
    if let Ok(text) = std::fs::read_to_string(&cargo_path) {
        if let Ok(value) = toml::from_str::<toml::Table>(&text) {
            for section in ["dependencies", "dev-dependencies", "build-dependencies"] {
                if let Some(deps) = value.get(section).and_then(|v| v.as_table()) {
                    for (name, val) in deps {
                        let version = extract_version(val);
                        m.dependencies.push(Dependency {
                            name: name.clone(),
                            version,
                        });
                        m.names.push(name.clone());
                    }
                }
            }
        }
        m.cargo_toml = Some(text);
    }

    let pkg_path = root.join("package.json");
    if let Ok(text) = std::fs::read_to_string(&pkg_path)
        && let Ok(value) = serde_json::from_str::<serde_json::Value>(&text)
    {
        for section in ["dependencies", "devDependencies"] {
            if let Some(deps) = value.get(section).and_then(|v| v.as_object()) {
                for (name, val) in deps {
                    m.dependencies.push(Dependency {
                        name: name.clone(),
                        version: val.as_str().map(str::to_string),
                    });
                    m.names.push(name.clone());
                }
            }
        }
        m.package_json = Some(value);
    }

    let pyproject_path = root.join("pyproject.toml");
    if let Ok(text) = std::fs::read_to_string(&pyproject_path)
        && let Ok(value) = toml::from_str::<toml::Table>(&text)
        && let Some(deps) = value
            .get("project")
            .and_then(|p| p.get("dependencies"))
            .and_then(|d| d.as_array())
    {
        for dep in deps {
            if let Some(spec) = dep.as_str() {
                let name = spec
                    .split(&['=', '>', '<', '[', ';', ' '][..])
                    .next()
                    .unwrap_or(spec);
                m.dependencies.push(Dependency {
                    name: name.to_string(),
                    version: None,
                });
                m.names.push(name.to_string());
            }
        }
        m.pyproject = Some(text);
    }

    let req_path = root.join("requirements.txt");
    if let Ok(text) = std::fs::read_to_string(&req_path) {
        for line in text.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let name = line
                .split(&['=', '>', '<', '[', ';', ' '][..])
                .next()
                .unwrap_or(line);
            m.dependencies.push(Dependency {
                name: name.to_string(),
                version: None,
            });
            m.names.push(name.to_string());
        }
        m.requirements = Some(text);
    }

    m
}

fn extract_version(value: &toml::Value) -> Option<String> {
    if let Some(v) = value.as_str() {
        return Some(v.to_string());
    }
    if let Some(table) = value.as_table() {
        return table
            .get("version")
            .and_then(|v| v.as_str())
            .map(str::to_string);
    }
    None
}

fn detect_framework(m: &Manifests) -> Option<String> {
    const RUST_FRAMEWORKS: &[(&str, &str)] = &[
        ("axum", "Axum"),
        ("actix-web", "Actix Web"),
        ("rocket", "Rocket"),
        ("warp", "Warp"),
        ("poem", "Poem"),
    ];
    const NODE_FRAMEWORKS: &[(&str, &str)] = &[
        ("next", "Next.js"),
        ("nuxt", "Nuxt"),
        ("react", "React"),
        ("vue", "Vue"),
        ("svelte", "Svelte"),
        ("express", "Express"),
        ("fastify", "Fastify"),
        ("nestjs-core", "NestJS"),
        ("hono", "Hono"),
    ];
    const PY_FRAMEWORKS: &[(&str, &str)] = &[
        ("django", "Django"),
        ("flask", "Flask"),
        ("fastapi", "FastAPI"),
        ("starlette", "Starlette"),
    ];

    for (dep, framework) in RUST_FRAMEWORKS
        .iter()
        .chain(NODE_FRAMEWORKS)
        .chain(PY_FRAMEWORKS)
    {
        if m.has_dep(dep) {
            return Some((*framework).to_string());
        }
    }
    None
}

fn detect_package_manager(root: &Path, language: &str, m: &Manifests) -> Option<String> {
    if root.join("Cargo.toml").exists() {
        return Some("cargo".to_string());
    }
    if root.join("pnpm-lock.yaml").exists() {
        return Some("pnpm".to_string());
    }
    if root.join("yarn.lock").exists() {
        return Some("yarn".to_string());
    }
    if root.join("package.json").exists() {
        return Some("npm".to_string());
    }
    if root.join("poetry.lock").exists() {
        return Some("poetry".to_string());
    }
    if root.join("uv.lock").exists() {
        return Some("uv".to_string());
    }
    if m.pyproject.is_some() || m.requirements.is_some() {
        return Some("pip".to_string());
    }
    if root.join("go.mod").exists() {
        return Some("go".to_string());
    }
    if language == "Go" {
        return Some("go".to_string());
    }
    None
}

fn detect_entrypoints(files: &[String]) -> Vec<String> {
    const CANDIDATES: &[&str] = &[
        "src/main.rs",
        "src/main.py",
        "main.py",
        "manage.py",
        "src/index.ts",
        "src/index.js",
        "index.ts",
        "index.js",
        "main.go",
        "app.py",
        "src/app.py",
        "src/App.tsx",
    ];
    CANDIDATES
        .iter()
        .filter(|c| files.iter().any(|f| f == *c))
        .map(|c| c.to_string())
        .collect()
}

fn detect_test_frameworks(root: &Path, m: &Manifests, files: &[String]) -> Vec<String> {
    let mut found = Vec::new();
    let has_tests_dir = files.iter().any(|f| f.starts_with("tests/")) || root.join("test").is_dir();
    let has_inline_rust_tests =
        m.cargo_toml.is_some() && files.iter().any(|f| f.starts_with("src/"));

    if m.cargo_toml.is_some() && (has_tests_dir || has_inline_rust_tests) {
        found.push("cargo-test".to_string());
    }
    if m.has_dep("criterion") {
        found.push("criterion".to_string());
    }
    const JS_TESTERS: &[&str] = &[
        "jest",
        "vitest",
        "mocha",
        "jasmine",
        "@playwright/test",
        "cypress",
    ];
    for tester in JS_TESTERS {
        if m.has_dep(tester) {
            found.push(tester.trim_start_matches('@').to_string());
        }
    }
    if m.package_json.is_some() && !found.is_empty() {
        // frameworks already appended above
    } else if let Some(scripts) = m
        .package_json
        .as_ref()
        .and_then(|v| v.get("scripts"))
        .and_then(|s| s.as_object())
        && scripts.contains_key("test")
    {
        found.push("npm-test".to_string());
    }
    const PY_TESTERS: &[&str] = &["pytest", "unittest2", "nose2"];
    for tester in PY_TESTERS {
        if m.has_dep(tester) {
            found.push((*tester).to_string());
        }
    }
    if (m.pyproject.is_some() || m.requirements.is_some())
        && files.iter().any(|f| f.starts_with("tests/"))
        && !found.iter().any(|f| f.starts_with("pytest"))
    {
        found.push("pytest-assumed".to_string());
    }
    if files.iter().any(|f| f.starts_with("tests/")) && found.is_empty() {
        found.push("tests-directory".to_string());
    }
    found.dedup();
    found
}

fn detect_databases(m: &Manifests) -> Vec<String> {
    const DB_DEPS: &[(&str, &str)] = &[
        ("postgres", "postgres"),
        ("postgresql", "postgres"),
        ("pg", "postgres"),
        ("tokio-postgres", "postgres"),
        ("sqlx", "sqlx"),
        ("diesel", "diesel"),
        ("sea-orm", "sea-orm"),
        ("rusqlite", "sqlite"),
        ("sqlite3", "sqlite"),
        ("mysql", "mysql"),
        ("mongodb", "mongodb"),
        ("mongoose", "mongodb"),
        ("redis", "redis"),
        ("prisma", "prisma"),
        ("typeorm", "typeorm"),
        ("sequelize", "sequelize"),
        ("psycopg2", "postgres"),
        ("psycopg2-binary", "postgres"),
        ("psycopg", "postgres"),
        ("pymongo", "mongodb"),
        ("sqlalchemy", "sqlalchemy"),
    ];
    let mut dbs = Vec::new();
    for (dep, db) in DB_DEPS {
        if m.has_dep(dep) && !dbs.iter().any(|d: &String| d == db) {
            dbs.push((*db).to_string());
        }
    }
    dbs
}

fn detect_external_services(m: &Manifests) -> Vec<String> {
    const SERVICE_DEPS: &[&str] = &[
        "reqwest",
        "hyper",
        "axios",
        "got",
        "node-fetch",
        "ky",
        "boto3",
        "aws-sdk",
        "stripe",
        "twilio",
        "sendgrid",
        "firebase",
        "supabase",
    ];
    SERVICE_DEPS
        .iter()
        .filter(|dep| m.has_dep(dep))
        .map(|dep| (*dep).to_string())
        .collect()
}

fn detect_risky_areas(files: &[String]) -> Vec<RiskArea> {
    const PATTERNS: &[(&str, &str)] = &[
        ("auth", "authentication"),
        ("jwt", "authentication"),
        ("login", "authentication"),
        ("session", "authentication"),
        ("token", "authentication"),
        ("password", "secrets"),
        ("secret", "secrets"),
        ("credential", "secrets"),
        ("migration", "database"),
        ("schema", "database"),
        ("payment", "payments"),
        ("billing", "payments"),
        ("checkout", "payments"),
        ("admin", "admin-surface"),
        ("crypto", "cryptography"),
        ("middleware", "api-surface"),
    ];
    let mut areas = Vec::new();
    for file in files {
        if areas.len() >= 30 {
            break;
        }
        let lower = file.to_lowercase();
        for (pattern, category) in PATTERNS {
            if lower.contains(pattern) {
                areas.push(RiskArea {
                    path: file.clone(),
                    category: (*category).to_string(),
                });
                break;
            }
        }
    }
    areas
}

fn detect_config_files(files: &[String]) -> Vec<String> {
    const CANDIDATES: &[&str] = &[
        "Dockerfile",
        "docker-compose.yml",
        "docker-compose.yaml",
        "Makefile",
        "justfile",
        ".github/workflows",
        "rust-toolchain.toml",
        "tsconfig.json",
        "vite.config.ts",
        "vite.config.js",
        "next.config.js",
        "next.config.ts",
        "pyproject.toml",
        "go.mod",
        ".env.example",
        "forge.yaml",
    ];
    let mut found = Vec::new();
    for candidate in CANDIDATES {
        let matches = files.iter().any(|f| {
            if candidate.ends_with('/') {
                f.starts_with(candidate)
            } else if candidate.contains('/') {
                f == candidate || f.starts_with(&format!("{candidate}/"))
            } else {
                f == candidate || f.split('/').next_back() == Some(candidate)
            }
        });
        if matches {
            found.push((*candidate).to_string());
        }
    }
    found
}

fn build_tree(files: &[String]) -> Vec<TreeEntry> {
    let mut seen = std::collections::BTreeSet::new();
    let mut tree = Vec::new();
    for file in files {
        let parts: Vec<&str> = file.split('/').collect();
        let depth_limit = parts.len().saturating_sub(1);
        for depth in 0..depth_limit.min(3) {
            let dir = parts[..=depth].join("/");
            if seen.insert(dir.clone()) {
                tree.push(TreeEntry {
                    path: dir,
                    is_dir: true,
                });
            }
        }
        if tree.len() >= MAX_TREE_ENTRIES {
            break;
        }
        if parts.len() <= 3 {
            tree.push(TreeEntry {
                path: file.clone(),
                is_dir: false,
            });
        }
        if tree.len() >= MAX_TREE_ENTRIES {
            break;
        }
    }
    tree
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn write(path: &Path, content: &str) {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        fs::write(path, content).unwrap();
    }

    #[test]
    fn detects_rust_repo_with_axum_and_postgres() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        write(
            &root.join("Cargo.toml"),
            "[package]\nname = \"api\"\nversion = \"0.1.0\"\n\n[dependencies]\naxum = \"0.7\"\ntokio = { version = \"1\", features = [\"full\"] }\ntokio-postgres = \"0.7\"\n",
        );
        write(&root.join("src/main.rs"), "fn main() {}\n");
        write(
            &root.join("src/services/auth_service.rs"),
            "pub fn check() {}\n",
        );
        write(
            &root.join("tests/integration/basic.rs"),
            "#[test]\nfn ok() {}\n",
        );

        let profile = inspect(root).unwrap();
        assert_eq!(profile.primary_language, "Rust");
        assert_eq!(profile.framework.as_deref(), Some("Axum"));
        assert_eq!(profile.package_manager.as_deref(), Some("cargo"));
        assert!(profile.entrypoints.contains(&"src/main.rs".to_string()));
        assert!(profile.test_frameworks.iter().any(|t| t == "cargo-test"));
        assert!(profile.databases.iter().any(|d| d == "postgres"));
        assert!(
            profile
                .risky_areas
                .iter()
                .any(|r| r.category == "authentication" && r.path.contains("auth_service"))
        );
    }

    #[test]
    fn detects_typescript_nextjs_repo() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        write(
            &root.join("package.json"),
            r#"{
  "name": "web",
  "scripts": { "test": "vitest run" },
  "dependencies": { "next": "14.0.0", "react": "18.0.0", "prisma": "5.0.0" },
  "devDependencies": { "vitest": "1.0.0" }
}"#,
        );
        write(&root.join("src/index.ts"), "console.log(1);\n");
        write(&root.join("src/app.tsx"), "export default () => null;\n");

        let profile = inspect(root).unwrap();
        assert_eq!(profile.primary_language, "TypeScript");
        assert_eq!(profile.framework.as_deref(), Some("Next.js"));
        assert_eq!(profile.package_manager.as_deref(), Some("npm"));
        assert!(profile.test_frameworks.iter().any(|t| t == "vitest"));
        assert!(profile.databases.iter().any(|d| d == "prisma"));
    }

    #[test]
    fn detects_python_fastapi_repo() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        write(
            &root.join("requirements.txt"),
            "fastapi==0.110.0\npsycopg2-binary==2.9\n",
        );
        write(&root.join("main.py"), "from fastapi import FastAPI\n");
        write(
            &root.join("tests/test_api.py"),
            "def test_ok():\n    assert True\n",
        );

        let profile = inspect(root).unwrap();
        assert_eq!(profile.primary_language, "Python");
        assert_eq!(profile.framework.as_deref(), Some("FastAPI"));
        assert!(profile.package_manager.is_some());
        assert!(profile.databases.iter().any(|d| d == "postgres"));
        assert!(
            profile
                .test_frameworks
                .iter()
                .any(|t| t.starts_with("pytest") || t == "tests-directory")
        );
    }

    #[test]
    fn skips_node_modules_and_target_dirs() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        write(&root.join("src/main.rs"), "fn main() {}\n");
        write(&root.join("target/debug/junk.rs"), "fn noise() {}\n");
        write(&root.join("node_modules/pkg/index.js"), "1;\n");

        let profile = inspect(root).unwrap();
        assert_eq!(profile.file_count, 1);
        assert!(!profile.tree.iter().any(|t| t.path.contains("target")));
    }

    #[test]
    fn empty_directory_is_a_graceful_error() {
        let tmp = tempfile::tempdir().unwrap();
        assert!(inspect(tmp.path()).is_err());
        assert!(inspect(Path::new("Z:/definitely/missing/path")).is_err());
    }

    #[test]
    fn config_files_are_detected() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        write(&root.join("Cargo.toml"), "[package]\nname=\"x\"\n");
        write(&root.join("src/main.rs"), "fn main() {}\n");
        write(&root.join("Dockerfile"), "FROM rust\n");
        write(&root.join(".github/workflows/ci.yml"), "on: push\n");

        let profile = inspect(root).unwrap();
        assert!(profile.config_files.contains(&"Dockerfile".to_string()));
        assert!(
            profile
                .config_files
                .contains(&".github/workflows".to_string())
        );
    }
}
