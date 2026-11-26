// The Thepia Agent Config VS Code extension (WORK-0046 #6, ST-13).
//
// A thin LSP client: it spawns `thepiad --lsp` over stdio and routes SKILL.md and agent-definition
// files to it, so the-skills / the-agents lint findings show up as live editor diagnostics. All the
// language-server logic lives in the daemon (crates/the-daemon/src/lsp.rs); this is only the VS Code
// transport glue. Under Remote-SSH it works unchanged: vscode-server runs the extension host on the
// remote node, so `thepiad` is launched there, next to the files it lints.

import * as vscode from "vscode";
import {
  LanguageClient,
  LanguageClientOptions,
  ServerOptions,
  TransportKind,
} from "vscode-languageclient/node";

let client: LanguageClient | undefined;

function makeClient(): LanguageClient {
  const cfg = vscode.workspace.getConfiguration("thepia.lsp");
  const command = cfg.get<string>("serverPath", "thepiad");
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
