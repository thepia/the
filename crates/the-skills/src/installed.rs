//! Discovery of Coding Agent CLIs installed on the machine (`claude`, `gemini`, `opencode`) and a
//! strategy pattern for building and running commands against each one.
//!
//! Each agent has a distinct CLI surface for version checks, zero-API skill enumeration (where the
//! binary supports it), and headless single-turn prompts:
//!
//! - **Claude Code (`claude`)**: no zero-API skill-list subcommand; headless prompt via
//!   `claude --setting-sources project --model <model> -p <prompt>`, defaulting to `haiku`.
//! - **Gemini CLI (`gemini`)**: zero-API skill list via `gemini skills list`; headless prompt via
//!   `gemini --skip-trust -m <model> -p <prompt>`, defaulting to `gemini-2.5-flash-lite` and
//!   stripping ambient `GOOGLE_CLOUD_PROJECT[_ID]` so personal OAuth does not fail on disabled
//!   `cloudcode-pa.googleapis.com`.
//! - **OpenCode (`opencode`)**: zero-API skill dump via `opencode debug skill` (JSON array on
//!   stdout); headless prompt via `opencode run [-m <model>] <prompt>`.

#![allow(dead_code)]

use std::env;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use clap::ValueEnum;
use serde::Serialize;

use super::locate::Env;

/// Known agent CLIs that `installed` scans for and drives.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, ValueEnum)]
#[serde(rename_all = "lowercase")]
pub enum AgentKind {
    /// Claude Code (`claude`).
    Claude,
    /// Gemini CLI (`gemini`).
    Gemini,
    /// OpenCode (`opencode`).
    Opencode,
    /// Goose (`goose`).
    Goose,
}

impl AgentKind {
    pub const ALL: [AgentKind; 4] = [
        AgentKind::Claude,
        AgentKind::Gemini,
        AgentKind::Opencode,
        AgentKind::Goose,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            AgentKind::Claude => "claude",
            AgentKind::Gemini => "gemini",
            AgentKind::Opencode => "opencode",
            AgentKind::Goose => "goose",
        }
    }

    pub fn binary_name(self) -> &'static str {
        self.as_str()
    }

    pub fn to_skill_agent(self) -> super::Agent {
        match self {
            AgentKind::Claude => super::Agent::Claude,
            AgentKind::Gemini => super::Agent::Gemini,
            AgentKind::Opencode => super::Agent::Opencode,
            AgentKind::Goose => super::Agent::Goose,
        }
    }

    /// Infer the agent from a model identifier (e.g. `haiku`, `sonnet`, `gemini-2.5-flash`).
    pub fn from_model_hint(model: &str) -> Option<Self> {
        let m = model.trim().to_ascii_lowercase();
        if m.starts_with("gemini") || m == "flash" || m == "flash-lite" {
            Some(AgentKind::Gemini)
        } else if matches!(m.as_str(), "haiku" | "sonnet" | "opus") || m.starts_with("claude") {
            Some(AgentKind::Claude)
        } else if m.contains('/') {
            // OpenCode uses `<provider>/<model>` identifiers.
            Some(AgentKind::Opencode)
        } else {
            None
        }
    }

    /// Return the strategy implementation for this agent kind.
    pub fn strategy(self) -> &'static dyn AgentStrategy {
        match self {
            AgentKind::Claude => &ClaudeStrategy,
            AgentKind::Gemini => &GeminiStrategy,
            AgentKind::Opencode => &OpencodeStrategy,
            AgentKind::Goose => &GooseStrategy,
        }
    }
}

/// A pure, inspectable specification of a child process before it is spawned.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommandSpec {
    pub program: PathBuf,
    pub args: Vec<String>,
    pub cwd: Option<PathBuf>,
    pub env_set: Vec<(String, String)>,
    pub env_remove: Vec<String>,
}

impl CommandSpec {
    pub fn new(program: impl Into<PathBuf>) -> Self {
        Self {
            program: program.into(),
            args: Vec::new(),
            cwd: None,
            env_set: Vec::new(),
            env_remove: Vec::new(),
        }
    }

    pub fn arg(mut self, a: impl Into<String>) -> Self {
        self.args.push(a.into());
        self
    }

