//! Framework rules (design doc 6.7): detection from manifests, extract hooks that add route
//! nodes, resolve rules. The hook list is empty for projects that use none.

pub mod href;
pub mod jsroutes;
pub mod nav;
pub mod routes;

use crate::project::ProjectInfo;
use cg_extract::FileHook;
use routes::{Framework, RouteHook};
use std::sync::Arc;

pub fn hooks(project: &ProjectInfo) -> Vec<Arc<dyn FileHook>> {
    // synthesis site scanners: cheap literal prefilters keep them off files that cannot match
    let mut v: Vec<Arc<dyn FileHook>> = vec![
        Arc::new(crate::synth::sites::SiteScanner),
        Arc::new(crate::synth::sites::GoTypeSites),
        Arc::new(crate::synth::dynamic::CallbackScanner),
        Arc::new(crate::synth::dynamic::RegistryScanner),
        Arc::new(crate::synth::dynamic::TierScanner),
        Arc::new(crate::synth::dynamic::CeleryScanner),
        Arc::new(crate::synth::dynamic::SpringEventScanner),
    ];
    let frameworks = project.detect_frameworks();
    if frameworks.iter().any(|f| matches!(f.as_str(), "next" | "react-router" | "tanstack-router")) {
        v.push(Arc::new(nav::NavScanner));
    }
    if frameworks.iter().any(|f| f == "next") {
        v.push(Arc::new(nav::ProxyScanner));
    }
    for name in frameworks {
        let fw = match name.as_str() {
            "express" => Framework::Express,
            "flask" => Framework::Flask,
            "fastapi" => Framework::FastApi,
            "django" => Framework::Django,
            "gin" => Framework::Gin,
            "spring" => Framework::Spring,
            "axum" => Framework::Axum,
            "next" => Framework::Next,
            "nestjs" => Framework::Nest,
            "react-router" => Framework::ReactRouter,
            "tanstack-router" => Framework::TanStack,
            _ => continue,
        };
        v.push(Arc::new(RouteHook { fw }));
    }
    v
}
