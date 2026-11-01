//! Claude Code: MCP server in `~/.claude.json` (or `./.mcp.json` for a project install), the
//! instruction block in `CLAUDE.md`, the explore permission and the `UserPromptSubmit` hook in
//! `settings.json`. Honours `CLAUDE_CONFIG_DIR`. Never writes `./.claude.json`, which Claude Code
//! does not read (upstream #207).

use crate::{block, edit_file, instruction_block_body, json, AgentTarget, Change, Env, Request, Result};
use cg_core::brand;
use serde_json::json;
use std::path::PathBuf;

pub struct ClaudeCode;

struct Paths {
    mcp: PathBuf,
    settings: PathBuf,
    md: PathBuf,
}

fn paths(env: &Env, req: &Request) -> Paths {
    if req.local {
        return Paths { mcp: req.project.join(".mcp.json"), settings: req.project.join(".claude/settings.json"), md: req.project.join("CLAUDE.md") };
    }
    match &env.claude_config_dir {
        Some(d) => Paths { mcp: d.join(".claude.json"), settings: d.join("settings.json"), md: d.join("CLAUDE.md") },
        None => Paths { mcp: env.home.join(".claude.json"), settings: env.home.join(".claude/settings.json"), md: env.home.join(".claude/CLAUDE.md") },
    }
}

fn permission_entries() -> Vec<String> {
    vec![format!("mcp__{}__{}explore", brand::MCP_SERVER_NAME, brand::MCP_TOOL_PREFIX)]
}

fn hook_command(req: &Request) -> String {
    format!("{} prompt-hook", shell_quote(&req.bin.to_string_lossy()))
}

fn shell_quote(s: &str) -> String {
    if s.chars().all(|c| c.is_ascii_alphanumeric() || "/._-+:@".contains(c)) {
        s.to_owned()
    } else {
        format!("'{}'", s.replace('\'', "'\\''"))
    }
}

fn is_our_hook(cmd: &str) -> bool {
    cmd.contains("prompt-hook") && (cmd.contains("cgr") || cmd.contains(brand::PROJECT_NAME))
}