    pub fn args<I, S>(mut self, iter: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        self.args.extend(iter.into_iter().map(Into::into));
        self
    }

    pub fn cwd(mut self, dir: impl Into<PathBuf>) -> Self {
        self.cwd = Some(dir.into());
        self
    }

    pub fn env(mut self, key: impl Into<String>, val: impl Into<String>) -> Self {
        self.env_set.push((key.into(), val.into()));
        self
    }

    pub fn env_remove(mut self, key: impl Into<String>) -> Self {
        self.env_remove.push(key.into());
        self
    }

    /// Convert into a `std::process::Command` ready to execute.
    pub fn to_command(&self) -> Command {
        let mut cmd = Command::new(&self.program);
        cmd.args(&self.args);
        if let Some(dir) = &self.cwd {
            cmd.current_dir(dir);
        }
        for k in &self.env_remove {
            cmd.env_remove(k);
        }
        for (k, v) in &self.env_set {
            cmd.env(k, v);
        }
        cmd
    }

    /// Execute the command and capture its output, classifying quota/rate limits if present.
    pub fn run(&self) -> io::Result<CommandOutput> {
        let out: Output = self.to_command().output()?;
        let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
        let stderr = String::from_utf8_lossy(&out.stderr).into_owned();
        let combined = if stderr.is_empty() {
            stdout.clone()
        } else if stdout.is_empty() {
            stderr.clone()
        } else {
            format!("{}\n{}", stderr.trim(), stdout.trim())
        };
        let quota_limit = detect_quota_limit(&combined);
        Ok(CommandOutput {
            status: out.status.code().unwrap_or(-1),
            stdout,
            stderr,
            quota_limit,
        })
    }
}

/// Captured output from running a [`CommandSpec`], with rate/quota limit detection attached.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CommandOutput {
    pub status: i32,
    pub stdout: String,
    pub stderr: String,
    pub quota_limit: Option<QuotaLimit>,
}

impl CommandOutput {
    pub fn succeeded(&self) -> bool {
        self.status == 0 && self.quota_limit.is_none()
    }
}

/// Rate or allocation limit reported by an agent CLI.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct QuotaLimit {
    pub message: String,
    pub reset: Option<String>,
}

/// Parameters for a headless single-turn prompt invocation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PromptRequest {
    pub prompt: String,
    pub model: Option<String>,
    pub cwd: Option<PathBuf>,
}

impl PromptRequest {
    pub fn new(prompt: impl Into<String>) -> Self {
        Self {
            prompt: prompt.into(),
            model: None,
            cwd: None,
        }
    }

    pub fn model(mut self, model: impl Into<String>) -> Self {
        self.model = Some(model.into());
        self
    }

    pub fn cwd(mut self, cwd: impl Into<PathBuf>) -> Self {
        self.cwd = Some(cwd.into());
        self
    }
}

/// Strategy interface for building commands and parsing outputs for a specific agent CLI.
pub trait AgentStrategy: Sync {
    fn kind(&self) -> AgentKind;

    /// Cheapest default model for headless evaluation calls, if the agent requires or prefers one.
    fn default_model(&self) -> Option<&'static str>;

    /// Candidate fallback paths under the user's home directory when the binary is not on `PATH`.
    fn home_fallbacks(&self, home: &Path) -> Vec<PathBuf>;

    /// Command to check `--version`.
    fn version_cmd(&self, binary: &Path) -> CommandSpec {
        CommandSpec::new(binary).arg("--version")
    }

    /// Zero-API command to list skills discovered from `cwd`, if the CLI supports one natively.
    fn list_skills_cmd(&self, binary: &Path, cwd: &Path) -> Option<CommandSpec>;

    /// Parse skill names from the stdout of [`Self::list_skills_cmd`].
    fn parse_listed_skills(&self, stdout: &str) -> Vec<String>;

    /// Headless non-interactive prompt command.
    fn prompt_cmd(&self, binary: &Path, req: &PromptRequest) -> CommandSpec;
}

/// Strategy for Claude Code (`claude`).
#[derive(Debug, Clone, Copy)]
pub struct ClaudeStrategy;

