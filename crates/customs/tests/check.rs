use std::fs;
use std::path::PathBuf;
use std::process::Command;

fn fixture(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/fixtures")
        .join(name)
        .canonicalize()
        .unwrap()
}

fn customs() -> Command {
    Command::new(env!("CARGO_BIN_EXE_customs"))
}

#[test]
fn check_reports_forbidden_imports() {
    let out = customs()
        .args(["check", fixture("basic").to_str().unwrap()])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(1), "stderr: {}", String::from_utf8_lossy(&out.stderr));
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("Forbidden import of my_project.apps.service [my-service]"),
        "{stdout}"
    );
    assert!(
        stdout.contains("Forbidden import of my_project.libraries.utils [libraries-utils]"),
        "{stdout}"
    );
    assert!(!stdout.contains("test_model.py"), "{stdout}");
    assert!(!stdout.contains("demo.py"), "{stdout}");
    assert!(!stdout.contains("service/app.py"), "{stdout}");
}

#[test]
fn check_warns_without_pyproject() {
    let empty = tempfile();
    let out = customs()
        .args(["check", empty.to_str().unwrap()])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(0));
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("no pyproject.toml found"), "{stderr}");
}

#[test]
fn check_uses_nearest_nested_pyproject() {
    let root = tempfile();
    fs::write(
        root.join("pyproject.toml"),
        r#"
[tool.customs]
src-roots = ["."]

[tool.customs.module.shared]
module = "shared"
"#,
    )
    .unwrap();
    fs::write(root.join("shared.py"), "").unwrap();
    fs::write(root.join("app.py"), "import shared\n").unwrap();

    let pkg = root.join("pkg");
    fs::create_dir_all(&pkg).unwrap();
    fs::write(
        pkg.join("pyproject.toml"),
        "[tool.customs]\nsrc-roots = [\".\"]\n",
    )
    .unwrap();
    fs::write(pkg.join("app.py"), "import shared\n").unwrap();

    let out = customs()
        .args(["check", root.to_str().unwrap()])
        .output()
        .unwrap();
    assert_eq!(
        out.status.code(),
        Some(1),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("app.py") && stdout.contains("[shared]"),
        "{stdout}"
    );
    assert!(
        !stdout.contains("pkg/app.py") && !stdout.contains(&format!("{}/app.py", pkg.display())),
        "nested package should use its own pyproject (no rules): {stdout}"
    );
}

fn tempfile() -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "customs-empty-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("empty.py"), "import os\n").unwrap();
    dir
}
