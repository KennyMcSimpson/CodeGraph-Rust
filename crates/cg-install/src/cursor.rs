//! Cursor: `~/.cursor/mcp.json`. Cursor does not start servers in the workspace directory, so
//! `--path` is injected: the literal project path for a project install, `${workspaceFolder}`
//! for a global one (design doc 11.3).

use crate::{edit_file, json, AgentTarget, Change, Env, Request, Result};
use cg_core::brand;
use serde_json::json;

pub struct Cursor;

fn mcp_path(env: &Env, req: &Request) -> std::path::PathBuf {
    if req.local {
        req.project.join(".cursor/mcp.json")
    } else {
        env.home.join(".cursor/mcp.json")
    }
}

impl AgentTarget for Cursor {
    fn id(&self) -> &'static str {
        "cursor"
    }
    fn display(&self) -> &'static str {
        "Cursor"
    }
    fn detect(&self, env: &Env) -> bool {
        env.home.join(".cursor").is_dir()
    }
    fn install(&self, env: &Env, req: &Request) -> Result<Vec<Change>> {
        let path = mcp_path(env, req);
        let project_arg = if req.local { req.project.to_string_lossy().into_owned() } else { "${workspaceFolder}".to_owned() };
        let entry = json!({"command": req.bin.to_string_lossy(), "args": ["serve", "--mcp", "--path", project_arg]});
        Ok(vec![edit_file(&path, req, "MCP server", false, |cur| {
            let root = json::parse(&path, cur)?;
            json::ensure_entry(&root, &["mcpServers"], brand::MCP_SERVER_NAME, &entry);
            // a pinned entry without --path still needs one
            json::ensure_args(&root, &["mcpServers"], brand::MCP_SERVER_NAME, "args", &["--path", &project_arg]);
            Ok(Some(json::finish(&root, cur)))
        })?])
    }
    fn uninstall(&self, env: &Env, req: &Request) -> Result<Vec<Change>> {
        let path = mcp_path(env, req);
        Ok(vec![edit_file(&path, req, "MCP server", false, |cur| {
            let Some(text) = cur else { return Ok(None) };
            let root = json::parse(&path, Some(text))?;
            json::remove_entry(&root, &["mcpServers"], brand::MCP_SERVER_NAME);
            Ok(if json::is_empty_object(&root) { None } else { Some(json::finish(&root, cur)) })
        })?])
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Action;

    #[test]
    fn global_install_injects_workspace_folder_and_is_idempotent() {
        let dir = tempfile::tempdir().unwrap();
        let env = Env { home: dir.path().to_path_buf(), cwd: dir.path().into(), claude_config_dir: None, codex_home: None };
        std::fs::create_dir_all(dir.path().join(".cursor")).unwrap();
        std::fs::write(dir.path().join(".cursor/mcp.json"), "{\"mcpServers\":{\"keep\":{\"command\":\"k\"}}}").unwrap();
        let req = Request { bin: "/opt/cgr".into(), local: false, project: dir.path().into(), hook: false, permissions: false, dry_run: false };
        Cursor.install(&env, &req).unwrap();
        let text = std::fs::read_to_string(dir.path().join(".cursor/mcp.json")).unwrap();
        assert!(text.contains("${workspaceFolder}") && text.contains("\"keep\""));
        assert_eq!(Cursor.install(&env, &req).unwrap()[0].action, Action::Unchanged);
        Cursor.uninstall(&env, &req).unwrap();
        let v: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(dir.path().join(".cursor/mcp.json")).unwrap()).unwrap();
        assert_eq!(v, json!({"mcpServers": {"keep": {"command": "k"}}}));
    }

    #[test]
    fn project_install_uses_the_absolute_path() {
        let dir = tempfile::tempdir().unwrap();
        let env = Env { home: dir.path().join("h"), cwd: dir.path().into(), claude_config_dir: None, codex_home: None };
        let req = Request { bin: "/opt/cgr".into(), local: true, project: dir.path().join("proj"), hook: false, permissions: false, dry_run: false };
        Cursor.install(&env, &req).unwrap();
        let text = std::fs::read_to_string(dir.path().join("proj/.cursor/mcp.json")).unwrap();
        assert!(text.contains(&dir.path().join("proj").to_string_lossy().into_owned()));
    }

    #[test]
    fn pinned_entry_without_path_gets_one() {
        let dir = tempfile::tempdir().unwrap();
        let env = Env { home: dir.path().into(), cwd: dir.path().into(), claude_config_dir: None, codex_home: None };
        std::fs::create_dir_all(dir.path().join(".cursor")).unwrap();
        std::fs::write(dir.path().join(".cursor/mcp.json"), "{\"mcpServers\":{\"codegraph-rust\":{\"command\":\"/mine\",\"args\":[\"serve\",\"--mcp\"]}}}").unwrap();
        let req = Request { bin: "/opt/cgr".into(), local: false, project: dir.path().into(), hook: false, permissions: false, dry_run: false };
        Cursor.install(&env, &req).unwrap();
        let text = std::fs::read_to_string(dir.path().join(".cursor/mcp.json")).unwrap();
        assert!(text.contains("/mine") && text.contains("--path") && text.contains("${workspaceFolder}"), "{text}");
    }
}
