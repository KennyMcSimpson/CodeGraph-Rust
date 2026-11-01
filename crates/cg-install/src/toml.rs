//! Local edits of TOML files with `toml_edit`: other tables and comments stay untouched, and a
//! table is recognised by its key path however its header is spelled or indented (upstream #2562).

use crate::{InstallError, Result};
use std::path::Path;
use toml_edit::{value, Array, DocumentMut, Item, Table};

pub fn parse(path: &Path, text: Option<&str>) -> Result<DocumentMut> {
    text.unwrap_or("").parse::<DocumentMut>().map_err(|e| InstallError::Parse(path.to_path_buf(), e.to_string()))
}

/// Ensure `[mcp_servers.<name>]` exists with `command` and `args` (existing keys are kept).
pub fn ensure_mcp_server(doc: &mut DocumentMut, name: &str, command: &str, args: &[&str]) -> bool {
    let servers = doc.entry("mcp_servers").or_insert_with(|| {
        let mut t = Table::new();
        t.set_implicit(true);
        Item::Table(t)
    });
    let Some(servers) = servers.as_table_like_mut() else { return false };
    let mut changed = false;
    if servers.get(name).is_none() {
        servers.insert(name, Item::Table(Table::new()));
        changed = true;
    }
    let Some(entry) = servers.get_mut(name).and_then(|i| i.as_table_like_mut()) else { return changed };
    if entry.get("command").is_none() {
        entry.insert("command", value(command));
        changed = true;
    }
    if entry.get("args").is_none() {
        let mut a = Array::new();
        for s in args {
            a.push(*s);
        }
        entry.insert("args", value(a));
        changed = true;
    }
    changed
}

pub fn remove_mcp_server(doc: &mut DocumentMut, name: &str) -> bool {
    let Some(servers) = doc.get_mut("mcp_servers").and_then(|i| i.as_table_like_mut()) else { return false };
    let removed = servers.remove(name).is_some();
    if removed && servers.is_empty() {
        doc.remove("mcp_servers");
    }
    removed
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn indented_headers_and_comments_are_respected() {
        let text =
            "# my codex config\nmodel = \"x\"\n\n  [mcp_servers.other]\n  command = \"o\"\n\n  [mcp_servers.cgr]\n  # pinned\n  command = \"/mine/cgr\"\n";
        let mut doc = parse(Path::new("c.toml"), Some(text)).unwrap();
        assert!(ensure_mcp_server(&mut doc, "cgr", "/bin/cgr", &["serve", "--mcp"]));
        let out = doc.to_string();
        assert_eq!(out.matches("[mcp_servers.cgr]").count(), 1, "no duplicate table: {out}");
        assert!(out.contains("# pinned") && out.contains("/mine/cgr") && out.contains("args"));
        let mut doc2 = parse(Path::new("c.toml"), Some(&out)).unwrap();
        assert!(!ensure_mcp_server(&mut doc2, "cgr", "/bin/cgr", &["serve", "--mcp"]), "second run is a no-op");
    }

    #[test]
    fn create_and_remove() {
        let mut doc = parse(Path::new("c.toml"), None).unwrap();
        assert!(ensure_mcp_server(&mut doc, "cgr", "/bin/cgr", &["serve", "--mcp"]));
        assert!(doc.to_string().contains("[mcp_servers.cgr]"));
        assert!(remove_mcp_server(&mut doc, "cgr"));
        assert_eq!(doc.to_string().trim(), "");
    }
}
