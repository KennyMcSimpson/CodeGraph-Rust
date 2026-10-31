//! Grammar registry: codegraph `Language` string -> native tree-sitter grammar.
//!
//! P1: reduced to the MVP languages (each behind a cargo feature). `tsx` and
//! `jsx` reuse another language's grammar exactly as upstream does.

use tree_sitter::Language;

// Vendored kotlin grammar (build.rs-compiled C, no usable crate exists).
#[cfg(feature = "lang-kotlin")]
extern "C" {
    fn tree_sitter_kotlin() -> *const ();
}

/// Languages this kernel build can extract.
pub const LANGUAGES: &[&str] = &[
    #[cfg(feature = "lang-ts")]
    "typescript",
    #[cfg(feature = "lang-ts")]
    "tsx",
    #[cfg(feature = "lang-ts")]
    "javascript",
    #[cfg(feature = "lang-ts")]
    "jsx",
    #[cfg(feature = "lang-java")]
    "java",
    #[cfg(feature = "lang-python")]
    "python",
    #[cfg(feature = "lang-go")]
    "go",
    #[cfg(feature = "lang-rust")]
    "rust",
    #[cfg(feature = "lang-kotlin")]
    "kotlin",
];

pub fn grammar_for(language: &str) -> Option<Language> {
    match language {
        #[cfg(feature = "lang-ts")]
        "typescript" => Some(tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into()),
        #[cfg(feature = "lang-ts")]
        "tsx" => Some(tree_sitter_typescript::LANGUAGE_TSX.into()),
        #[cfg(feature = "lang-ts")]
        "javascript" | "jsx" => Some(tree_sitter_javascript::LANGUAGE.into()),
        #[cfg(feature = "lang-java")]
        "java" => Some(tree_sitter_java::LANGUAGE.into()),
        #[cfg(feature = "lang-python")]
        "python" => Some(tree_sitter_python::LANGUAGE.into()),
        #[cfg(feature = "lang-go")]
        "go" => Some(tree_sitter_go::LANGUAGE.into()),
        #[cfg(feature = "lang-rust")]
        "rust" => Some(tree_sitter_rust::LANGUAGE.into()),
        #[cfg(feature = "lang-kotlin")]
        "kotlin" => {
            Some(unsafe { tree_sitter_language::LanguageFn::from_raw(tree_sitter_kotlin) }.into())
        }
        _ => None,
    }
}
