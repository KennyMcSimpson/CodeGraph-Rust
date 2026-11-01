//! Marker-delimited instruction blocks inside markdown files (`CLAUDE.md`, `AGENTS.md`).

pub const START: &str = "<!-- CGR_START -->";
pub const END: &str = "<!-- CGR_END -->";

/// Insert or refresh the block. Content outside the markers is never touched.
pub fn upsert(current: Option<&str>, body: &str) -> String {
    let block = format!("{START}\n{body}\n{END}");
    let Some(cur) = current else { return format!("{block}\n") };
    if let (Some(a), Some(b)) = (cur.find(START), cur.find(END)) {
        if a < b {
            let end = b + END.len();
            return format!("{}{block}{}", &cur[..a], &cur[end..]);
        }
    }
    if cur.is_empty() {
        return format!("{block}\n");
    }
    let sep = if cur.ends_with("\n\n") {
        ""
    } else if cur.ends_with('\n') {
        "\n"
    } else {
        "\n\n"
    };
    format!("{cur}{sep}{block}\n")
}

/// Remove the block (and the blank line that separated it). `None` when nothing else is left.
pub fn remove(current: &str) -> Option<String> {
    let (Some(a), Some(b)) = (current.find(START), current.find(END)) else { return Some(current.to_owned()) };
    if a >= b {
        return Some(current.to_owned());
    }
    let mut before = current[..a].to_owned();
    let mut after = &current[b + END.len()..];
    if let Some(rest) = after.strip_prefix('\n') {
        after = rest;
    }
    // drop the blank separator that `upsert` added in front of the block
    if before.ends_with("\n\n") && (after.is_empty() || !after.starts_with('\n')) {
        before.pop();
    }
    let out = format!("{before}{after}");
    if out.trim().is_empty() {
        None
    } else {
        Some(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn upsert_is_idempotent_and_remove_restores() {
        for original in ["", "# Title\n\nText\n", "no trailing newline", "# A\n\n\n"] {
            let cur = (!original.is_empty()).then_some(original);
            let once = upsert(cur, "body");
            assert_eq!(upsert(Some(&once), "body"), once, "second install changes nothing");
            let removed = remove(&once);
            if original.trim().is_empty() {
                assert_eq!(removed, None);
            } else {
                // trailing newline normalisation aside, the original text comes back
                assert_eq!(removed.as_deref().map(str::trim_end), Some(original.trim_end()), "{original:?}");
            }
        }
    }

    #[test]
    fn refresh_replaces_only_the_block() {
        let a = upsert(Some("keep me\n"), "v1");
        let b = upsert(Some(&a), "v2");
        assert!(b.starts_with("keep me\n"));
        assert!(b.contains("v2") && !b.contains("v1"));
    }
}
