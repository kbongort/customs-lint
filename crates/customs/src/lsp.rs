use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use customs_core::{find_pyproject, lint_source, ConfigStore, Violation};
use serde_json::Value;
use tokio::sync::{Mutex, RwLock};
use tower_lsp_server::jsonrpc::Result;
use tower_lsp_server::ls_types::{
    Diagnostic, DiagnosticSeverity, DidChangeTextDocumentParams, DidCloseTextDocumentParams,
    DidOpenTextDocumentParams, DidSaveTextDocumentParams, InitializeParams, InitializeResult,
    InitializedParams, MessageType, NumberOrString, Position, PositionEncodingKind, Range,
    ServerCapabilities, TextDocumentSyncCapability, TextDocumentSyncKind, Uri,
};
use tower_lsp_server::{Client, LanguageServer, LspService, Server};

const DEFAULT_DEBOUNCE_MS: u64 = 300;

struct Document {
    path: PathBuf,
    text: String,
}

struct Backend {
    client: Client,
    documents: RwLock<HashMap<Uri, Document>>,
    configs: Mutex<ConfigStore>,
    debounce_ms: Mutex<u64>,
    pending: Mutex<HashMap<Uri, tokio::task::JoinHandle<()>>>,
}

impl Backend {
    fn new(client: Client) -> Self {
        Self {
            client,
            documents: RwLock::new(HashMap::new()),
            configs: Mutex::new(ConfigStore::new()),
            debounce_ms: Mutex::new(DEFAULT_DEBOUNCE_MS),
            pending: Mutex::new(HashMap::new()),
        }
    }

    async fn lint_uri(&self, uri: &Uri) {
        let Some((path, text)) = self.document_snapshot(uri).await else {
            return;
        };
        let label = path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| path.display().to_string());
        self.client
            .log_message(MessageType::INFO, format!("Linting {label}"))
            .await;
        let diagnostics = self.diagnostics_for(&path, &text).await;
        self.client
            .log_message(
                MessageType::INFO,
                format!("Finished linting {label}: {} issue(s)", diagnostics.len()),
            )
            .await;
        self.client
            .publish_diagnostics(uri.clone(), diagnostics, None)
            .await;
    }

    async fn document_snapshot(&self, uri: &Uri) -> Option<(PathBuf, String)> {
        let docs = self.documents.read().await;
        let doc = docs.get(uri)?;
        Some((doc.path.clone(), doc.text.clone()))
    }

    async fn diagnostics_for(&self, path: &Path, text: &str) -> Vec<Diagnostic> {
        let Some(pyproject) = find_pyproject(path) else {
            return Vec::new();
        };
        let loaded = {
            let mut store = self.configs.lock().await;
            store.get(&pyproject)
        };
        let Some(loaded) = loaded else {
            return Vec::new();
        };
        let config = match loaded {
            Ok(cfg) => cfg,
            Err(err) => {
                self.client
                    .show_message(MessageType::ERROR, format!("Customs config error: {err}"))
                    .await;
                return Vec::new();
            }
        };
        let project_root = pyproject.parent().unwrap_or(Path::new("."));
        lint_source(path, text, project_root, &config)
            .into_iter()
            .map(|v| violation_to_diagnostic(&v, text))
            .collect()
    }

    async fn schedule_lint(self: &Arc<Self>, uri: Uri, debounce: bool) {
        let delay = if debounce {
            *self.debounce_ms.lock().await
        } else {
            0
        };
        let mut pending = self.pending.lock().await;
        if let Some(handle) = pending.remove(&uri) {
            handle.abort();
        }
        let backend = Arc::clone(self);
        let key = uri.clone();
        let handle = tokio::spawn(async move {
            if delay > 0 {
                tokio::time::sleep(Duration::from_millis(delay)).await;
            }
            backend.lint_uri(&uri).await;
        });
        pending.insert(key, handle);
    }
}

fn violation_to_diagnostic(v: &Violation, source: &str) -> Diagnostic {
    Diagnostic {
        range: Range {
            start: point_to_position(source, v.start.row, v.start.column),
            end: point_to_position(source, v.end.row, v.end.column),
        },
        severity: Some(DiagnosticSeverity::ERROR),
        source: Some("customs".to_string()),
        message: format!("Forbidden import of {}", v.controlled_module),
        code: Some(NumberOrString::String(v.rule_name.clone())),
        code_description: None,
        related_information: None,
        tags: None,
        data: None,
    }
}