impl AgentStrategy for ClaudeStrategy {
    fn kind(&self) -> AgentKind {
        AgentKind::Claude
    }

    fn default_model(&self) -> Option<&'static str> {
        Some("haiku")
    }

    fn home_fallbacks(&self, home: &Path) -> Vec<PathBuf> {
        vec![
            home.join(".claude/local/claude"),
            home.join(".local/bin/claude"),
        ]
    }

    fn list_skills_cmd(&self, _binary: &Path, _cwd: &Path) -> Option<CommandSpec> {
        // Claude Code has no local `skills list` subcommand without starting a prompt session.
        None
    }

    fn parse_listed_skills(&self, _stdout: &str) -> Vec<String> {
        Vec::new()
    }

    fn prompt_cmd(&self, binary: &Path, req: &PromptRequest) -> CommandSpec {
        let model = req
            .model
            .as_deref()
            .filter(|m| !m.is_empty())
            .or_else(|| self.default_model())
            .unwrap_or("haiku");
        let mut spec = CommandSpec::new(binary).args([
            "--setting-sources",
            "project",
            "--model",
            model,
            "-p",
            &req.prompt,
        ]);
        if let Some(dir) = &req.cwd {
            spec = spec.cwd(dir);
        }
        spec
    }
}

/// Strategy for Gemini CLI (`gemini`).
#[derive(Debug, Clone, Copy)]
pub struct GeminiStrategy;

impl AgentStrategy for GeminiStrategy {
    fn kind(&self) -> AgentKind {
        AgentKind::Gemini
    }

    fn default_model(&self) -> Option<&'static str> {
        Some("gemini-2.5-flash-lite")
    }

    fn home_fallbacks(&self, home: &Path) -> Vec<PathBuf> {
        vec![home.join(".local/bin/gemini")]
    }

    fn version_cmd(&self, binary: &Path) -> CommandSpec {
        CommandSpec::new(binary)
            .arg("--version")
            .env("GEMINI_CLI_HOME", env::temp_dir().to_string_lossy())
    }

    fn list_skills_cmd(&self, binary: &Path, cwd: &Path) -> Option<CommandSpec> {
        Some(
            CommandSpec::new(binary)
                .args(["skills", "list"])
                .cwd(cwd)
                .env_remove("GOOGLE_CLOUD_PROJECT")
                .env_remove("GOOGLE_CLOUD_PROJECT_ID"),
        )
    }

    fn parse_listed_skills(&self, stdout: &str) -> Vec<String> {
        let mut out = Vec::new();
        for line in stdout.lines() {
            let trimmed = line.trim();
            if let Some((name, rest)) = trimmed.split_once(" [")
                && rest.ends_with(']')
                && !name.is_empty()
                && !name.contains(' ')
            {
                out.push(name.to_owned());
            }
        }
        out.sort();
        out.dedup();
        out
    }

    fn prompt_cmd(&self, binary: &Path, req: &PromptRequest) -> CommandSpec {
        let model = match req.model.as_deref().filter(|m| !m.is_empty()) {
            Some("flash") => "gemini-2.5-flash",
            Some("flash-lite") => "gemini-2.5-flash-lite",
            Some(m) => m,
            None => self.default_model().unwrap_or("gemini-2.5-flash-lite"),
        };
        let mut spec = CommandSpec::new(binary)
            .args(["--skip-trust", "-m", model, "-p", &req.prompt])
            .env_remove("GOOGLE_CLOUD_PROJECT")
            .env_remove("GOOGLE_CLOUD_PROJECT_ID");
        if let Some(dir) = &req.cwd {
            spec = spec.cwd(dir);
        }
        spec
    }
}

/// Strategy for OpenCode (`opencode`).
#[derive(Debug, Clone, Copy)]
pub struct OpencodeStrategy;

impl AgentStrategy for OpencodeStrategy {
    fn kind(&self) -> AgentKind {
        AgentKind::Opencode
    }

