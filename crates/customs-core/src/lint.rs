use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime};

use tree_sitter::Point;

use crate::config::{is_module_or_submodule, is_submodules_token, load_config, CustomsConfig};
use crate::imports::extract_imports;
use crate::mapping::{file_to_module, is_ignored};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Violation {
    pub rule_name: String,
    pub controlled_module: String,
    pub start: Point,
    pub end: Point,
}

impl Violation {
    pub fn message(&self) -> String {
        format!(
            "Customs: Forbidden import of {} [{}]",
            self.controlled_module, self.rule_name
        )
    }
}

/// Lint a Python source file. Returns no violations when the file cannot be
/// mapped to a module (outside src-roots) or is ignored.
pub fn lint_source(
    file: &Path,
    source: &str,
    project_root: &Path,
    config: &CustomsConfig,
) -> Vec<Violation> {
    let Some(importer) = file_to_module(file, project_root, config) else {
        return Vec::new();
    };
    if is_ignored(&importer, config) {
        return Vec::new();
    }
    let imports = extract_imports(source, file, &importer);
    let mut violations = Vec::new();
    let mut seen = std::collections::BTreeSet::new();
    for import in imports {
        for module in &import.modules {
            if let Some((rule_name, rule)) = matching_rule(config, module) {
                if importer_allowed(&importer, rule_name, rule) {
                    continue;
                }
                let key = (import.start.row, import.start.column, rule_name.to_string());
                if !seen.insert(key) {
                    continue;
                }
                violations.push(Violation {
                    rule_name: rule_name.to_string(),
                    controlled_module: rule.module.clone(),
                    start: import.start,
                    end: import.end,
                });
            }
        }
    }
    violations
}

fn matching_rule<'a>(
    config: &'a CustomsConfig,
    imported: &str,
) -> Option<(&'a str, &'a crate::config::ModuleRule)> {
    config
        .module
        .iter()
        .filter(|(_, rule)| is_module_or_submodule(imported, &rule.module))
        .max_by_key(|(_, rule)| rule.module.len())
        .map(|(name, rule)| (name.as_str(), rule))
}

fn importer_allowed(importer: &str, _rule_name: &str, rule: &crate::config::ModuleRule) -> bool {
    for entry in &rule.resolved_allow() {
        if is_submodules_token(entry) {
            if is_module_or_submodule(importer, &rule.module) {
                return true;
            }
        } else if is_module_or_submodule(importer, entry) {
            return true;
        }
    }
    false
}

const STAT_INTERVAL: Duration = Duration::from_secs(1);

#[derive(Debug, Clone)]
struct CachedConfig {
    last_stat: Instant,
    meta: Option<(SystemTime, u64)>,
    /// `None` if the file was missing at last read.
    result: Option<Result<CustomsConfig, String>>,
}

/// Rate-limited pyproject.toml cache: stat at most once per second per path;
/// reread only when mtime or size changes.
#[derive(Debug, Default)]
pub struct ConfigStore {
    entries: HashMap<PathBuf, CachedConfig>,
}

impl ConfigStore {
    pub fn new() -> Self {
        Self::default()
    }

    /// Look up config for a pyproject.toml path.
    ///
    /// * `None` — file does not exist
    /// * `Some(Ok(_))` — parsed config
    /// * `Some(Err(_))` — parse/validation error
    pub fn get(&mut self, pyproject: &Path) -> Option<Result<CustomsConfig, String>> {
        let now = Instant::now();
        if let Some(entry) = self.entries.get(pyproject) {
            if now.duration_since(entry.last_stat) < STAT_INTERVAL {
                return entry.result.clone();
            }
        }

        let meta = std::fs::metadata(pyproject).ok().and_then(|m| {
            let modified = m.modified().ok()?;
            Some((modified, m.len()))
        });

        if let Some(entry) = self.entries.get_mut(pyproject) {
            if entry.meta == meta {
                entry.last_stat = now;
                return entry.result.clone();
            }
        }

        let result = if meta.is_none() {
            None
        } else {
            Some(load_config(pyproject).map_err(|e| e.to_string()))
        };

        self.entries.insert(
            pyproject.to_path_buf(),
            CachedConfig {
                last_stat: now,
                meta,
                result: result.clone(),
            },
        );
        result
    }
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
        let dir = std::env::temp_dir().join(format!("customs-lint-{}-{}", std::process::id(), n));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn prd_config() -> CustomsConfig {
        parse_pyproject(
            r#"
[tool.customs]
src-roots = ["src", "."]
ignore = ["examples", "tests"]

[tool.customs.module.my-service]
module = "my_project.apps.service"

[tool.customs.module.libraries-utils]
module = "my_project.libraries.utils"
allow = [
    "$submodules",
    "my_project.apps.service",
]
"#,
        )
        .unwrap()
    }

