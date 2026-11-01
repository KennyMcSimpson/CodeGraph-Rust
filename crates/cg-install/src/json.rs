//! Comment- and format-preserving edits of JSON / JSONC files through the `jsonc-parser` CST.

use crate::{InstallError, Result};
use jsonc_parser::cst::{CstArray, CstInputValue, CstObject, CstRootNode};
use jsonc_parser::ParseOptions;
use serde_json::Value;
use std::path::Path;

pub fn parse(path: &Path, text: Option<&str>) -> Result<CstRootNode> {
    let src = match text {
        Some(t) if !t.trim().is_empty() => t,
        _ => "{}",
    };
    let root = CstRootNode::parse(src, &ParseOptions::default()).map_err(|e| InstallError::Parse(path.to_path_buf(), e.to_string()))?;
    if root.object_value().is_none() {
        return Err(InstallError::Parse(path.to_path_buf(), "top level must be a JSON object".into()));
    }
    Ok(root)
}

pub fn to_input(v: &Value) -> CstInputValue {
    match v {
        Value::Null => CstInputValue::Null,
        Value::Bool(b) => CstInputValue::Bool(*b),
        Value::Number(n) => CstInputValue::Number(n.to_string()),
        Value::String(s) => CstInputValue::String(s.clone()),
        Value::Array(a) => CstInputValue::Array(a.iter().map(to_input).collect()),
        Value::Object(o) => CstInputValue::Object(o.iter().map(|(k, v)| (k.clone(), to_input(v))).collect()),
    }
}

/// Descend through `path` creating objects as needed.
pub fn object_at(root: &CstRootNode, path: &[&str]) -> CstObject {
    let mut cur = root.object_value_or_set();
    for seg in path {
        cur = cur.object_value_or_set(seg);
    }
    cur
}

fn existing_object_at(root: &CstRootNode, path: &[&str]) -> Option<CstObject> {
    let mut cur = root.object_value()?;
    for seg in path {
        cur = cur.object_value(seg)?;
    }
    Some(cur)
}

/// Make sure `container[name]` exists and carries every key of `desired`; existing values are
/// kept as they are. Returns whether anything was added.
pub fn ensure_entry(root: &CstRootNode, path: &[&str], name: &str, desired: &Value) -> bool {
    let container = object_at(root, path);
    match container.get(name) {
        None => {
            container.append(name, to_input(desired));
            true
        }
        Some(prop) => {
            let (Some(obj), Some(want)) = (prop.value().and_then(|v| v.as_object()), desired.as_object()) else { return false };
            let mut changed = false;
            for (k, v) in want {
                if obj.get(k).is_none() {
                    obj.append(k, to_input(v));
                    changed = true;
                }
            }
            changed
        }
    }
}

/// `entry[key]` is a string array; make sure the argument sequence `want` appears in it
/// (appending it when missing).
pub fn ensure_args(root: &CstRootNode, path: &[&str], name: &str, key: &str, want: &[&str]) -> bool {
    let Some(container) = existing_object_at(root, path) else { return false };
    let Some(entry) = container.get(name).and_then(|p| p.value()).and_then(|v| v.as_object()) else { return false };
    let Some(arr) = entry.get(key).and_then(|p| p.value()).and_then(|v| v.as_array()) else { return false };
    let have: Vec<String> = arr.elements().iter().filter_map(|e| e.as_string_lit().and_then(|s| s.decoded_value().ok())).collect();
    if want.iter().all(|w| have.iter().any(|h| h == w)) {
        return false;
    }
    for w in want {
        if !have.iter().any(|h| h == w) {
            arr.append(CstInputValue::String((*w).to_owned()));
        }
    }
    true
}

/// Remove `container[name]`, then any ancestors on `path` that are left empty.
pub fn remove_entry(root: &CstRootNode, path: &[&str], name: &str) -> bool {
    let Some(container) = existing_object_at(root, path) else { return false };
    let Some(prop) = container.get(name) else { return false };
    prop.remove();
    prune_empty(root, path);
    true
}

fn prune_empty(root: &CstRootNode, path: &[&str]) {
    for depth in (1..=path.len()).rev() {
        let sub = &path[..depth];
        let Some(obj) = existing_object_at(root, sub) else { return };
        if !obj.properties().is_empty() {
            return;
        }
        let parent = existing_object_at(root, &sub[..depth - 1]);
        if let Some(p) = parent.and_then(|p| p.get(sub[depth - 1])) {
            p.remove();
        }
    }
}

pub fn array_strings(arr: &CstArray) -> Vec<String> {
    arr.elements().iter().filter_map(|e| e.as_string_lit().and_then(|s| s.decoded_value().ok())).collect()
}

/// Serialise: JSON files end with a newline.
pub fn finish(root: &CstRootNode, original: Option<&str>) -> String {
    let mut s = root.to_string();
    if !s.ends_with('\n') && original.map_or(true, |o| o.ends_with('\n') || o.trim().is_empty()) {
        s.push('\n');
    }
    s
}

/// True when the document is an empty object (nothing left worth keeping).
pub fn is_empty_object(root: &CstRootNode) -> bool {
    root.object_value().map_or(true, |o| o.properties().is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn run(text: &str) -> String {
        let root = parse(Path::new("x.json"), Some(text)).unwrap();
        ensure_entry(&root, &["mcpServers"], "cgr", &json!({"command": "/bin/cgr", "args": ["serve"]}));
        finish(&root, Some(text))
    }

    #[test]
    fn comments_and_siblings_survive_and_install_is_idempotent() {
        let text = "{\n  // my servers\n  \"mcpServers\": {\n    \"other\": { \"command\": \"x\" } // keep\n  },\n  \"theme\": \"dark\"\n}\n";
        let once = run(text);
        assert!(once.contains("// my servers") && once.contains("// keep") && once.contains("\"theme\": \"dark\""));
        assert!(once.contains("\"cgr\""));
        assert_eq!(run(&once), once);
        // removal restores the original bytes
        let root = parse(Path::new("x.json"), Some(&once)).unwrap();
        assert!(remove_entry(&root, &["mcpServers"], "cgr"));
        assert_eq!(finish(&root, Some(&once)), text);
    }

    #[test]
    fn user_pinned_values_are_not_overwritten() {
        let text = "{\"mcpServers\":{\"cgr\":{\"command\":\"/mine/cgr\"}}}";
        let root = parse(Path::new("x.json"), Some(text)).unwrap();
        assert!(ensure_entry(&root, &["mcpServers"], "cgr", &json!({"command": "/bin/cgr", "args": ["serve"]})));
        let v = root.to_serde_value().unwrap();
        assert_eq!(v["mcpServers"]["cgr"]["command"], "/mine/cgr");
        assert_eq!(v["mcpServers"]["cgr"]["args"][0], "serve");
    }

    #[test]
    fn creating_from_nothing_and_removing_leaves_an_empty_document() {
        let root = parse(Path::new("x.json"), None).unwrap();
        ensure_entry(&root, &["mcpServers"], "cgr", &json!({"command": "c"}));
        assert!(!is_empty_object(&root));
        remove_entry(&root, &["mcpServers"], "cgr");
        assert!(is_empty_object(&root));
    }

    #[test]
    fn rejects_non_objects() {
        assert!(parse(Path::new("x.json"), Some("[1,2]")).is_err());
        assert!(parse(Path::new("x.json"), Some("{ broken")).is_err());
    }
}
