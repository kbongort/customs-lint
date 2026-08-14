# Customs

A Rust lint tool that enforces Python import boundaries declared in `pyproject.toml`.

```toml
[tool.customs]
src-roots = ["src", "."]
ignore = ["examples", "tests"]

[tool.customs.module.my-service]
module = "my_project.apps.service"

[tool.customs.module.libraries-utils]
module = "my_project.libraries.utils"
allow = ["$submodules", "my_project.apps.service"]
```

## Install

```bash
uv tool install customs-lint
# or: pip install customs-lint
customs check .
```

The VS Code extension starts `customs lsp` from the selected Python interpreter (the same `customs-lint` install you use for CI) and lints the active Python file on edit.

## Commands

- `customs check [paths...]` — lint files; exit `0` if clean, `1` on violations, `2` on tool errors
- `customs lsp` — Language Server Protocol server on stdio

## Configuration

Put a `[tool.customs]` table in `pyproject.toml`. Customs walks up from each file to the nearest `pyproject.toml`.

**`src-roots`** (default `["."]`) — directories relative to that file, used to map a path to a module. The longest matching root wins. With `["src", "."]`, `src/foo/bar.py` is `foo.bar`, not `src.foo.bar`.

**`ignore`** — module prefixes of *importing* files to skip. `tests` skips `tests` and `tests.test_foo`, but not `testsuite`.

**`[tool.customs.module.<rule-name>]`** — a named rule. `<rule-name>` is your label; it appears in diagnostics (for example `[my-service]`).

- **`module`** (required) — the controlled package. Imports of this module and its submodules are restricted.
- **`allow`** — who may import that tree. Each entry is a module prefix (the named module and its submodules). `$submodules` means the controlled module and its own children. If `allow` is omitted, it defaults to `["$submodules"]`. If you set `allow` yourself, `$submodules` is not implied; list it to keep internal imports allowed.

## Development

### System dependencies

You must have the following dependencies installed:

- **Rust and Cargo** — stable toolchain, rustc 1.77 or newer. Install via [rustup](https://rustup.rs).
- **[uv](https://docs.astral.sh/uv/)** — Python package and environment manager (installs Python 3.12 from `.python-version` when needed).
- **Node.js 20+** and npm to build the VS Code extension.

### Run tests

From the repo root, with Rust and Cargo on `PATH`:

```bash
cargo test --workspace
```

This runs the `customs-core` unit tests and the `customs check` integration tests against `tests/fixtures/`. To run one crate or one test:

```bash
cargo test -p customs-core
cargo test -p customs
cargo test -p customs-core longest_prefix_wins
```

### Set up a virtual environment and build the wheel

From the repo root (Rust/Cargo must be on `PATH`):

```bash
uv sync --group dev
uv build --out-dir target/wheels
```

`uv sync` creates `.venv`, installs the locked `dev` group (maturin), and builds/installs `customs` into that environment. Wheels from `uv build` are written to `target/wheels/`.

### Install the wheel into another environment

The distribution name is `customs-lint`; the installed executable is `customs`.

```bash
uv pip install target/wheels/customs_lint-*.whl
```

### Invoke the CLI

Once the wheel is installed and the environment is active:

```bash
customs check .
customs check path/to/file.py
customs lsp   # stdio language server
```

`customs check` exits `0` if clean, `1` if it found forbidden imports, and `2` on tool or config errors.

### Build the VS Code extension

The extension does not bundle `customs`. Install `customs-lint` in the workspace interpreter (or set `customs.path`). From the repo root:

```bash
cd editors/vscode
npm ci
npm run package
```

`npm run package` runs `vsce package` and produces `editors/vscode/customs-0.1.0.vsix` (version from `package.json`).

For local iteration without a VSIX, run `npm run compile` in `editors/vscode`.

### Install the VS Code extension

```bash
code --install-extension editors/vscode/customs-0.1.0.vsix
```

Or use the Command Palette: **Extensions: Install from VSIX…** and select the `.vsix` file.

After install, open a Python file and select the project interpreter (the Python extension is required). The extension runs `customs lsp` from that environment, or from `customs.path` if you set it. If `customs` is missing, it reports an error and asks you to install `customs-lint` in that environment (`uv pip install customs-lint` or `pip install customs-lint`).
