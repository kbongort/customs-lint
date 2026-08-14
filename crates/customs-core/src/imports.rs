use tree_sitter::{Node, Parser, Point, Tree};

use crate::mapping::is_package_file;
use std::path::Path;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Import {
    /// Dotted module paths this statement is considered to import.
    pub modules: Vec<String>,
    pub start: Point,
    pub end: Point,
}

pub fn parse_tree(source: &str) -> Option<Tree> {
    let mut parser = Parser::new();
    parser
        .set_language(&tree_sitter_python::LANGUAGE.into())
        .ok()?;
    parser.parse(source, None)
}

/// Extract every static `import` / `from ... import`, including nested ones.
pub fn extract_imports(source: &str, file: &Path, importer_module: &str) -> Vec<Import> {
    let Some(tree) = parse_tree(source) else {
        return Vec::new();
    };
    let is_package = is_package_file(file);
    let mut out = Vec::new();
    walk(tree.root_node(), source, importer_module, is_package, &mut out);
    out
}

fn walk(
    node: Node,
    source: &str,
    importer_module: &str,
    is_package: bool,
    out: &mut Vec<Import>,
) {
    match node.kind() {
        "import_statement" => {
            if let Some(import) = parse_import_statement(node, source) {
                out.push(import);
            }
        }
        "import_from_statement" => {
            if let Some(import) = parse_import_from(node, source, importer_module, is_package) {
                out.push(import);
            }
        }
        _ => {
            let mut cursor = node.walk();
            for child in node.children(&mut cursor) {
                walk(child, source, importer_module, is_package, out);
            }
        }
    }
}

fn parse_import_statement(node: Node, source: &str) -> Option<Import> {
    let mut modules = Vec::new();
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        match child.kind() {
            "dotted_name" => modules.push(text(child, source)),
            "aliased_import" => {
                if let Some(name) = child.child_by_field_name("name") {
                    modules.push(text(name, source));
                }
            }
            _ => {}
        }
    }
    if modules.is_empty() {
        return None;
    }
    Some(Import {
        modules,
        start: node.start_position(),
        end: node.end_position(),
    })
}

fn parse_import_from(
    node: Node,
    source: &str,
    importer_module: &str,
    is_package: bool,
) -> Option<Import> {
    let module_node = node.child_by_field_name("module_name")?;
    let base = resolve_from_module(module_node, source, importer_module, is_package);

    let mut names: Vec<String> = Vec::new();
    let mut star = false;
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        match child.kind() {
            "wildcard_import" => star = true,
            "dotted_name" => {
                // Skip the module_name dotted_name; names also use dotted_name.
                if child.id() == module_node.id() {
                    continue;
                }
                // `from a.b import c` — c may be dotted_name or identifier-as-dotted_name
                names.push(text(child, source));
            }
            "aliased_import" => {
                if let Some(name) = child.child_by_field_name("name") {
                    names.push(text(name, source));
                }
            }
            "parenthesized_import_list" | "import_list" => {
                collect_imported_names(child, source, &mut names, &mut star);
            }
            _ => {}
        }
    }

    // Also collect `name` fields — tree-sitter-python marks imported names as field `name`.
    let mut field_cursor = node.walk();
    for name_node in node.children_by_field_name("name", &mut field_cursor) {
        if name_node.kind() == "aliased_import" {
            if let Some(name) = name_node.child_by_field_name("name") {
                let t = text(name, source);
                if !names.contains(&t) {
                    names.push(t);
                }
            }
        } else if name_node.kind() != "wildcard_import" {
            let t = text(name_node, source);
            if !names.contains(&t) {
                names.push(t);
            }
        }
    }

    let mut modules = Vec::new();
    if star {
        if let Some(base) = &base {
            modules.push(base.clone());
        }
    } else {
        if let Some(base) = &base {
            modules.push(base.clone());
        }
        for name in &names {
            if let Some(base) = &base {
                modules.push(format!("{base}.{name}"));
            } else {
                modules.push(name.clone());
            }
        }
    }

    modules.sort();
    modules.dedup();
    if modules.is_empty() {
        return None;
    }
    Some(Import {
        modules,
        start: node.start_position(),
        end: node.end_position(),
    })
}

fn collect_imported_names(node: Node, source: &str, names: &mut Vec<String>, star: &mut bool) {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        match child.kind() {
            "wildcard_import" => *star = true,
            "dotted_name" => {
                let t = text(child, source);
                if !names.contains(&t) {
                    names.push(t);
                }
            }
            "aliased_import" => {
                if let Some(name) = child.child_by_field_name("name") {
                    let t = text(name, source);
                    if !names.contains(&t) {
                        names.push(t);
                    }
                }
            }
            _ => collect_imported_names(child, source, names, star),
        }
    }
}