fn point_to_position(source: &str, row: usize, byte_col: usize) -> Position {
    let line = source.lines().nth(row).unwrap_or("");
    let byte_col = byte_col.min(line.len());
    // Avoid splitting a UTF-8 code point if tree-sitter handed a mid-char column.
    let byte_col = if line.is_char_boundary(byte_col) {
        byte_col
    } else {
        let mut c = byte_col;
        while c > 0 && !line.is_char_boundary(c) {
            c -= 1;
        }
        c
    };
    let character = line[..byte_col].encode_utf16().count() as u32;
    Position {
        line: row as u32,
        character,
    }
}

fn uri_to_path(uri: &Uri) -> Option<PathBuf> {
    uri.to_file_path().map(|p| p.into_owned())
}

struct BackendService(Arc<Backend>);

impl BackendService {
    fn new(client: Client) -> Self {
        Self(Arc::new(Backend::new(client)))
    }
}

impl LanguageServer for BackendService {
    async fn initialize(&self, params: InitializeParams) -> Result<InitializeResult> {
        let debounce = debounce_from_init(&params).unwrap_or(DEFAULT_DEBOUNCE_MS);
        *self.0.debounce_ms.lock().await = debounce;
        Ok(InitializeResult {
            capabilities: ServerCapabilities {
                text_document_sync: Some(TextDocumentSyncCapability::Kind(
                    TextDocumentSyncKind::FULL,
                )),
                position_encoding: Some(PositionEncodingKind::UTF16),
                ..Default::default()
            },
            ..Default::default()
        })
    }

    async fn initialized(&self, _: InitializedParams) {
        self.0
            .client
            .log_message(MessageType::INFO, "Customs language server ready")
            .await;
    }

    async fn shutdown(&self) -> Result<()> {
        Ok(())
    }

    async fn did_open(&self, params: DidOpenTextDocumentParams) {
        let uri = params.text_document.uri;
        let Some(path) = uri_to_path(&uri) else {
            return;
        };
        {
            let mut docs = self.0.documents.write().await;
            docs.insert(
                uri.clone(),
                Document {
                    path,
                    text: params.text_document.text,
                },
            );
        }
        self.0.schedule_lint(uri, false).await;
    }

    async fn did_change(&self, params: DidChangeTextDocumentParams) {
        let uri = params.text_document.uri;
        if let Some(change) = params.content_changes.into_iter().next() {
            let mut docs = self.0.documents.write().await;
            if let Some(doc) = docs.get_mut(&uri) {
                doc.text = change.text;
            }
        }
        self.0.schedule_lint(uri, true).await;
    }

    async fn did_save(&self, params: DidSaveTextDocumentParams) {
        if let Some(text) = params.text {
            let mut docs = self.0.documents.write().await;
            if let Some(doc) = docs.get_mut(&params.text_document.uri) {
                doc.text = text;
            }
        }
        self.0.schedule_lint(params.text_document.uri, false).await;
    }

    async fn did_close(&self, params: DidCloseTextDocumentParams) {
        let uri = params.text_document.uri;
        {
            let mut pending = self.0.pending.lock().await;
            if let Some(handle) = pending.remove(&uri) {
                handle.abort();
            }
        }
        self.0.documents.write().await.remove(&uri);
        self.0
            .client
            .publish_diagnostics(uri, Vec::new(), None)
            .await;
    }
}

fn debounce_from_init(params: &InitializeParams) -> Option<u64> {
    let options = params.initialization_options.as_ref()?;
    match options {
        Value::Object(map) => map
            .get("lintDebounceMs")
            .and_then(Value::as_u64)
            .or_else(|| {
                map.get("lintDebounceMs")
                    .and_then(Value::as_i64)
                    .map(|n| n as u64)
            }),
        _ => None,
    }
}

pub fn run() -> anyhow::Result<()> {
    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?;
    rt.block_on(async {
        let stdin = tokio::io::stdin();
        let stdout = tokio::io::stdout();
        let (service, socket) = LspService::new(BackendService::new);
        Server::new(stdin, stdout, socket).serve(service).await;
        Ok(())
    })
}
