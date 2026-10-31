//! Patch P4 - parse hook.
//!
//! Every walker parses through [`parse`], which
//!  1. enforces the per-file parse timeout (design doc 4.4) with tree-sitter's
//!     progress callback, and
//!  2. stashes a clone of the finished syntax tree in a thread-local so that
//!     `cg-extract` can build the facts tables (imports, exports, scopes,
//!     bindings: design doc 6.1) from the SAME parse instead of parsing twice.
//!
//! A `Tree` clone is a reference-count bump, so the stash is free.

use std::cell::{Cell, RefCell};
use std::time::{Duration, Instant};
use tree_sitter::{ParseOptions, Parser, Tree};

thread_local! {
    static STASH: RefCell<Option<Tree>> = const { RefCell::new(None) };
    static TIMEOUT: Cell<Option<Duration>> = const { Cell::new(None) };
    static TIMED_OUT: Cell<bool> = const { Cell::new(false) };
}

/// Per-thread parse deadline for the next extractions (`None` = unlimited).
pub fn set_parse_timeout(limit: Option<Duration>) {
    TIMEOUT.with(|c| c.set(limit));
}

/// The syntax tree of the most recent successful parse on this thread.
pub fn take_tree() -> Option<Tree> {
    STASH.with(|s| s.borrow_mut().take())
}

/// Whether the most recent parse on this thread was cancelled by the timeout.
pub fn parse_timed_out() -> bool {
    TIMED_OUT.with(|c| c.get())
}

/// Drop any stashed tree (call after a failed extraction).
pub fn clear() {
    STASH.with(|s| *s.borrow_mut() = None);
}

pub(crate) fn parse(parser: &mut Parser, source: &str) -> Option<Tree> {
    STASH.with(|s| *s.borrow_mut() = None);
    TIMED_OUT.with(|c| c.set(false));
    let bytes = source.as_bytes();
    let tree = match TIMEOUT.with(|c| c.get()) {
        None => parser.parse(bytes, None),
        Some(limit) => {
            let deadline = Instant::now() + limit;
            let mut expired = false;
            let mut progress = |_: &tree_sitter::ParseState| {
                if Instant::now() >= deadline {
                    expired = true;
                    true // cancel
                } else {
                    false
                }
            };
            let tree = parser.parse_with_options(
                &mut |offset, _| if offset < bytes.len() { &bytes[offset..] } else { &[][..] },
                None,
                Some(ParseOptions::new().progress_callback(&mut progress)),
            );
            if expired {
                TIMED_OUT.with(|c| c.set(true));
            }
            tree
        }
    };
    if let Some(t) = &tree {
        STASH.with(|s| *s.borrow_mut() = Some(t.clone()));
    }
    tree
}

/// The tree-sitter grammar the kernel uses for `lang` (tests and dev tools; the facts
/// extractor never needs it because it reuses the kernel's tree).
pub fn language(lang: &str) -> Option<tree_sitter::Language> {
    crate::langs::grammar_for(lang)
}
