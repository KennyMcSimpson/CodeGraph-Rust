//! cg-install: registering the MCP server with agents (design doc 11.3). One module per agent,
//! all built from the same primitives: comment-preserving JSON edits, TOML table edits and
//! marker-delimited instruction blocks. Every operation is idempotent and reversible, and only
//! adds what is missing: a command, cwd or argument the user pinned is never overwritten.

pub mod block;
pub mod claude;
pub mod codex;
pub mod cursor;
pub mod json;
pub mod toml;

use std::path::{Path, PathBuf};

#[derive(thiserror::Error, Debug)]
pub enum InstallError {
    #[error("{0}: {1}")]
    Io(PathBuf, std::io::Error),
    #[error("{0}: {1}")]
    Parse(PathBuf, String),
}

pub type Result<T> = std::result::Result<T, InstallError>;

/// What the machine looks like (injected so tests run in a temporary HOME).
#[derive(Clone, Debug)]
pub struct Env {
    pub home: PathBuf,
    pub cwd: PathBuf,
    /// `CLAUDE_CONFIG_DIR`, honoured by Claude Code.
    pub claude_config_dir: Option<PathBuf>,
    /// `CODEX_HOME`, honoured by Codex.
    pub codex_home: Option<PathBuf>,
}

impl Env {
    pub fn from_process() -> Env {
        let home = std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE")).map(PathBuf::from).unwrap_or_default();
        Env {
            home,
            cwd: std::env::current_dir().unwrap_or_default(),
            claude_config_dir: std::env::var_os("CLAUDE_CONFIG_DIR").filter(|v| !v.is_empty()).map(PathBuf::from),
            codex_home: std::env::var_os("CODEX_HOME").filter(|v| !v.is_empty()).map(PathBuf::from),
        }
    }
}

#[derive(Clone, Debug)]
pub struct Request {
    /// Absolute path of the `cgr` executable written into the agent configs.
    pub bin: PathBuf,
    /// Install into the project (`./.mcp.json`, `./CLAUDE.md`, ...) instead of the user's home.
    pub local: bool,
    pub project: PathBuf,
    /// Claude Code: register the prompt hook.
    pub hook: bool,
    /// Claude Code: allow the explore tool without a prompt.
    pub permissions: bool,
    pub dry_run: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    Created,
    Updated,
    Unchanged,
    Removed,
}

#[derive(Clone, Debug)]
pub struct Change {
    pub path: PathBuf,
    pub action: Action,
    pub what: &'static str,
}

pub trait AgentTarget {
    fn id(&self) -> &'static str;
    fn display(&self) -> &'static str;
    /// Is the agent present on this machine?
    fn detect(&self, env: &Env) -> bool;
    fn install(&self, env: &Env, req: &Request) -> Result<Vec<Change>>;
    fn uninstall(&self, env: &Env, req: &Request) -> Result<Vec<Change>>;
}

pub fn targets() -> Vec<Box<dyn AgentTarget>> {
    vec![Box::new(claude::ClaudeCode), Box::new(cursor::Cursor), Box::new(codex::Codex)]
}

/// Write `content` to `path` atomically (temp file + rename), keeping an existing file's mode.
pub fn write_atomic(path: &Path, content: &str, secret: bool) -> Result<()> {
    let io = |e| InstallError::Io(path.to_path_buf(), e);
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(io)?;
    }
    let tmp = path.with_extension(format!("cgr-tmp-{}", std::process::id()));
    std::fs::write(&tmp, content).map_err(io)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = match std::fs::metadata(path) {
            Ok(m) => m.permissions().mode() & 0o7777,
            Err(_) => {
                if secret {
                    0o600
                } else {
                    0o644
                }
            }
        };
        let _ = std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(mode));
    }
    #[cfg(not(unix))]
    let _ = secret;
    std::fs::rename(&tmp, path).map_err(io)
}

/// Apply a text edit to a file and report what happened. `edit` receives the current text
/// (`None` when the file does not exist) and returns the new text (`None` = delete the file).
pub fn edit_file(path: &Path, req: &Request, what: &'static str, secret: bool, edit: impl FnOnce(Option<&str>) -> Result<Option<String>>) -> Result<Change> {
    let current = match std::fs::read_to_string(path) {
        Ok(s) => Some(s),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
        Err(e) => return Err(InstallError::Io(path.to_path_buf(), e)),
    };
    let new = edit(current.as_deref())?;
    let action = match (&current, &new) {
        (None, None) => Action::Unchanged,
        (None, Some(_)) => Action::Created,
        (Some(_), None) => Action::Removed,
        (Some(a), Some(b)) if a == b => Action::Unchanged,
        _ => Action::Updated,
    };
    if !req.dry_run {
        match (&new, action) {
            (_, Action::Unchanged) => {}
            (Some(text), _) => write_atomic(path, text, secret)?,
            (None, _) => {
                let _ = std::fs::remove_file(path);
            }
        }
    }
    Ok(Change { path: path.to_path_buf(), action, what })
}

/// Text of the instruction block both Claude Code and Codex get (design doc 11.3): names the
/// tool and the CLI; how to use the tool lives in the MCP server instructions only.
pub fn instruction_block_body() -> String {
    use cg_core::brand;
    format!(
        "## {name}\n\nThis project is indexed by {name} (`{dir}/`). Use the `{prefix}explore` MCP tool — or `{bin} explore \"<question or symbol names>\"` in a shell — to get the verbatim source, call paths and blast radius of the code you are about to read or change in one call. If a tool reports that the project is not indexed, do not index it yourself; the user can run `{bin} init`.",
        name = brand::PROJECT_NAME,
        dir = brand::DATA_DIR,
        prefix = brand::MCP_TOOL_PREFIX,
        bin = brand::BIN_NAME
    )
}
