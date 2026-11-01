//! One literal scan per source file, shared by every framework / synthesis scanner.

use crate::FileHook;
use aho_corasick::AhoCorasick;
use cg_core::Lang;
use smallvec::SmallVec;
use std::collections::BTreeMap;
use std::sync::Arc;

pub(crate) struct HookPlan {
    matcher: Option<AhoCorasick>,
    owners: Vec<Vec<usize>>,
}

impl HookPlan {
    pub fn new(hooks: &[Arc<dyn FileHook>]) -> Self {
        let mut literals: BTreeMap<&str, Vec<usize>> = BTreeMap::new();
        for (i, hook) in hooks.iter().enumerate() {
            for literal in hook.triggers() {
                literals.entry(literal).or_default().push(i);
            }
        }
        let matcher = (!literals.is_empty()).then(|| AhoCorasick::new(literals.keys()).expect("valid literal hook triggers"));
        Self { matcher, owners: literals.into_values().collect() }
    }

    pub fn select(&self, hooks: &[Arc<dyn FileHook>], lang: Lang, rel: &str, src: &str) -> SmallVec<[bool; 16]> {
        let enabled: SmallVec<[bool; 16]> = hooks.iter().map(|h| h.applies(lang, rel)).collect();
        let mut selected: SmallVec<[bool; 16]> = hooks.iter().zip(&enabled).map(|(h, enabled)| *enabled && h.triggers().is_empty()).collect();
        let mut left = enabled.iter().zip(&selected).filter(|(a, b)| **a && !**b).count();
        if left == 0 {
            return selected;
        }
        if let Some(matcher) = &self.matcher {
            // Overlap matters: one literal may be a suffix/prefix of another hook's literal.
            for hit in matcher.find_overlapping_iter(src) {
                for &i in &self.owners[hit.pattern().as_usize()] {
                    if enabled[i] && !selected[i] {
                        selected[i] = true;
                        left -= 1;
                    }
                }
                if left == 0 {
                    break;
                }
            }
        }
        selected
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{HookCtx, HookOut};

    struct Hook(&'static [&'static str], bool);
    impl FileHook for Hook {
        fn name(&self) -> &'static str {
            "test"
        }
        fn triggers(&self) -> &'static [&'static str] {
            self.0
        }
        fn applies(&self, _: Lang, _: &str) -> bool {
            self.1
        }
        fn run(&self, _: &HookCtx<'_>, _: &mut HookOut) {}
    }

    #[test]
    fn fused_scan_matches_literal_semantics_including_overlap_and_shared_triggers() {
        let hooks: Vec<Arc<dyn FileHook>> = vec![
            Arc::new(Hook(&["app.get", "@route"], true)),
            Arc::new(Hook(&["get"], true)),
            Arc::new(Hook(&["app.get"], true)),
            Arc::new(Hook(&[], true)),
            Arc::new(Hook(&["app.get"], false)),
        ];
        let plan = HookPlan::new(&hooks);
        for src in ["", "app.get('/x')", "@route hi", "forget", "世界 app.get app.get"] {
            let expected: Vec<_> = hooks
                .iter()
                .map(|h| h.applies(Lang::JavaScript, "x.js") && (h.triggers().is_empty() || h.triggers().iter().any(|t| src.contains(t))))
                .collect();
            assert_eq!(&plan.select(&hooks, Lang::JavaScript, "x.js", src)[..], expected);
        }
    }
}
