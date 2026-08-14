use std::path::{Path, PathBuf};

use crate::config::is_module_or_submodule;
use crate::config::CustomsConfig;

/// Walk up from `start` (file or directory) looking for `pyproject.toml`.
pub fn find_pyproject(start: &Path) -> Option<PathBuf> {
    let mut current = if start.is_file() {
        start.parent()?.to_path_buf()
    } else {
        start.to_path_buf()
    };
    loop {
        let candidate = current.join("pyproject.toml");
        if candidate.is_file() {
            return Some(candidate);
        }
        if !current.pop() {
            return None;
        }
    }
}

/// Map a source file to a dotted module path using the longest matching src-root.
///
/// Returns `None` when the file is not under any configured src-root.
pub fn file_to_module(file: &Path, project_root: &Path, config: &CustomsConfig) -> Option<String> {
    let file = normalize(file);
    let project_root = normalize(project_root);

    let mut best: Option<(usize, PathBuf)> = None;
    for root in &config.src_roots {
        let abs = if root == "." {
            project_root.clone()
        } else {
            project_root.join(root)
        };
        let abs = normalize(&abs);
        if file.starts_with(&abs) {
            let len = abs.as_os_str().len();
            if best.as_ref().map_or(true, |(best_len, _)| len > *best_len) {
                best = Some((len, abs));
            }
        }
    }
    let (_, root) = best?;
    let rel = file.strip_prefix(&root).ok()?;
    relative_to_module(rel)
}

fn relative_to_module(rel: &Path) -> Option<String> {
    let mut parts: Vec<String> = rel
        .iter()
        .map(|s| s.to_string_lossy().into_owned())
        .collect();
    if parts.is_empty() {
        return None;
    }
    let last = parts.last()?.as_str();
    if last == "__init__.py" || last == "__init__.pyi" {
        parts.pop();
    } else if let Some(stem) = last.strip_suffix(".pyi") {
        let stem = stem.to_string();
        *parts.last_mut()? = stem;
    } else if let Some(stem) = last.strip_suffix(".py") {
        let stem = stem.to_string();
        *parts.last_mut()? = stem;
    } else {
        return None;
    }
    if parts.is_empty() {
        return None;
    }
    Some(parts.join("."))
}

/// True when this file is a package (`__init__.py` / `__init__.pyi`).
pub fn is_package_file(file: &Path) -> bool {
    matches!(
        file.file_name().and_then(|n| n.to_str()),
        Some("__init__.py" | "__init__.pyi")
    )
}

pub fn is_ignored(importer_module: &str, config: &CustomsConfig) -> bool {
    config
        .ignore
        .iter()
        .any(|pkg| is_module_or_submodule(importer_module, pkg))
}

fn normalize(path: &Path) -> PathBuf {
    std::fs::canonicalize(path).unwrap_or_else(|_| {
        if path.is_absolute() {
            path.to_path_buf()
        } else {
            std::env::current_dir()
                .map(|cwd| cwd.join(path))
                .unwrap_or_else(|_| path.to_path_buf())
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::parse_pyproject;
    use std::fs;
    use std::sync::atomic::{AtomicU64, Ordering};

    static UNIQUE: AtomicU64 = AtomicU64::new(0);

    fn temp_tree() -> PathBuf {
        let n = UNIQUE.fetch_add(1, Ordering::SeqCst);
        let dir = std::env::temp_dir().join(format!("customs-map-{}-{}", std::process::id(), n));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn longest_src_root_wins() {
        let root = temp_tree();
        fs::create_dir_all(root.join("src/my_project/foo")).unwrap();
        fs::write(root.join("pyproject.toml"), "[tool.customs]\n").unwrap();
        fs::write(root.join("src/my_project/foo/bar.py"), "").unwrap();
        let config = parse_pyproject(
            r#"
[tool.customs]
src-roots = ["src", "."]
"#,
        )
        .unwrap();
        let module = file_to_module(&root.join("src/my_project/foo/bar.py"), &root, &config).unwrap();
        assert_eq!(module, "my_project.foo.bar");
    }

    #[test]
    fn init_py_maps_to_package() {
        let root = temp_tree();
        fs::create_dir_all(root.join("src/my_project/foo")).unwrap();
        fs::write(root.join("src/my_project/foo/__init__.py"), "").unwrap();
        let config = parse_pyproject("[tool.customs]\nsrc-roots = [\"src\"]\n").unwrap();
        let module =
            file_to_module(&root.join("src/my_project/foo/__init__.py"), &root, &config).unwrap();
        assert_eq!(module, "my_project.foo");
    }

    #[test]
    fn outside_src_roots_is_none() {
        let root = temp_tree();
        fs::create_dir_all(root.join("src")).unwrap();
        fs::create_dir_all(root.join("other")).unwrap();
        fs::write(root.join("other/x.py"), "").unwrap();
        let config = parse_pyproject("[tool.customs]\nsrc-roots = [\"src\"]\n").unwrap();
        assert!(file_to_module(&root.join("other/x.py"), &root, &config).is_none());
    }

    #[test]
    fn ignore_is_prefix_not_string_prefix() {
        let config = parse_pyproject("[tool.customs]\nignore = [\"tests\"]\n").unwrap();
        assert!(is_ignored("tests", &config));
        assert!(is_ignored("tests.test_foo", &config));
        assert!(!is_ignored("testsuite", &config));
        assert!(!is_ignored("my_project.tests", &config));
    }

    #[test]
    fn find_pyproject_walks_up() {
        let root = temp_tree();
        fs::create_dir_all(root.join("src/pkg")).unwrap();
        fs::write(root.join("pyproject.toml"), "").unwrap();
        fs::write(root.join("src/pkg/mod.py"), "").unwrap();
        let found = find_pyproject(&root.join("src/pkg/mod.py")).unwrap();
        assert_eq!(found, root.join("pyproject.toml"));
    }
}
