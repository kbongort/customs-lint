# Customs

VS Code client for the Customs Python import-boundary linter.

The extension starts `customs lsp` from the selected Python interpreter and publishes diagnostics for the active Python file. Install `customs-lint` in that environment (`pip install customs-lint`) so the editor matches CI.

Set `customs.path` only if you need to override the interpreter lookup. Edits are debounced (`customs.lintDebounceMs`, default 300ms).
