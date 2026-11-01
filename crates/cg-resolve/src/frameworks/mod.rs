//! Framework rules (design doc 6.7): detection from manifests, extract hooks that add route
//! nodes, resolve rules. Populated per framework; the hook list is empty for projects that use
//! none.

use crate::project::ProjectInfo;
use cg_extract::FileHook;
use std::sync::Arc;

pub fn hooks(_project: &ProjectInfo) -> Vec<Arc<dyn FileHook>> {
    Vec::new()
}
