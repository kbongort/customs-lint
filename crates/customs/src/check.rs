use std::path::{Path, PathBuf};
use std::process::ExitCode;

use anyhow::Context;
use customs_core::{find_pyproject, lint_source, load_config, ConfigError, CustomsConfig};
use walkdir::WalkDir;

const SKIP_DIRS: &[&str] = &[
    ".git",
    ".hg",
    ".svn",
    ".venv",
    "venv",
    "node_modules",
    "target",
    "__pycache__",
    ".mypy_cache",
    ".pytest_cache",
    ".ruff_cache",
];

pub fn run(paths: &[PathBuf]) -> anyhow::Result<ExitCode> {
    let mut files = Vec::new();
    let mut pyprojects = Vec::new();
    for path in paths {
        collect_sources(path, &mut files, &mut pyprojects)?;
        if let Some(found) = find_pyproject(path) {
            pyprojects.push(normalize(&found));
        }
    }
    files.sort();
    files.dedup();
    pyprojects.sort();
    pyprojects.dedup();

    if pyprojects.is_empty() {
        eprintln!("warning: no pyproject.toml found; nothing to lint");
        return Ok(ExitCode::SUCCESS);
    }

    let roots: Vec<(PathBuf, PathBuf)> = pyprojects
        .iter()
        .filter_map(|pp| {
            let parent = pp.parent()?.to_path_buf();
            Some((parent, pp.clone()))
        })
        .collect();

    let mut cache: Vec<(PathBuf, Result<CustomsConfig, String>)> = Vec::new();
    let mut encountered_error = false;
    let mut violations = 0usize;

    for file in &files {
        let Some(pyproject) = pyproject_for_file(file, &roots) else {
            continue;
        };
        let project_root = pyproject.parent().unwrap_or(Path::new("."));
        let config = match cached_config(&mut cache, pyproject) {
            Ok(cfg) => cfg,
            Err(_) => {
                encountered_error = true;
                continue;
            }
        };
        let source = match std::fs::read_to_string(file) {
            Ok(s) => s,
            Err(err) => {
                eprintln!("error reading {}: {err}", file.display());
                encountered_error = true;
                continue;
            }
        };
        for v in lint_source(file, &source, project_root, config) {
            let line = v.start.row + 1;
            let col = v.start.column + 1;
            println!("{}:{line}:{col}: {}", file.display(), v.message());
            violations += 1;
        }
    }

    if encountered_error {
        Ok(ExitCode::from(2))
    } else if violations > 0 {
        Ok(ExitCode::from(1))
    } else {
        Ok(ExitCode::SUCCESS)
    }
}

fn pyproject_for_file<'a>(
    file: &Path,
    roots: &'a [(PathBuf, PathBuf)],
) -> Option<&'a PathBuf> {
    roots
        .iter()
        .filter(|(parent, _)| file.starts_with(parent))
        .max_by_key(|(parent, _)| parent.as_os_str().len())
        .map(|(_, pp)| pp)
}

fn cached_config<'a>(
    cache: &'a mut Vec<(PathBuf, Result<CustomsConfig, String>)>,
    pyproject: &Path,
) -> Result<&'a CustomsConfig, &'a String> {
    if let Some(idx) = cache.iter().position(|(p, _)| p == pyproject) {
        return cache[idx].1.as_ref();
    }
    let loaded = load_config(pyproject).map_err(|e: ConfigError| e.to_string());
    if let Err(e) = &loaded {
        eprintln!("error loading {}: {e}", pyproject.display());
    }
    cache.push((pyproject.to_path_buf(), loaded));
    cache.last().unwrap().1.as_ref()
}

fn collect_sources(
    path: &Path,
    files: &mut Vec<PathBuf>,
    pyprojects: &mut Vec<PathBuf>,
) -> anyhow::Result<()> {
    if path.is_file() {
        if is_python_file(path) {
            files.push(normalize(path));
        }
        if is_pyproject(path) {
            pyprojects.push(normalize(path));
        }
        return Ok(());
    }
    for entry in WalkDir::new(path).into_iter().filter_entry(|e| {
        if e.file_type().is_dir() {
            let name = e.file_name().to_string_lossy();
            !SKIP_DIRS.contains(&name.as_ref())
        } else {
            true
        }
    }) {
        let entry = entry.with_context(|| format!("walking {}", path.display()))?;
        if !entry.file_type().is_file() {
            continue;
        }
        let entry_path = entry.path();
        if is_python_file(entry_path) {
            files.push(normalize(entry_path));
        } else if is_pyproject(entry_path) {
            pyprojects.push(normalize(entry_path));
        }
    }
    Ok(())
}

fn is_python_file(path: &Path) -> bool {
    matches!(
        path.extension().and_then(|e| e.to_str()),
        Some("py" | "pyi")
    )
}

fn is_pyproject(path: &Path) -> bool {
    path.file_name().and_then(|n| n.to_str()) == Some("pyproject.toml")
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
