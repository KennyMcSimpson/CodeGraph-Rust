//! Language detection (upstream `detectLanguage`, `EXTENSION_MAP`): extension map plus the
//! content checks that matter for the supported languages. Detection of unsupported
//! languages still works so `status` can report how much of a repo is outside the MVP set.

use cg_core::Lang;
use std::collections::BTreeMap;

/// Extension (lower-case, with dot) -> language. Mirrors upstream `EXTENSION_MAP` for the
/// languages `cg-core::Lang` knows.
pub fn language_for_extension(ext: &str) -> Lang {
    match ext {
        ".ts" | ".mts" | ".cts" => Lang::TypeScript,
        ".tsx" => Lang::Tsx,
        ".js" | ".mjs" | ".cjs" | ".xsjs" | ".xsjslib" => Lang::JavaScript,
        ".jsx" => Lang::Jsx,
        ".py" | ".pyw" => Lang::Python,
        ".go" => Lang::Go,
        ".rs" => Lang::Rust,
        ".java" => Lang::Java,
        ".kt" | ".kts" => Lang::Kotlin,
        ".c" | ".h" => Lang::C,
        ".cpp" | ".cc" | ".cxx" | ".hpp" | ".hxx" | ".metal" | ".cu" | ".cuh" => Lang::Cpp,
        ".cs" => Lang::CSharp,
        ".php" | ".module" | ".install" | ".theme" | ".inc" => Lang::Php,
        ".rb" | ".rake" => Lang::Ruby,
        ".swift" => Lang::Swift,
        ".dart" => Lang::Dart,
        ".scala" | ".sc" => Lang::Scala,
        ".lua" => Lang::Lua,
        ".luau" => Lang::Luau,
        ".r" => Lang::R,
        _ => Lang::Unknown,
    }
}

/// Lower-cased extension of the last path segment, including the dot ("" if none).
pub fn extension_of(path: &str) -> String {
    let name = path.rsplit('/').next().unwrap_or(path);
    match name.rfind('.') {
        Some(i) => name[i..].to_ascii_lowercase(),
        None => String::new(),
    }
}

/// Detect from the path alone (config `extensions` take precedence over the built-in map).
pub fn detect_from_path(path: &str, overrides: &BTreeMap<String, Lang>) -> Lang {
    let ext = extension_of(path);
    if let Some(l) = overrides.get(&ext) {
        return *l;
    }
    language_for_extension(&ext)
}

/// Detect with the file's text available: a `.js`/`.jsx` file carrying Flow's `@flow`
/// pragma is parsed with the TSX grammar, as upstream does.
pub fn detect(path: &str, source: Option<&str>, overrides: &BTreeMap<String, Lang>) -> Lang {
    let lang = detect_from_path(path, overrides);
    if matches!(lang, Lang::JavaScript | Lang::Jsx) {
        if let Some(src) = source {
            if has_flow_pragma(src) {
                return Lang::Tsx;
            }
        }
    }
    lang
}

/// Whether leading comments carry `@flow` (and not `@noflow`).
pub fn has_flow_pragma(source: &str) -> bool {
    let mut end = source.len().min(4096);
    while !source.is_char_boundary(end) {
        end -= 1;
    }
    let mut head = &source[..end];
    if head.starts_with("#!") {
        head = head.find('\n').map(|i| &head[i + 1..]).unwrap_or("");
    }
    // Collect the run of leading comments.
    let mut lead_end = 0usize;
    let mut rest = head;
    loop {
        let trimmed = rest.trim_start();
        let skipped = rest.len() - trimmed.len();
        if trimmed.starts_with("//") {
            let line_len = trimmed.find('\n').unwrap_or(trimmed.len());
            lead_end += skipped + line_len;
            rest = &trimmed[line_len..];
        } else if trimmed.starts_with("/*") {
            match trimmed.find("*/") {
                Some(i) => {
                    lead_end += skipped + i + 2;
                    rest = &trimmed[i + 2..];
                }
                None => break,
            }
        } else {
            break;
        }
    }
    let lead = &head[..lead_end.min(head.len())];
    let has = |word: &str| lead.match_indices(word).any(|(i, _)| lead[i + word.len()..].chars().next().map_or(true, |c| !(c.is_alphanumeric() || c == '_')));
    has("@flow") && !has("@noflow")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extensions() {
        let none = BTreeMap::new();
        assert_eq!(detect_from_path("a/b/c.TS", &none), Lang::TypeScript);
        assert_eq!(detect_from_path("x.mjs", &none), Lang::JavaScript);
        assert_eq!(detect_from_path("x.kts", &none), Lang::Kotlin);
        assert_eq!(detect_from_path("Makefile", &none), Lang::Unknown);
        let mut o = BTreeMap::new();
        o.insert(".tpl".to_string(), Lang::Php);
        assert_eq!(detect_from_path("a.tpl", &o), Lang::Php);
    }

    #[test]
    fn flow_pragma() {
        assert!(has_flow_pragma("// @flow\nconst a = 1;"));
        assert!(has_flow_pragma("/* hello */\n/** @flow strict */\nx"));
        assert!(!has_flow_pragma("// @noflow\nx"));
        assert!(!has_flow_pragma("const a = 1; // @flow"));
        assert_eq!(detect("a.js", Some("// @flow\n"), &BTreeMap::new()), Lang::Tsx);
    }
}
