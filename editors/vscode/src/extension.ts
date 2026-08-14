import * as fs from "fs";
import * as path from "path";
import * as vscode from "vscode";
import {
  LanguageClient,
  LanguageClientOptions,
  RevealOutputChannelOn,
  ServerOptions,
} from "vscode-languageclient/node";

const PYTHON_EXTENSION_ID = "ms-python.python";
const INSTALL_HINT = "pip install customs-lint";

let client: LanguageClient | undefined;
let output: vscode.OutputChannel;
let status: vscode.StatusBarItem;
let starting = false;

interface PythonEnvironmentPath {
  id: string;
  path: string;
}

interface PythonResolvedEnvironment {
  executable?: {
    uri?: vscode.Uri;
    sysPrefix?: string;
  };
}

interface PythonEnvironmentsApi {
  getActiveEnvironmentPath(
    resource?: vscode.Uri | vscode.WorkspaceFolder,
  ): PythonEnvironmentPath;
  resolveEnvironment(
    path: string | PythonEnvironmentPath,
  ): Thenable<PythonResolvedEnvironment | undefined>;
  onDidChangeActiveEnvironmentPath: vscode.Event<unknown>;
}

interface PythonExtensionApi {
  environments: PythonEnvironmentsApi;
}

function customsExecutableName(): string {
  return process.platform === "win32" ? "customs.exe" : "customs";
}

function workspaceResource(): vscode.Uri | vscode.WorkspaceFolder | undefined {
  const active = vscode.window.activeTextEditor?.document.uri;
  if (active) {
    return vscode.workspace.getWorkspaceFolder(active) ?? active;
  }
  return vscode.workspace.workspaceFolders?.[0];
}

async function getPythonApi(): Promise<PythonExtensionApi | undefined> {
  const ext = vscode.extensions.getExtension<PythonExtensionApi>(
    PYTHON_EXTENSION_ID,
  );
  if (!ext) {
    return undefined;
  }
  if (!ext.isActive) {
    return ext.activate();
  }
  return ext.exports;
}

function existingFile(candidate: string): string | undefined {
  try {
    if (fs.existsSync(candidate) && fs.statSync(candidate).isFile()) {
      return candidate;
    }
  } catch {
    return undefined;
  }
  return undefined;
}

function customsBesideInterpreter(
  pythonPath: string,
  sysPrefix?: string,
): string | undefined {
  const exe = customsExecutableName();
  const candidates = [
    path.join(path.dirname(pythonPath), exe),
    sysPrefix ? path.join(sysPrefix, "bin", exe) : undefined,
    sysPrefix ? path.join(sysPrefix, "Scripts", exe) : undefined,
  ];
  for (const candidate of candidates) {
    if (candidate) {
      const found = existingFile(candidate);
      if (found) {
        return found;
      }
    }
  }
  return undefined;
}

async function resolveFromInterpreter(): Promise<{
  command?: string;
  detail: string;
}> {
  const api = await getPythonApi();
  if (!api?.environments) {
    return {
      detail:
        "The Python extension is not available. Install ms-python.python and select an interpreter.",
    };
  }

  const resource = workspaceResource();
  const envPath = api.environments.getActiveEnvironmentPath(resource);
  const resolved = await api.environments.resolveEnvironment(envPath);
  const pythonPath =
    resolved?.executable?.uri?.fsPath ??
    (envPath.path && !envPath.path.startsWith("env:")
      ? envPath.path
      : undefined);

  if (!pythonPath) {
    return {
      detail:
        "No Python interpreter is selected. Use the Python: Select Interpreter command, then install customs-lint in that environment.",
    };
  }

  const command = customsBesideInterpreter(
    pythonPath,
    resolved?.executable?.sysPrefix,
  );
  if (!command) {
    return {
      detail: `No ${customsExecutableName()} executable in the selected environment (${pythonPath}). ${INSTALL_HINT}`,
    };
  }
  return { command, detail: `Using interpreter ${pythonPath}` };
}

