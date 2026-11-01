//! Codex: `[mcp_servers.<name>]` in `~/.codex/config.toml` (or `$CODEX_HOME`), edited in place
//! with `toml_edit`, and the instruction block in `AGENTS.md`.

use crate::{block, edit_file, instruction_block_body, toml, AgentTarget, Change, Env, Request, Result};
use cg_core::brand;
use std::path::PathBuf;

pub struct Codex;

fn home(env: &Env) -> PathBuf {
    env.codex_home.clone().unwrap_or_else(|| env.home.join(".codex"))
}

impl AgentTarget for Codex {
    fn id(&self) -> &'static str {
        "codex"
    }
    fn display(&self) -> &'static str {
        "Codex"
    }
    fn detect(&self, env: &Env) -> bool {
        home(env).is_dir()
    }
    fn install(&self, env: &Env, req: &Request) -> Result<Vec<Change>> {
        // Codex has no per-project configuration: a project install adds only the instruction
        // block to the project's AGENTS.md, the server stays registered globally.
        let mut out = Vec::new();
        if !req.local {
            let path = home(env).join("config.toml");
            let bin = req.bin.to_string_lossy().into_owned();
            out.push(edit_file(&path, req, "MCP server", false, |cur| {
                let mut doc = toml::parse(&path, cur)?;
                toml::ensure_mcp_server(&mut doc, brand::MCP_SERVER_NAME, &bin, &["serve", "--mcp"]);
                Ok(Some(doc.to_string()))
            })?);
        }
        let md = if req.local { req.project.join("AGENTS.md") } else { home(env).join("AGENTS.md") };
        out.push(edit_file(&md, req, "instruction block", false, |cur| Ok(Some(block::upsert(cur, &instruction_block_body()))))?);
        Ok(out)
    }
    fn uninstall(&self, env: &Env, req: &Request) -> Result<Vec<Change>> {
        let mut out = Vec::new();
        if !req.local {
            let path = home(env).join("config.toml");
            out.push(edit_file(&path, req, "MCP server", false, |cur| {
                let Some(text) = cur else { return Ok(None) };
                let mut doc = toml::parse(&path, Some(text))?;
                toml::remove_mcp_server(&mut doc, brand::MCP_SERVER_NAME);
                let s = doc.to_string();
                Ok(if s.trim().is_empty() { None } else { Some(s) })
            })?);
        }
        let md = if req.local { req.project.join("AGENTS.md") } else { home(env).join("AGENTS.md") };
        out.push(edit_file(&md, req, "instruction block", false, |cur| Ok(cur.and_then(block::remove)))?);
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Action;

    #[test]
    fn roundtrip_preserves_user_config() {
        let dir = tempfile::tempdir().unwrap();
        let env = Env { home: dir.path().into(), cwd: dir.path().into(), claude_config_dir: None, codex_home: None };
        std::fs::create_dir_all(dir.path().join(".codex")).unwrap();
        let original = "# my config\nmodel = \"gpt\"\n\n    [mcp_servers.other]\n    command = \"x\"\n";
        std::fs::write(dir.path().join(".codex/config.toml"), original).unwrap();
        let req = Request { bin: "/opt/cgr".into(), local: false, project: dir.path().into(), hook: false, permissions: false, dry_run: false };
        Codex.install(&env, &req).unwrap();
        let text = std::fs::read_to_string(dir.path().join(".codex/config.toml")).unwrap();
        assert!(text.contains("# my config") && text.contains("[mcp_servers.other]") && text.contains("[mcp_servers.codegraph-rust]"));
        assert!(Codex.install(&env, &req).unwrap().iter().all(|c| c.action == Action::Unchanged));
        Codex.uninstall(&env, &req).unwrap();
        assert_eq!(std::fs::read_to_string(dir.path().join(".codex/config.toml")).unwrap().trim_end(), original.trim_end());
        assert!(!dir.path().join(".codex/AGENTS.md").exists());
    }
}
