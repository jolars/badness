import * as vscode from "vscode";
import * as fs from "node:fs/promises";
import * as path from "node:path";
import {
  LanguageClient,
  LanguageClientOptions,
  Middleware,
  ServerOptions,
  Trace,
  vsdiag,
} from "vscode-languageclient/node";
import { resolveBadnessBinary } from "./installer";

let client: LanguageClient | undefined;

function isReleaseTagExplicitlyConfigured(
  config: vscode.WorkspaceConfiguration,
): boolean {
  const value = config.inspect<string>("releaseTag");
  return (
    value?.globalValue !== undefined ||
    value?.workspaceValue !== undefined ||
    value?.workspaceFolderValue !== undefined
  );
}

type ExecutableStrategy = "bundled" | "environment" | "path";

async function findBundledBinary(
  context: vscode.ExtensionContext,
): Promise<string | undefined> {
  const binaryName = process.platform === "win32" ? "badness.exe" : "badness";
  const candidate = path.join(context.extensionPath, "server", binaryName);
  try {
    await fs.access(candidate);
    return candidate;
  } catch {
    return undefined;
  }
}

async function resolveCommandPath(
  context: vscode.ExtensionContext,
  config: vscode.WorkspaceConfiguration,
  outputChannel: vscode.LogOutputChannel,
): Promise<string> {
  const githubRepo = config.get<string>("githubRepo", "jolars/badness");
  const version = config.get<string>("version", "latest");
  const releaseTag = config.get<string>("releaseTag", "latest");
  const releaseTagExplicit = isReleaseTagExplicitlyConfigured(config);
  const selectedRelease = releaseTagExplicit ? releaseTag : version;
  const versionInspect = config.inspect<string>("version");
  const versionPinExplicit =
    releaseTagExplicit ||
    versionInspect?.globalValue !== undefined ||
    versionInspect?.workspaceValue !== undefined ||
    versionInspect?.workspaceFolderValue !== undefined;

  const strategy = config.get<ExecutableStrategy>("executableStrategy", "bundled");
  const executablePath = config.get<string | null>("executablePath", null);

  if (strategy === "path") {
    if (!executablePath || executablePath.trim().length === 0) {
      void vscode.window.showWarningMessage(
        "badness.executableStrategy is set to 'path' but badness.executablePath is empty. Falling back to 'badness' on PATH.",
      );
      return "badness";
    }
    return executablePath;
  }

  if (strategy === "environment") {
    return "badness";
  }

  // strategy === "bundled"
  if (!versionPinExplicit) {
    const bundled = await findBundledBinary(context);
    if (bundled) {
      outputChannel.appendLine(`Using bundled Badness binary at ${bundled}.`);
      return bundled;
    }
  }
  try {
    return await resolveBadnessBinary(
      context.globalStorageUri.fsPath,
      githubRepo,
      selectedRelease,
    );
  } catch (error) {
    const message =
      error instanceof Error ? error.message : "Unknown download error";
    void vscode.window.showWarningMessage(
      `Badness binary download failed (${message}). Falling back to 'badness' on PATH.`,
    );
    return "badness";
  }
}

function mergeServerEnvironment(
  baseEnv: NodeJS.ProcessEnv,
  overrides: Record<string, string>,
  extraPathEntries: string[],
): NodeJS.ProcessEnv {
  const env: NodeJS.ProcessEnv = { ...baseEnv, ...overrides };
  const normalizedExtraPath = extraPathEntries
    .map((entry) => entry.trim())
    .filter((entry) => entry.length > 0);

  if (normalizedExtraPath.length === 0) {
    return env;
  }

  const pathKey =
    process.platform === "win32"
      ? Object.keys(env).find((key) => key.toLowerCase() === "path") ?? "Path"
      : "PATH";

  for (const key of Object.keys(env)) {
    if (key !== pathKey && key.toLowerCase() === "path") {
      delete env[key];
    }
  }

  const existingPath = env[pathKey]?.trim() ?? "";
  env[pathKey] =
    normalizedExtraPath.join(path.delimiter) +
    (existingPath ? `${path.delimiter}${existingPath}` : "");

  return env;
}

/** Whether a `badness.<section>.enable` toggle is on (default on). Read live so
 *  the middleware reflects the current setting without a server round-trip. */
function featureEnabled(section: "formatting" | "diagnostics" | "languageFeatures"): boolean {
  return vscode.workspace
    .getConfiguration("badness")
    .get<boolean>(`${section}.enable`, true);
}

/**
 * Client-side gates for the three `badness.*.enable` toggles. Each request is
 * intercepted before it reaches (or after it leaves) the server: diagnostics can
 * be dropped wholesale (including the parse errors a `badness.toml` cannot mute)
 * while formatting and language features are suppressed by returning nothing, so
 * the server keeps running and the other lanes are untouched.
 */