async function resolveServer(): Promise<{ command?: string; detail: string }> {
  const configured = vscode.workspace
    .getConfiguration("customs")
    .get<string>("path")
    ?.trim();
  if (configured) {
    const found = existingFile(configured);
    if (!found) {
      return {
        detail: `customs.path is set to ${configured}, but that file does not exist.`,
      };
    }
    return { command: found, detail: "Using customs.path" };
  }
  return resolveFromInterpreter();
}

function clientOptions(): LanguageClientOptions {
  const debounce =
    vscode.workspace.getConfiguration("customs").get<number>("lintDebounceMs") ??
    300;
  return {
    documentSelector: [{ scheme: "file", language: "python" }],
    initializationOptions: { lintDebounceMs: debounce },
    outputChannel: output,
    revealOutputChannelOn: RevealOutputChannelOn.Never,
    synchronize: {
      configurationSection: "customs",
    },
    middleware: {
      handleDiagnostics: (uri, diagnostics, next) => {
        const file = path.basename(uri.fsPath);
        const count = diagnostics.length;
        output.appendLine(`Diagnostics for ${uri.fsPath}: ${count} issue(s)`);
        status.text =
          count > 0
            ? `Customs: ${count} issue${count === 1 ? "" : "s"} in ${file}`
            : "Customs";
        next(uri, diagnostics);
      },
    },
  };
}

async function stopClient(): Promise<void> {
  if (!client) {
    return;
  }
  const current = client;
  client = undefined;
  await current.stop();
}

async function startClient(): Promise<void> {
  if (starting) {
    return;
  }
  starting = true;
  try {
    await stopClient();
    const resolved = await resolveServer();
    output.appendLine(resolved.detail);
    if (!resolved.command) {
      output.appendLine("Language server not started.");
      status.text = "Customs: not found";
      status.tooltip = resolved.detail;
      status.show();
      void vscode.window.showErrorMessage(`Customs: ${resolved.detail}`);
      return;
    }

    const serverOptions: ServerOptions = {
      run: { command: resolved.command, args: ["lsp"] },
      debug: { command: resolved.command, args: ["lsp"] },
    };

    output.appendLine(`Starting language server: ${resolved.command} lsp`);
    status.text = "$(sync~spin) Customs: starting language server…";
    status.tooltip = "Customs import-boundary linter";
    status.show();

    client = new LanguageClient(
      "customs",
      "Customs",
      serverOptions,
      clientOptions(),
    );
    await client.start();

    output.appendLine("Language server started.");
    status.text = "Customs";
    status.tooltip = `Customs language server: ${resolved.command}`;
  } finally {
    starting = false;
  }
}

export async function activate(context: vscode.ExtensionContext): Promise<void> {
  output = vscode.window.createOutputChannel("Customs");
  status = vscode.window.createStatusBarItem(vscode.StatusBarAlignment.Left, 100);
  status.name = "Customs";
  status.command = "customs.showOutput";
  context.subscriptions.push(output, status);

  context.subscriptions.push(
    vscode.commands.registerCommand("customs.showOutput", () => {
      output.show(true);
    }),
  );

  const python = await getPythonApi();
  if (python?.environments.onDidChangeActiveEnvironmentPath) {
    context.subscriptions.push(
      python.environments.onDidChangeActiveEnvironmentPath(() => {
        void startClient();
      }),
    );
  }

  context.subscriptions.push(
    vscode.workspace.onDidChangeConfiguration((event) => {
      if (
        event.affectsConfiguration("customs.path") ||
        event.affectsConfiguration("customs.lintDebounceMs")
      ) {
        void startClient();
      }
    }),
  );

  context.subscriptions.push({
    dispose: () => {
      void stopClient();
    },
  });

  await startClient();
}

export async function deactivate(): Promise<void> {
  await stopClient();
}
