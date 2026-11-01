//! Framework rules (design doc 6.7): detection from manifests, extract hooks that add route
//! nodes, resolve rules. The hook list is empty for projects that use none.

pub mod routes;

use crate::project::ProjectInfo;
use cg_extract::FileHook;
use routes::{Framework, RouteHook};
use std::sync::Arc;

pub fn hooks(project: &ProjectInfo) -> Vec<Arc<dyn FileHook>> {
    let mut v: Vec<Arc<dyn FileHook>> = Vec::new();
    // synthesis site scanners: cheap literal prefilters keep them off files that cannot match
    v.push(Arc::new(crate::synth::sites::SiteScanner));
    for name in project.detect_frameworks() {
        let fw = match name.as_str() {
            "express" => Framework::Express,
            "flask" => Framework::Flask,
            "fastapi" => Framework::FastApi,
            "django" => Framework::Django,
            "gin" => Framework::Gin,
            "spring" => Framework::Spring,
            "axum" => Framework::Axum,
            "next" => Framework::Next,
            _ => continue,
        };
        v.push(Arc::new(RouteHook { fw }));
    }
    v
}
