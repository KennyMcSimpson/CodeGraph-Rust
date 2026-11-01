//! Reference-name shapes (design doc 6.3): the walkers encode the syntactic form of a call in
//! the name (`this.svc.run`, `new Widget().render`, `super().run`, `T::new`, `inner().m`).

use smallvec::SmallVec;

#[derive(Debug, PartialEq, Eq)]
pub enum Shape<'a> {
    Bare(&'a str),
    /// `root.path.method` / `root::path::method` with `root` an ordinary identifier.
    Member { root: &'a str, path: SmallVec<[&'a str; 3]>, method: &'a str },
    /// `this|self|cls|Self` followed by fields and a final method.
    This { kw: &'a str, fields: SmallVec<[&'a str; 3]>, method: &'a str },
    Super { method: &'a str },
    New { class: &'a str, method: &'a str },
    CallResult { inner: &'a str, method: &'a str },
}

fn split_segments(name: &str) -> SmallVec<[&str; 6]> {
    let mut out: SmallVec<[&str; 6]> = SmallVec::new();
    let mut start = 0;
    let b = name.as_bytes();
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'.' {
            out.push(&name[start..i]);
            start = i + 1;
        } else if b[i] == b':' && b.get(i + 1) == Some(&b':') {
            out.push(&name[start..i]);
            i += 1;
            start = i + 1;
        }
        i += 1;
    }
    out.push(&name[start..]);
    out
}

fn last_segment(s: &str) -> &str {
    split_segments(s).last().copied().unwrap_or(s)
}

pub fn parse_shape(name: &str) -> Shape<'_> {
    let name = name.trim();
    if let Some(rest) = name.strip_prefix("new ") {
        if let Some(i) = rest.find("().") {
            return Shape::New { class: rest[..i].trim(), method: last_segment(&rest[i + 3..]) };
        }
        return Shape::Bare(rest.trim());
    }
    if let Some(i) = name.find("().") {
        let inner = name[..i].trim();
        let method = last_segment(&name[i + 3..]);
        return if inner == "super" { Shape::Super { method } } else { Shape::CallResult { inner, method } };
    }
    if name == "super" {
        return Shape::Bare("super");
    }
    let segs = split_segments(name);
    let n = segs.len();
    if n == 1 {
        return Shape::Bare(segs[0].trim_end_matches("[]"));
    }
    let first = segs[0];
    let method = segs[n - 1].trim_end_matches("[]");
    if matches!(first, "this" | "self" | "cls" | "Self") {
        return Shape::This { kw: first, fields: segs[1..n - 1].iter().map(|s| s.trim_end_matches("[]")).collect(), method };
    }
    if first == "super" {
        return Shape::Super { method };
    }
    Shape::Member { root: first.trim_end_matches("[]"), path: segs[1..n - 1].iter().map(|s| s.trim_end_matches("[]")).collect(), method }
}

/// First and remaining segments of a dotted / `::` name (type references like `pkg.Type`).
pub fn split_head(name: &str) -> Option<(&str, &str)> {
    if let Some(i) = name.find("::") {
        return Some((&name[..i], &name[i + 2..]));
    }
    name.find('.').map(|i| (&name[..i], &name[i + 1..]))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shapes() {
        assert_eq!(parse_shape("foo"), Shape::Bare("foo"));
        assert_eq!(parse_shape("helper.go"), Shape::Member { root: "helper", path: SmallVec::new(), method: "go" });
        assert_eq!(
            parse_shape("s.repo.Find"),
            Shape::Member { root: "s", path: SmallVec::from_slice(&["repo"]), method: "Find" }
        );
        assert_eq!(
            parse_shape("this.svc.run"),
            Shape::This { kw: "this", fields: SmallVec::from_slice(&["svc"]), method: "run" }
        );
        assert_eq!(parse_shape("self.repo[].find"), Shape::This { kw: "self", fields: SmallVec::from_slice(&["repo"]), method: "find" });
        assert_eq!(parse_shape("new Widget().render"), Shape::New { class: "Widget", method: "render" });
        assert_eq!(parse_shape("super().run"), Shape::Super { method: "run" });
        assert_eq!(parse_shape("inner().m"), Shape::CallResult { inner: "inner", method: "m" });
        assert_eq!(parse_shape("T::new"), Shape::Member { root: "T", path: SmallVec::new(), method: "new" });
        assert_eq!(parse_shape("Self::other"), Shape::This { kw: "Self", fields: SmallVec::new(), method: "other" });
        assert_eq!(parse_shape("a::b::c"), Shape::Member { root: "a", path: SmallVec::from_slice(&["b"]), method: "c" });
        assert_eq!(split_head("pkg.Type"), Some(("pkg", "Type")));
        assert_eq!(split_head("a::B"), Some(("a", "B")));
        assert_eq!(split_head("Type"), None);
    }
}