    fn default_model(&self) -> Option<&'static str> {
        None
    }

    fn home_fallbacks(&self, home: &Path) -> Vec<PathBuf> {
        vec![
            home.join(".opencode/bin/opencode"),
            home.join(".bun/bin/opencode"),
            home.join(".local/bin/opencode"),
        ]
    }

    fn list_skills_cmd(&self, binary: &Path, cwd: &Path) -> Option<CommandSpec> {
        Some(CommandSpec::new(binary).args(["debug", "skill"]).cwd(cwd))
    }

    fn parse_listed_skills(&self, stdout: &str) -> Vec<String> {
        let mut out = Vec::new();
        if let Some(idx) = stdout.find('[')
            && let Ok(serde_json::Value::Array(items)) = serde_json::from_str(&stdout[idx..])
        {
            for item in items {
                if let Some(name) = item.get("name").and_then(serde_json::Value::as_str)
                    && !name.is_empty()
                {
                    out.push(name.to_owned());
                }
            }
        }
        out.sort();
        out.dedup();
        out
    }

    fn prompt_cmd(&self, binary: &Path, req: &PromptRequest) -> CommandSpec {
        let mut spec = CommandSpec::new(binary).arg("run");
        if let Some(m) = req.model.as_deref().filter(|m| !m.is_empty()) {
            spec = spec.args(["-m", m]);
        }
        spec = spec.arg(&req.prompt);
        if let Some(dir) = &req.cwd {
            spec = spec.cwd(dir);
        }
        spec
    }
}

/// Strategy for Goose (`goose`).
#[derive(Debug, Clone, Copy)]
pub struct GooseStrategy;

impl AgentStrategy for GooseStrategy {
    fn kind(&self) -> AgentKind {
        AgentKind::Goose
    }

    fn default_model(&self) -> Option<&'static str> {
        None
    }

    fn home_fallbacks(&self, home: &Path) -> Vec<PathBuf> {
        vec![home.join(".local/bin/goose")]
    }

    fn list_skills_cmd(&self, _binary: &Path, _cwd: &Path) -> Option<CommandSpec> {
        None
    }

    fn parse_listed_skills(&self, _stdout: &str) -> Vec<String> {
        Vec::new()
    }

    fn prompt_cmd(&self, binary: &Path, req: &PromptRequest) -> CommandSpec {
        let mut spec = CommandSpec::new(binary).args(["run", "--text", &req.prompt]);
        if let Some(dir) = &req.cwd {
            spec = spec.cwd(dir);
        }
        spec
    }
}

/// An agent CLI found on the current machine, paired with its execution strategy.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct InstalledAgent {
    pub kind: AgentKind,
    pub binary: PathBuf,
    pub supports_zero_api_list: bool,
    pub default_model: Option<&'static str>,
}

impl InstalledAgent {
    pub fn new(kind: AgentKind, binary: PathBuf) -> Self {
        let strategy = kind.strategy();
        let supports_zero_api_list = strategy.list_skills_cmd(&binary, Path::new(".")).is_some();
        Self {
            kind,
            binary,
            supports_zero_api_list,
            default_model: strategy.default_model(),
        }
    }

    pub fn strategy(&self) -> &'static dyn AgentStrategy {
        self.kind.strategy()
    }

    pub fn version_cmd(&self) -> CommandSpec {
        self.strategy().version_cmd(&self.binary)
    }

    pub fn list_skills_cmd(&self, cwd: &Path) -> Option<CommandSpec> {
        self.strategy().list_skills_cmd(&self.binary, cwd)
    }

    pub fn prompt_cmd(&self, req: &PromptRequest) -> CommandSpec {
        self.strategy().prompt_cmd(&self.binary, req)
    }

    /// Run `<binary> --version` and return the trimmed version string if it succeeds.
    pub fn probe_version(&self) -> Option<String> {
        let out = self.version_cmd().run().ok()?;
        if out.status == 0 {
            let v = out.stdout.trim();
            if !v.is_empty() {
                return Some(v.lines().next().unwrap_or(v).trim().to_owned());
            }
        }
        None
    }
}

/// Scan the system (`PATH` plus per-agent fallback directories under `HOME`) for all known agents.
pub fn scan_installed(env: &Env) -> Vec<InstalledAgent> {
    AgentKind::ALL
        .into_iter()
        .filter_map(|k| find_installed(env, k))
        .collect()
}