function buildMiddleware(): Middleware {
  // Run `next` only when the lane is enabled; otherwise contribute nothing.
  const fmt = <T>(run: () => T): T | undefined =>
    featureEnabled("formatting") ? run() : undefined;
  const lf = <T>(run: () => T): T | undefined =>
    featureEnabled("languageFeatures") ? run() : undefined;

  return {
    // Diagnostics: push (publishDiagnostics) and pull (textDocument/diagnostic).
    handleDiagnostics(uri, diagnostics, next) {
      next(uri, featureEnabled("diagnostics") ? diagnostics : []);
    },
    provideDiagnostics(document, previousResultId, token, next) {
      if (!featureEnabled("diagnostics")) {
        return { kind: vsdiag.DocumentDiagnosticReportKind.full, items: [] };
      }
      return next(document, previousResultId, token);
    },

    // Formatting.
    provideDocumentFormattingEdits: (document, options, token, next) =>
      fmt(() => next(document, options, token)),
    provideDocumentRangeFormattingEdits: (document, range, options, token, next) =>
      fmt(() => next(document, range, options, token)),
    provideOnTypeFormattingEdits: (document, position, ch, options, token, next) =>
      fmt(() => next(document, position, ch, options, token)),

    // Language features.
    provideHover: (document, position, token, next) =>
      lf(() => next(document, position, token)),
    provideSignatureHelp: (document, position, context, token, next) =>
      lf(() => next(document, position, context, token)),
    provideCompletionItem: (document, position, context, token, next) =>
      lf(() => next(document, position, context, token)),
    provideDefinition: (document, position, token, next) =>
      lf(() => next(document, position, token)),
    provideReferences: (document, position, options, token, next) =>
      lf(() => next(document, position, options, token)),
    provideDocumentHighlights: (document, position, token, next) =>
      lf(() => next(document, position, token)),
    provideDocumentSymbols: (document, token, next) =>
      lf(() => next(document, token)),
    provideWorkspaceSymbols: (query, token, next) =>
      lf(() => next(query, token)),
    provideCodeActions: (document, range, context, token, next) =>
      lf(() => next(document, range, context, token)),
    provideRenameEdits: (document, position, newName, token, next) =>
      lf(() => next(document, position, newName, token)),
    prepareRename: (document, position, token, next) =>
      lf(() => next(document, position, token)),
    provideFoldingRanges: (document, context, token, next) =>
      lf(() => next(document, context, token)),
    provideSelectionRanges: (document, positions, token, next) =>
      lf(() => next(document, positions, token)),
    provideDocumentLinks: (document, token, next) =>
      lf(() => next(document, token)),
  };
}

async function startClient(
  context: vscode.ExtensionContext,
  outputChannel: vscode.LogOutputChannel,
): Promise<void> {
  const config = vscode.workspace.getConfiguration("badness");
  const commandPath = await resolveCommandPath(context, config, outputChannel);

  const serverArgs = config.get<string[]>("serverArgs", []);
  const userServerEnv = config.get<Record<string, string>>("serverEnv", {});
  const logLevel = config.get<string | null>("logLevel", null);
  const serverEnv: Record<string, string> = {
    ...(logLevel ? { RUST_LOG: logLevel } : {}),
    ...userServerEnv,
  };
  const extraPath = config.get<string[]>("extraPath", []);
  const traceLevel = config.get<"off" | "messages" | "verbose">(
    "trace.server",
    "off",
  );

  const serverOptions: ServerOptions = {
    command: commandPath,
    args: ["lsp", ...serverArgs],
    options: {
      env: mergeServerEnvironment(process.env, serverEnv, extraPath),
    },
  };

  const clientOptions: LanguageClientOptions = {
    documentSelector: [
      { scheme: "file", language: "latex" },
      { scheme: "untitled", language: "latex" },
      { scheme: "file", language: "tex" },
      { scheme: "untitled", language: "tex" },
      { scheme: "file", language: "bibtex" },
      { scheme: "untitled", language: "bibtex" },
      { scheme: "file", pattern: "**/*.tex" },
    ],
    outputChannel,
    traceOutputChannel: outputChannel,
    middleware: buildMiddleware(),
  };

  client = new LanguageClient(
    "badnessLanguageServer",
    "Badness Language Server",
    serverOptions,
    clientOptions,
  );

  try {
    await client.start();
  } catch (error) {
    const message = error instanceof Error ? error.message : String(error);
    outputChannel.appendLine(`Failed to start Badness language server: ${message}`);
    void vscode.window.showErrorMessage(
      `Badness language server failed to start: ${message}`,
    );
    client = undefined;
    return;
  }
  if (traceLevel === "messages") {
    void client.setTrace(Trace.Messages);
  } else if (traceLevel === "verbose") {
    void client.setTrace(Trace.Verbose);
  }
}

async function restartClient(
  context: vscode.ExtensionContext,
  outputChannel: vscode.LogOutputChannel,
): Promise<void> {
  if (client) {
    try {
      await client.stop();
    } catch (error) {
      const message = error instanceof Error ? error.message : String(error);
      outputChannel.appendLine(`Error stopping Badness language server: ${message}`);
    }
    client = undefined;
  }
  await startClient(context, outputChannel);
}

export async function activate(context: vscode.ExtensionContext): Promise<void> {
  const outputChannel = vscode.window.createOutputChannel(
    "Badness Language Server",
    { log: true },
  );
  context.subscriptions.push(outputChannel);

  context.subscriptions.push(
    vscode.commands.registerCommand("badness.restart", () =>
      restartClient(context, outputChannel),
    ),
  );

  // The middleware reads the toggles live, so formatting/languageFeatures need no
  // restart. Diagnostics are pushed, though, so toggling them must flush the
  // already-published set: clear on disable, restart to repopulate on enable.
  context.subscriptions.push(
    vscode.workspace.onDidChangeConfiguration((event) => {
      if (!event.affectsConfiguration("badness.diagnostics")) {
        return;
      }
      if (featureEnabled("diagnostics")) {
        void restartClient(context, outputChannel);
      } else {
        client?.diagnostics?.clear();
      }
    }),
  );

  await startClient(context, outputChannel);
}

export async function deactivate(): Promise<void> {
  if (client) {
    await client.stop();
  }
}