    fn write_py(root: &Path, rel: &str, body: &str) -> PathBuf {
        let path = root.join(rel);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        fs::write(&path, body).unwrap();
        path
    }

    fn messages(file: &Path, source: &str, root: &Path, config: &CustomsConfig) -> Vec<String> {
        lint_source(file, source, root, config)
            .into_iter()
            .map(|v| v.message())
            .collect()
    }

    #[test]
    fn outsider_cannot_import_controlled_module() {
        let root = temp_tree();
        let file = write_py(&root, "src/my_project/other.py", "");
        let source = "from my_project.apps import service\n";
        let msgs = messages(&file, source, &root, &prd_config());
        assert_eq!(
            msgs,
            vec!["Customs: Forbidden import of my_project.apps.service [my-service]"]
        );
    }

    #[test]
    fn submodules_may_import_each_other_by_default() {
        let root = temp_tree();
        let file = write_py(&root, "src/my_project/apps/service/app.py", "");
        let source = "from my_project.apps.service import handlers\n";
        let msgs = messages(&file, source, &root, &prd_config());
        assert!(msgs.is_empty());
    }

    #[test]
    fn explicit_allow_grants_subtree() {
        let root = temp_tree();
        let file = write_py(&root, "src/my_project/apps/service/app.py", "");
        let source = "from my_project.libraries.utils import client\n";
        let msgs = messages(&file, source, &root, &prd_config());
        assert!(msgs.is_empty());
    }

    #[test]
    fn outsider_cannot_import_launchdarkly() {
        let root = temp_tree();
        let file = write_py(&root, "src/my_project/other.py", "");
        let source = "import my_project.libraries.utils.client\n";
        let msgs = messages(&file, source, &root, &prd_config());
        assert_eq!(
            msgs,
            vec!["Customs: Forbidden import of my_project.libraries.utils [libraries-utils]"]
        );
    }

    #[test]
    fn ignored_importer_is_skipped() {
        let root = temp_tree();
        let file = write_py(&root, "tests/test_model.py", "");
        let source = "from my_project.apps.service import app\n";
        let msgs = messages(&file, source, &root, &prd_config());
        assert!(msgs.is_empty());
    }

    #[test]
    fn longest_prefix_wins() {
        let config = parse_pyproject(
            r#"
[tool.customs]
src-roots = ["src"]

[tool.customs.module.services]
module = "my_project.services"

[tool.customs.module.libraries-utils]
module = "my_project.libraries.utils"
allow = ["$submodules", "my_project.other"]
"#,
        )
        .unwrap();
        let root = temp_tree();
        let file = write_py(&root, "src/my_project/other.py", "");
        let source = "import my_project.libraries.utils\n";
        // Longest rule is libraries-utils, which allows my_project.other
        let msgs = messages(&file, source, &root, &config);
        assert!(msgs.is_empty());

        let source = "import my_project.services.other\n";
        let msgs = messages(&file, source, &root, &config);
        assert_eq!(
            msgs,
            vec!["Customs: Forbidden import of my_project.services [services]"]
        );
    }

    #[test]
    fn config_store_skips_reread_within_one_second() {
        let root = temp_tree();
        let path = root.join("pyproject.toml");
        fs::write(&path, "[tool.customs]\nignore = [\"a\"]\n").unwrap();
        let mut store = ConfigStore::new();
        let first = store.get(&path).unwrap().unwrap();
        assert_eq!(first.ignore, vec!["a"]);

        fs::write(&path, "[tool.customs]\nignore = [\"b\"]\n").unwrap();
        let cached = store.get(&path).unwrap().unwrap();
        assert_eq!(
            cached.ignore,
            vec!["a"],
            "must not reread within the rate limit"
        );
    }

    #[test]
    fn config_store_rereads_after_rate_limit() {
        let root = temp_tree();
        let path = root.join("pyproject.toml");
        fs::write(&path, "[tool.customs]\nignore = [\"a\"]\n").unwrap();
        let mut store = ConfigStore::new();
        store.get(&path).unwrap().unwrap();

        std::thread::sleep(Duration::from_millis(1100));
        fs::write(&path, "[tool.customs]\nignore = [\"b\"]\n").unwrap();
        let updated = store.get(&path).unwrap().unwrap();
        assert_eq!(updated.ignore, vec!["b"]);
    }

    #[test]
    fn explicit_allow_without_submodules_blocks_internal() {
        let config = parse_pyproject(
            r#"
[tool.customs]
src-roots = ["src"]

[tool.customs.module.locked]
module = "pkg.locked"
allow = ["pkg.trusted"]
"#,
        )
        .unwrap();
        let root = temp_tree();
        let file = write_py(&root, "src/pkg/locked/inner.py", "");
        let source = "from pkg.locked import sibling\n";
        let msgs = messages(&file, source, &root, &config);
        assert_eq!(
            msgs,
            vec!["Customs: Forbidden import of pkg.locked [locked]"]
        );
    }
}