/// Return the [`super::Agent`]s whose CLI binaries are installed on the system.
pub fn installed_agents(env: &Env) -> Vec<super::Agent> {
    scan_installed(env)
        .into_iter()
        .map(|a| a.kind.to_skill_agent())
        .collect()
}

/// Locate a single agent kind on the system.
pub fn find_installed(env: &Env, kind: AgentKind) -> Option<InstalledAgent> {
    let name = kind.binary_name();
    if let Some(path_var) = env.vars.get("PATH") {
        for dir in env::split_paths(path_var) {
            let candidate = dir.join(name);
            if is_executable_file(&candidate) {
                return Some(InstalledAgent::new(kind, candidate));
            }
        }
    }
    if let Some(home) = &env.home {
        for candidate in kind.strategy().home_fallbacks(home) {
            if is_executable_file(&candidate) {
                return Some(InstalledAgent::new(kind, candidate));
            }
        }
    }
    None
}

fn is_executable_file(path: &Path) -> bool {
    let Ok(meta) = fs::metadata(path) else {
        return false;
    };
    if !meta.is_file() {
        return false;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        meta.permissions().mode() & 0o111 != 0
    }
    #[cfg(not(unix))]
    {
        true
    }
}

/// Detect rate/quota/allocation limits from combined CLI output.
pub fn detect_quota_limit(text: &str) -> Option<QuotaLimit> {
    let lower = text.to_ascii_lowercase();
    let is_limit = lower.contains("hit your ") && lower.contains("limit")
        || lower.contains("hour limit")
        || lower.contains("rate_limit")
        || lower.contains("rate limit")
        || lower.contains("quota exceeded")
        || lower.contains("resource_exhausted")
        || lower.contains("http 429")
        || lower.contains("status 429")
        || lower.contains("too many requests")
        || lower.contains("credit balance is too low");
    if !is_limit {
        return None;
    }
    let first_line = text
        .lines()
        .map(str::trim)
        .find(|l| !l.is_empty())
        .unwrap_or("quota limit reached")
        .to_owned();
    let reset = text.lines().find_map(|line| {
        let l = line.to_ascii_lowercase();
        let idx = l.find("reset")?;
        Some(line[idx..].trim().to_owned())
    });
    Some(QuotaLimit {
        message: first_line,
        reset,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    #[cfg(unix)]
    use std::os::unix::fs::PermissionsExt;

    fn tmp(tag: &str) -> PathBuf {
        let d = env::temp_dir().join(format!("the-installed-{tag}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&d);
        fs::create_dir_all(&d).unwrap();
        d
    }

    fn fake_exe(dir: &Path, name: &str) -> PathBuf {
        let p = dir.join(name);
        fs::write(&p, "#!/bin/sh\necho 1.0.0\n").unwrap();
        #[cfg(unix)]
        {
            let mut perms = fs::metadata(&p).unwrap().permissions();
            perms.set_mode(0o755);
            fs::set_permissions(&p, perms).unwrap();
        }
        p
    }

    #[test]
    fn scans_path_and_home_fallbacks() {
        let root = tmp("scan");
        let bin_dir = root.join("bin");
        let home_dir = root.join("home");
        let oc_dir = home_dir.join(".opencode/bin");
        fs::create_dir_all(&bin_dir).unwrap();
        fs::create_dir_all(&oc_dir).unwrap();

        let claude_bin = fake_exe(&bin_dir, "claude");
        let gemini_bin = fake_exe(&bin_dir, "gemini");
        let opencode_bin = fake_exe(&oc_dir, "opencode");

        let mut vars = HashMap::new();
        vars.insert("PATH".to_owned(), bin_dir.to_string_lossy().into_owned());
        let env = Env {
            cwd: root.clone(),
            home: Some(home_dir),
            os: "linux",
            vars,
        };

        let found = scan_installed(&env);
        assert_eq!(found.len(), 3);
        assert_eq!(found[0].kind, AgentKind::Claude);
        assert_eq!(found[0].binary, claude_bin);
        assert!(!found[0].supports_zero_api_list);
        assert_eq!(found[0].default_model, Some("haiku"));

        assert_eq!(found[1].kind, AgentKind::Gemini);
        assert_eq!(found[1].binary, gemini_bin);
        assert!(found[1].supports_zero_api_list);
        assert_eq!(found[1].default_model, Some("gemini-2.5-flash-lite"));

        assert_eq!(found[2].kind, AgentKind::Opencode);
        assert_eq!(found[2].binary, opencode_bin);
        assert!(found[2].supports_zero_api_list);
        assert_eq!(found[2].default_model, None);
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn strategies_build_expected_commands() {
        let cwd = Path::new("/tmp/work");

        // Claude strategy
        let claude = InstalledAgent::new(AgentKind::Claude, PathBuf::from("/usr/bin/claude"));
        assert!(claude.list_skills_cmd(cwd).is_none());
        let c_prompt = claude.prompt_cmd(&PromptRequest::new("hello").cwd(cwd));
        assert_eq!(
            c_prompt.args,
            vec![
                "--setting-sources",
                "project",
                "--model",
                "haiku",
                "-p",
                "hello"
            ]
        );
        assert_eq!(c_prompt.cwd.as_deref(), Some(cwd));

        // Gemini strategy
        let gemini = InstalledAgent::new(AgentKind::Gemini, PathBuf::from("/opt/bin/gemini"));
        let g_list = gemini.list_skills_cmd(cwd).unwrap();
        assert_eq!(g_list.args, vec!["skills", "list"]);
        assert!(
            g_list
                .env_remove
                .contains(&"GOOGLE_CLOUD_PROJECT".to_owned())
        );
        let g_prompt = gemini.prompt_cmd(&PromptRequest::new("ping").model("flash").cwd(cwd));
        assert_eq!(
            g_prompt.args,
            vec!["--skip-trust", "-m", "gemini-2.5-flash", "-p", "ping"]
        );

        // OpenCode strategy
        let oc = InstalledAgent::new(AgentKind::Opencode, PathBuf::from("/opt/bin/opencode"));
        let oc_list = oc.list_skills_cmd(cwd).unwrap();
        assert_eq!(oc_list.args, vec!["debug", "skill"]);
        let oc_prompt =
            oc.prompt_cmd(&PromptRequest::new("check").model("anthropic/claude-haiku-4-5"));
        assert_eq!(
            oc_prompt.args,
            vec!["run", "-m", "anthropic/claude-haiku-4-5", "check"]
        );
    }

    #[test]
    fn parses_gemini_and_opencode_skill_lists() {
        let gemini_out = "\
Discovered Agent Skills:

marker-cwd [Enabled]
  Description: test
  Location:    /tmp/.gemini/skills/marker-cwd/SKILL.md

agent-skills [Enabled]
";
        assert_eq!(
            GeminiStrategy.parse_listed_skills(gemini_out),
            vec!["agent-skills".to_owned(), "marker-cwd".to_owned()]
        );

        let opencode_out = r#"[
  { "name": "marker-repo-root", "description": "d", "location": "/tmp/SKILL.md" },
  { "name": "marker-cwd", "description": "d", "location": "/tmp/2/SKILL.md" }
]"#;
        assert_eq!(
            OpencodeStrategy.parse_listed_skills(opencode_out),
            vec!["marker-cwd".to_owned(), "marker-repo-root".to_owned()]
        );
    }

    #[test]
    fn detects_quota_and_rate_limits() {
        let claude_msg = "You've hit your 5-hour limit · resets 6:20pm (Europe/Zurich)";
        let lim = detect_quota_limit(claude_msg).expect("should detect limit");
        assert_eq!(lim.message, claude_msg);
        assert_eq!(lim.reset.as_deref(), Some("resets 6:20pm (Europe/Zurich)"));

        assert!(detect_quota_limit("SKILL: marker-cwd").is_none());
    }

    #[test]
    fn infers_agent_from_model_hint() {
        assert_eq!(AgentKind::from_model_hint("haiku"), Some(AgentKind::Claude));
        assert_eq!(
            AgentKind::from_model_hint("sonnet"),
            Some(AgentKind::Claude)
        );
        assert_eq!(
            AgentKind::from_model_hint("gemini-2.5-flash-lite"),
            Some(AgentKind::Gemini)
        );
        assert_eq!(
            AgentKind::from_model_hint("anthropic/claude-3-5-haiku"),
            Some(AgentKind::Opencode)
        );
    }
}