impl AgentTarget for ClaudeCode {
    fn id(&self) -> &'static str {
        "claude-code"
    }
    fn display(&self) -> &'static str {
        "Claude Code"
    }
    fn detect(&self, env: &Env) -> bool {
        let dir = env.claude_config_dir.clone().unwrap_or_else(|| env.home.join(".claude"));
        dir.is_dir() || env.home.join(".claude.json").is_file()
    }

    fn install(&self, env: &Env, req: &Request) -> Result<Vec<Change>> {
        let p = paths(env, req);
        let mut out = Vec::new();
        let entry = json!({
            "type": "stdio",
            "command": req.bin.to_string_lossy(),
            "args": ["serve", "--mcp"],
            "alwaysLoad": true
        });
        out.push(edit_file(&p.mcp, req, "MCP server", true, |cur| {
            let root = json::parse(&p.mcp, cur)?;
            json::ensure_entry(&root, &["mcpServers"], brand::MCP_SERVER_NAME, &entry);
            Ok(Some(json::finish(&root, cur)))
        })?);
        if req.permissions || req.hook {
            let hook = hook_command(req);
            out.push(edit_file(&p.settings, req, "settings (permission, prompt hook)", false, |cur| {
                let root = json::parse(&p.settings, cur)?;
                if req.permissions {
                    let allow = json::object_at(&root, &["permissions"]).array_value_or_set("allow");
                    let have = json::array_strings(&allow);
                    for perm in permission_entries() {
                        if !have.contains(&perm) {
                            allow.append(jsonc_parser::cst::CstInputValue::String(perm));
                        }
                    }
                }
                if req.hook {
                    let list = json::object_at(&root, &["hooks"]).array_value_or_set("UserPromptSubmit");
                    let present = list.elements().iter().any(|e| {
                        e.to_serde_value()
                            .map_or(false, |v| v["hooks"].as_array().map_or(false, |hs| hs.iter().any(|h| h["command"].as_str().map_or(false, is_our_hook))))
                    });
                    if !present {
                        list.append(json::to_input(&json!({"hooks": [{"type": "command", "command": hook}]})));
                    }
                }
                Ok(Some(json::finish(&root, cur)))
            })?);
        }
        out.push(edit_file(&p.md, req, "instruction block", false, |cur| Ok(Some(block::upsert(cur, &instruction_block_body()))))?);
        Ok(out)
    }

    fn uninstall(&self, env: &Env, req: &Request) -> Result<Vec<Change>> {
        let p = paths(env, req);
        let mut out = Vec::new();
        out.push(edit_file(&p.mcp, req, "MCP server", true, |cur| {
            let Some(text) = cur else { return Ok(None) };
            let root = json::parse(&p.mcp, Some(text))?;
            json::remove_entry(&root, &["mcpServers"], brand::MCP_SERVER_NAME);
            Ok(if json::is_empty_object(&root) { None } else { Some(json::finish(&root, cur)) })
        })?);
        out.push(edit_file(&p.settings, req, "settings (permission, prompt hook)", false, |cur| {
            let Some(text) = cur else { return Ok(None) };
            let root = json::parse(&p.settings, Some(text))?;
            if let Some(allow) = root.object_value().and_then(|o| o.object_value("permissions")).and_then(|o| o.array_value("allow")) {
                for el in allow.elements() {
                    if el.as_string_lit().and_then(|s| s.decoded_value().ok()).map_or(false, |s| permission_entries().contains(&s)) {
                        el.remove();
                    }
                }
                if allow.elements().is_empty() {
                    if let Some(p) = root.object_value().and_then(|o| o.object_value("permissions")).and_then(|o| o.get("allow")) {
                        p.remove();
                    }
                    if let Some(perm) = root.object_value().and_then(|o| o.get("permissions")) {
                        if perm.value().and_then(|v| v.as_object()).map_or(false, |o| o.properties().is_empty()) {
                            perm.remove();
                        }
                    }
                }
            }
            if let Some(list) = root.object_value().and_then(|o| o.object_value("hooks")).and_then(|o| o.array_value("UserPromptSubmit")) {
                for el in list.elements() {
                    let ours = el
                        .to_serde_value()
                        .map_or(false, |v| v["hooks"].as_array().map_or(false, |hs| hs.iter().any(|h| h["command"].as_str().map_or(false, is_our_hook))));
                    if ours {
                        el.remove();
                    }
                }
                if list.elements().is_empty() {
                    if let Some(hooks) = root.object_value().and_then(|o| o.object_value("hooks")) {
                        if let Some(p) = hooks.get("UserPromptSubmit") {
                            p.remove();
                        }
                        if hooks.properties().is_empty() {
                            if let Some(p) = root.object_value().and_then(|o| o.get("hooks")) {
                                p.remove();
                            }
                        }
                    }
                }
            }
            Ok(if json::is_empty_object(&root) { None } else { Some(json::finish(&root, cur)) })
        })?);
        out.push(edit_file(&p.md, req, "instruction block", false, |cur| Ok(cur.and_then(block::remove)))?);
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Action;

    fn setup() -> (tempfile::TempDir, Env, Request) {
        let dir = tempfile::tempdir().unwrap();
        let env = Env { home: dir.path().join("home"), cwd: dir.path().join("proj"), claude_config_dir: None, codex_home: None };
        std::fs::create_dir_all(&env.home).unwrap();
        std::fs::create_dir_all(&env.cwd).unwrap();
        let req = Request { bin: "/opt/cgr".into(), local: false, project: env.cwd.clone(), hook: true, permissions: true, dry_run: false };
        (dir, env, req)
    }

    fn snapshot(root: &std::path::Path) -> Vec<(String, String)> {
        let mut v = Vec::new();
        fn walk(dir: &std::path::Path, base: &std::path::Path, v: &mut Vec<(String, String)>) {
            if let Ok(rd) = std::fs::read_dir(dir) {
                for e in rd.flatten() {
                    let p = e.path();
                    if p.is_dir() {
                        walk(&p, base, v);
                    } else {
                        v.push((p.strip_prefix(base).unwrap().to_string_lossy().into_owned(), std::fs::read_to_string(&p).unwrap_or_default()));
                    }
                }
            }
        }
        walk(root, root, &mut v);
        v.sort();
        v
    }

    #[test]
    fn install_twice_is_unchanged_and_uninstall_restores() {
        let (dir, env, req) = setup();
        let original = "{\n  \"numStartups\": 3,\n  \"mcpServers\": {\n    \"other\": { \"command\": \"x\" }\n  }\n}\n";
        std::fs::write(env.home.join(".claude.json"), original).unwrap();
        std::fs::create_dir_all(env.home.join(".claude")).unwrap();
        std::fs::write(env.home.join(".claude/CLAUDE.md"), "# Mine\n\nHello\n").unwrap();
        std::fs::write(env.home.join(".claude/settings.json"), "{\n  // keep\n  \"permissions\": { \"allow\": [\"Bash(ls)\"] }\n}\n").unwrap();
        let before = snapshot(dir.path());
        let first = ClaudeCode.install(&env, &req).unwrap();
        assert!(first.iter().all(|c| c.action == Action::Updated), "{first:?}");
        let installed = snapshot(dir.path());
        let second = ClaudeCode.install(&env, &req).unwrap();
        assert!(second.iter().all(|c| c.action == Action::Unchanged), "{second:?}");
        assert_eq!(snapshot(dir.path()), installed, "second install must not change a byte");
        let text = std::fs::read_to_string(env.home.join(".claude.json")).unwrap();
        assert!(text.contains("\"other\"") && text.contains("alwaysLoad") && text.contains("codegraph-rust"));
        let settings = std::fs::read_to_string(env.home.join(".claude/settings.json")).unwrap();
        assert!(settings.contains("// keep") && settings.contains("UserPromptSubmit") && settings.contains("Bash(ls)"));
        ClaudeCode.uninstall(&env, &req).unwrap();
        let after = snapshot(dir.path());
        let norm = |v: Vec<(String, String)>| v.into_iter().map(|(p, c)| (p, c.trim_end().to_owned())).collect::<Vec<_>>();
        assert_eq!(norm(after), norm(before), "uninstall must restore the files");
    }

    #[test]
    fn fresh_machine_roundtrip_leaves_nothing() {
        let (dir, env, req) = setup();
        ClaudeCode.install(&env, &req).unwrap();
        assert!(env.home.join(".claude.json").exists());
        ClaudeCode.uninstall(&env, &req).unwrap();
        assert!(snapshot(dir.path()).is_empty(), "{:?}", snapshot(dir.path()));
    }

    #[test]
    fn project_install_uses_mcp_json_and_never_dot_claude_json() {
        let (_dir, env, mut req) = setup();
        req.local = true;
        ClaudeCode.install(&env, &req).unwrap();
        assert!(env.cwd.join(".mcp.json").exists());
        assert!(env.cwd.join("CLAUDE.md").exists());
        assert!(!env.cwd.join(".claude.json").exists());
    }

    #[test]
    fn respects_claude_config_dir() {
        let (dir, mut env, req) = setup();
        env.claude_config_dir = Some(dir.path().join("cc"));
        ClaudeCode.install(&env, &req).unwrap();
        assert!(dir.path().join("cc/.claude.json").exists() && dir.path().join("cc/settings.json").exists());
        assert!(!env.home.join(".claude.json").exists());
    }

    #[test]
    fn half_finished_state_recovers() {
        let (_dir, env, req) = setup();
        // the MCP entry exists, the settings and CLAUDE.md do not
        std::fs::write(env.home.join(".claude.json"), "{\"mcpServers\":{\"codegraph-rust\":{\"command\":\"/opt/cgr\"}}}").unwrap();
        let changes = ClaudeCode.install(&env, &req).unwrap();
        assert!(changes.iter().any(|c| c.action == Action::Created));
        let v: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(env.home.join(".claude.json")).unwrap()).unwrap();
        assert_eq!(v["mcpServers"]["codegraph-rust"]["alwaysLoad"], true, "missing keys are filled in");
    }
}
