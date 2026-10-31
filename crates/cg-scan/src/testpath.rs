//! Test-file and non-production-directory heuristics (upstream `search/query-utils.ts`).

use regex::Regex;
use std::sync::OnceLock;

fn re(cell: &'static OnceLock<Regex>, p: &str) -> &'static Regex {
    cell.get_or_init(|| Regex::new(p).expect("test path regex"))
}

/// A suite that exercises other code: filename and directory conventions of every ecosystem.
pub fn is_test_path(path: &str) -> bool {
    static SEP: OnceLock<Regex> = OnceLock::new();
    static CAMEL: OnceLock<Regex> = OnceLock::new();
    static CAMEL_DIR: OnceLock<Regex> = OnceLock::new();
    static SUPPORT: OnceLock<Regex> = OnceLock::new();
    static UNIT: OnceLock<Regex> = OnceLock::new();
    let lower = path.to_ascii_lowercase();
    let name = path.rsplit('/').next().unwrap_or(path);
    let lname = name.to_ascii_lowercase();
    if lname.starts_with("test_")
        || lname.starts_with("test.")
        || re(&SEP, r"[._-](test|tests|unittest|unittests|spec|specs)\.[a-z0-9]+$").is_match(&lname)
        || re(&CAMEL, r"(?:Test|Tests|TestCase|Tester|Spec|Specs)\.[A-Za-z0-9]+$").is_match(name)
    {
        return true;
    }
    lower.contains("/tests/")
        || lower.contains("/test/")
        || lower.contains("/__tests__/")
        || lower.contains("/spec/")
        || lower.contains("/specs/")
        || lower.contains("/testlib/")
        || lower.contains("/testing/")
        || lower.contains("/e2e/")
        || lower.starts_with("test/")
        || lower.starts_with("tests/")
        || lower.starts_with("spec/")
        || lower.starts_with("specs/")
        || lower.starts_with("e2e/")
        || re(&CAMEL_DIR, r"(?:^|/)[A-Za-z0-9]*(?:Test|Tests|Spec)/").is_match(path)
        || re(
            &SUPPORT,
            r"(?:^|/)(?:[\w.]+[-_]test(?:s|ing)?|testdata|testutils?|test[-_]utils?|fakes?|mocks?|__mocks__|stubs)/",
        )
        .is_match(&lower)
        || re(&UNIT, r"(?:^|/)(?:unittests|[\w.]+[-_]unittests?)/").is_match(&lower)
}

/// examples / samples / benchmarks / fixtures / demos above a `src/`.
pub fn is_non_production_dir(path: &str) -> bool {
    let lower = path.to_ascii_lowercase();
    let scope = match lower.find("/src/") {
        Some(i) => &lower[..=i],
        None if lower.starts_with("src/") => "",
        None => &lower[..],
    };
    const DIRS: [&str; 11] = [
        "integration", "sample", "samples", "example", "examples", "fixture", "fixtures",
        "benchmark", "benchmarks", "demo", "demos",
    ];
    DIRS.iter().any(|d| scope.contains(&format!("/{d}/")) || scope.starts_with(&format!("{d}/")))
}

/// Test or non-production: down-ranked in search.
pub fn is_test_file(path: &str) -> bool {
    is_test_path(path) || is_non_production_dir(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn conventions() {
        assert!(is_test_path("pkg/foo_test.go"));
        assert!(is_test_path("tests/test_x.py"));
        assert!(is_test_path("src/app/foo.spec.ts"));
        assert!(is_test_path("app/src/androidTest/java/FooTest.kt"));
        assert!(!is_test_path("src/latest.kt"));
        assert!(!is_test_path("src/unittest.go"));
        assert!(is_non_production_dir("examples/basic/main.go"));
        assert!(!is_non_production_dir("core/src/main/kotlin/com/google/samples/App.kt"));
        assert!(is_test_file("benchmarks/run.js"));
    }
}
