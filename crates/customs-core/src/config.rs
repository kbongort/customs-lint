use std::collections::BTreeMap;
use std::path::Path;

use serde::Deserialize;
use thiserror::Error;

const SUBMODULES: &str = "$submodules";

#[derive(Debug, Error)]
pub enum ConfigError {
    #[error("failed to read {path}: {source}")]
    Io {
        path: String,
        #[source]
        source: std::io::Error,
    },
    #[error("failed to parse pyproject.toml: {0}")]
    Toml(#[from] toml::de::Error),
    #[error("rule '{rule}' is missing a module path")]
    MissingModule { rule: String },
    #[error("rule '{rule}' uses unknown token '{token}'")]
    UnknownToken { rule: String, token: String },
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
struct PyProject {
    tool: Option<ToolSection>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
struct ToolSection {
    customs: Option<CustomsConfig>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct CustomsConfig {
    /// Source roots relative to the pyproject.toml directory.
    #[serde(rename = "src-roots", default = "default_src_roots")]
    pub src_roots: Vec<String>,
    /// Importer module prefixes to skip entirely.
    #[serde(default)]
    pub ignore: Vec<String>,
    /// Named rules keyed by rule name.
    #[serde(default)]
    pub module: BTreeMap<String, ModuleRule>,
}

impl Default for CustomsConfig {
    fn default() -> Self {
        Self {
            src_roots: default_src_roots(),
            ignore: Vec::new(),
            module: BTreeMap::new(),
        }
    }
}

fn default_src_roots() -> Vec<String> {
    vec![".".to_string()]
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct ModuleRule {
    /// Controlled module path. The rule applies to this module and its submodules.
    pub module: String,
    /// Who may import the controlled tree. `None` means `["$submodules"]`.
    /// When present, `$submodules` is not implied.
    pub allow: Option<Vec<String>>,
}

impl ModuleRule {
    /// Returns the allow list, applying the default when `allow` is omitted.
    pub fn resolved_allow(&self) -> Vec<String> {
        self.allow
            .clone()
            .unwrap_or_else(|| vec![SUBMODULES.to_string()])
    }
}

impl CustomsConfig {
    pub fn validate(&self) -> Result<(), ConfigError> {
        for (name, rule) in &self.module {
            if rule.module.trim().is_empty() {
                return Err(ConfigError::MissingModule { rule: name.clone() });
            }
            if let Some(allow) = &rule.allow {
                for entry in allow {
                    if let Some(token) = entry.strip_prefix('$') {
                        if entry != SUBMODULES {
                            return Err(ConfigError::UnknownToken {
                                rule: name.clone(),
                                token: format!("${token}"),
                            });
                        }
                    }
                }
            }
        }
        Ok(())
    }
}

/// Parse a pyproject.toml document into a Customs config.
/// Missing `[tool.customs]` yields an empty default config (no rules).
pub fn parse_pyproject(text: &str) -> Result<CustomsConfig, ConfigError> {
    let py: PyProject = toml::from_str(text)?;
    let config = py.tool.and_then(|t| t.customs).unwrap_or_default();
    config.validate()?;
    Ok(config)
}

pub fn load_config(path: &Path) -> Result<CustomsConfig, ConfigError> {
    let text = std::fs::read_to_string(path).map_err(|source| ConfigError::Io {
        path: path.display().to_string(),
        source,
    })?;
    parse_pyproject(&text)
}

/// True if `path` is `prefix` or a submodule of `prefix` (`a.b` matches `a.b.c`, not `a.bc`).
pub fn is_module_or_submodule(path: &str, prefix: &str) -> bool {
    path == prefix || path.starts_with(&format!("{prefix}."))
}

pub fn is_submodules_token(entry: &str) -> bool {
    entry == SUBMODULES
}

#[cfg(test)]
mod tests {
    use super::*;

    const EXAMPLE: &str = r#"
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
"#;

    #[test]
    fn parses_prd_example() {
        let config = parse_pyproject(EXAMPLE).unwrap();
        assert_eq!(config.src_roots, vec!["src", "."]);
        assert_eq!(config.ignore, vec!["examples", "tests"]);
        let ms = &config.module["my-service"];
        assert_eq!(ms.module, "my_project.apps.service");
        assert!(ms.allow.is_none());
        assert_eq!(ms.resolved_allow(), vec!["$submodules"]);
        let ld = &config.module["libraries-utils"];
        assert_eq!(
            ld.allow.as_deref(),
            Some(
                [
                    "$submodules".to_string(),
                    "my_project.apps.service".to_string()
                ]
                .as_slice()
            )
        );
    }

    #[test]
    fn missing_tool_customs_is_default() {
        let config = parse_pyproject("[project]\nname = \"x\"\n").unwrap();
        assert_eq!(config, CustomsConfig::default());
    }

    #[test]
    fn unknown_dollar_token_is_error() {
        let text = r#"
[tool.customs.module.foo]
module = "foo"
allow = ["$unknown"]
"#;
        let err = parse_pyproject(text).unwrap_err();
        assert!(matches!(err, ConfigError::UnknownToken { .. }));
    }

    #[test]
    fn module_or_submodule_uses_dot_boundary() {
        assert!(is_module_or_submodule("a.b", "a.b"));
        assert!(is_module_or_submodule("a.b.c", "a.b"));
        assert!(!is_module_or_submodule("a.bc", "a.b"));
        assert!(!is_module_or_submodule("a", "a.b"));
    }
}
