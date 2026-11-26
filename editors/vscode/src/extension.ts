// The Thepia Agent Config VS Code extension (WORK-0046 #6, ST-13).
//
// A thin LSP client: it spawns `thepiad --lsp` over stdio and routes SKILL.md and agent-definition
// files to it, so the-skills / the-agents lint findings show up as live editor diagnostics. All the
// language-server logic lives in the daemon (crates/the-daemon/src/lsp.rs); this is only the VS Code
// transport glue. Under Remote-SSH it works unchanged: vscode-server runs the extension host on the
// remote node, so `thepiad` is launched there, next to the files it lints.

import * as fs from "fs";
import * as os from "os";
import * as path from "path";
import * as vscode from "vscode";
import {
  LanguageClient,
  LanguageClientOptions,
  ServerOptions,
  TransportKind,
} from "vscode-languageclient/node";

let client: LanguageClient | undefined;

// Resolve the server command. `thepia use +language-server` installs thepiad to ~/.thepia/bin, which
// is not necessarily on a GUI VS Code's PATH, so when the setting is left at the default `thepiad` we
// prefer that known install path and fall back to a PATH lookup. An explicit serverPath is honored
// verbatim (with a leading `~` expanded).
function resolveCommand(configured: string): string {
  if (configured !== "thepiad") {
    return configured.startsWith("~/")
      ? path.join(os.homedir(), configured.slice(2))
      : configured;
  }
  const installed = path.join(os.homedir(), ".thepia", "bin", "thepiad");
  return fs.existsSync(installed) ? installed : "thepiad";
}

function makeClient(): LanguageClient {
  const cfg = vscode.workspace.getConfiguration("thepia.lsp");
  const command = resolveCommand(cfg.get<string>("serverPath", "thepiad"));
  const args = cfg.get<string[]>("serverArgs", ["--lsp"]);

  // One server process, speaking stdio. `the` lint engines are synchronous, so no special env is needed.
  const serverOptions: ServerOptions = {
    run: { command, args, transport: TransportKind.stdio },
    debug: { command, args, transport: TransportKind.stdio },
  };

  // Scope to the files the two engines actually govern: SKILL.md anywhere, and Claude agent definitions
  // under `.claude/agents/`. Arbitrary markdown is deliberately NOT matched (it is not an agent config).
  const documentSelector = [
    { scheme: "file", pattern: "**/SKILL.md" },
    { scheme: "file", pattern: "**/.claude/agents/**/*.md" },
  ];

  const clientOptions: LanguageClientOptions = {
    documentSelector,
    synchronize: {
      configurationSection: "thepia.lsp",
    },
  };

  return new LanguageClient(
    "thepiaAgentConfig",
    "Thepia Agent Config",
    serverOptions,
    clientOptions,
  );
}

export async function activate(_context: vscode.ExtensionContext): Promise<void> {
  client = makeClient();
  try {
    await client.start();
  } catch (err) {
    const cfg = vscode.workspace.getConfiguration("thepia.lsp");
    const path = cfg.get<string>("serverPath", "thepiad");
    vscode.window.showErrorMessage(
      `Thepia Agent Config: could not start the language server (\`${path} --lsp\`). ` +
        `Set \`thepia.lsp.serverPath\` to your thepiad binary. (${err})`,
    );
  }
}

export async function deactivate(): Promise<void> {
  if (client) {
    await client.stop();
    client = undefined;
  }
}