fn resolve_from_module(
    module_node: Node,
    source: &str,
    importer_module: &str,
    is_package: bool,
) -> Option<String> {
    match module_node.kind() {
        "dotted_name" => Some(text(module_node, source)),
        "relative_import" => {
            let raw = text(module_node, source);
            let dots = raw.chars().take_while(|c| *c == '.').count();
            let rest = raw[dots..].trim();
            let rest = if rest.is_empty() { None } else { Some(rest) };
            resolve_relative(importer_module, is_package, dots, rest)
        }
        _ => {
            let raw = text(module_node, source);
            if raw.starts_with('.') {
                let dots = raw.chars().take_while(|c| *c == '.').count();
                let rest = raw[dots..].trim();
                let rest = if rest.is_empty() { None } else { Some(rest) };
                resolve_relative(importer_module, is_package, dots, rest)
            } else if raw.is_empty() {
                None
            } else {
                Some(raw)
            }
        }
    }
}

/// Resolve a relative import to an absolute module path.
///
/// `dots` is the number of leading dots. One dot is the current package.
pub fn resolve_relative(
    importer_module: &str,
    is_package: bool,
    dots: usize,
    rest: Option<&str>,
) -> Option<String> {
    if dots == 0 {
        return rest.filter(|s| !s.is_empty()).map(str::to_string);
    }
    let mut parts: Vec<&str> = if importer_module.is_empty() {
        Vec::new()
    } else {
        importer_module.split('.').collect()
    };
    if !is_package && !parts.is_empty() {
        parts.pop();
    }
    let up = dots.saturating_sub(1);
    if up > parts.len() {
        return None;
    }
    parts.truncate(parts.len() - up);
    if let Some(rest) = rest {
        if !rest.is_empty() {
            parts.extend(rest.split('.'));
        }
    }
    if parts.is_empty() {
        None
    } else {
        Some(parts.join("."))
    }
}

fn text(node: Node, source: &str) -> String {
    node.utf8_text(source.as_bytes())
        .unwrap_or_default()
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn extract(source: &str, module: &str) -> Vec<Vec<String>> {
        extract_at(source, module, "pkg/mod.py")
    }

    fn extract_at(source: &str, module: &str, file: &str) -> Vec<Vec<String>> {
        extract_imports(source, &PathBuf::from(file), module)
            .into_iter()
            .map(|i| i.modules)
            .collect()
    }

    #[test]
    fn import_dotted() {
        let got = extract("import a.b.c\n", "pkg.mod");
        assert_eq!(got, vec![vec!["a.b.c".to_string()]]);
    }

    #[test]
    fn import_alias() {
        let got = extract("import a.b as c\n", "pkg.mod");
        assert_eq!(got, vec![vec!["a.b".to_string()]]);
    }

    #[test]
    fn from_import_is_conservative() {
        let got = extract(
            "from my_project.apps import service\n",
            "pkg.mod",
        );
        assert_eq!(
            got,
            vec![vec![
                "my_project.apps".to_string(),
                "my_project.apps.service".to_string()
            ]]
        );
    }

    #[test]
    fn from_import_star() {
        let got = extract("from a.b import *\n", "pkg.mod");
        assert_eq!(got, vec![vec!["a.b".to_string()]]);
    }

    #[test]
    fn nested_and_type_checking() {
        let source = r#"
from typing import TYPE_CHECKING

def f():
    import hidden.mod

if TYPE_CHECKING:
    from a import b
"#;
        let got = extract(source, "pkg.mod");
        assert!(got.iter().any(|m| m == &vec!["hidden.mod".to_string()]));
        assert!(got.iter().any(|m| m.contains(&"a".to_string())
            && m.contains(&"a.b".to_string())));
    }

    #[test]
    fn relative_from_module_file() {
        // pkg/sub.py → package is pkg
        let got = extract_at("from . import foo\n", "pkg.sub", "pkg/sub.py");
        assert_eq!(got, vec![vec!["pkg".to_string(), "pkg.foo".to_string()]]);
    }

    #[test]
    fn relative_from_package_init() {
        let got = extract_at("from . import foo\n", "pkg.sub", "pkg/sub/__init__.py");
        assert_eq!(
            got,
            vec![vec!["pkg.sub".to_string(), "pkg.sub.foo".to_string()]]
        );
    }

    #[test]
    fn relative_parent() {
        let got = extract_at("from ..foo import bar\n", "pkg.sub.mod", "pkg/sub/mod.py");
        assert_eq!(
            got,
            vec![vec!["pkg.foo".to_string(), "pkg.foo.bar".to_string()]]
        );
    }
}
