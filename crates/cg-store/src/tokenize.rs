//! Identifier and text tokenisation shared by the index builder and the query side, so a
//! query is cut into words exactly the way the index was.

/// Split an identifier into lower-case segments at camelCase, snake_case, digit and
/// punctuation boundaries: `HTTPServer2Go` -> http, server, 2, go.
pub fn split_identifier(name: &str, out: &mut Vec<String>) {
    let chars: Vec<char> = name.chars().collect();
    let mut cur = String::new();
    let flush = |cur: &mut String, out: &mut Vec<String>| {
        if !cur.is_empty() {
            out.push(cur.to_lowercase());
            cur.clear();
        }
    };
    for i in 0..chars.len() {
        let c = chars[i];
        if !c.is_alphanumeric() {
            flush(&mut cur, out);
            continue;
        }
        if let Some(&prev) = cur.chars().last().as_ref() {
            let boundary = (prev.is_lowercase() && c.is_uppercase())
                || (prev.is_alphabetic() && c.is_ascii_digit())
                || (prev.is_ascii_digit() && c.is_alphabetic())
                || (prev.is_uppercase() && c.is_uppercase() && chars.get(i + 1).is_some_and(|n| n.is_lowercase()));
            if boundary {
                flush(&mut cur, out);
            }
        }
        cur.push(c);
    }
    flush(&mut cur, out);
}

pub const STOPWORDS: &[&str] = &[
    "a", "an", "the", "of", "to", "in", "on", "for", "and", "or", "is", "are", "was", "be", "it", "this", "that", "with", "as", "at", "by", "from", "how",
    "what", "does", "do", "where", "which", "when", "who", "why", "can", "i", "we", "you", "me", "my", "our", "about", "into", "not", "if", "then", "than",
    "so", "all", "any", "has", "have", "had", "will", "would", "should", "could", "there", "their", "its",
];

pub fn is_stopword(w: &str) -> bool {
    STOPWORDS.contains(&w)
}

/// Lower-case alphanumeric words of free text (docstrings, signatures, prose queries).
pub fn words(text: &str, max_words: usize, out: &mut Vec<String>) {
    let mut n = 0;
    for w in text.split(|c: char| !(c.is_alphanumeric() || c == '_')) {
        if w.len() < 2 {
            continue;
        }
        out.push(w.to_lowercase());
        n += 1;
        if n >= max_words {
            break;
        }
    }
}

/// Does the word look like code (camelCase, snake_case, PascalCase with an inner capital,
/// contains digits or `.`/`::`)? Such words are kept verbatim in queries.
pub fn looks_like_code(w: &str) -> bool {
    let has_inner_upper = w.chars().skip(1).any(|c| c.is_uppercase());
    let has_lower = w.chars().any(|c| c.is_lowercase());
    (has_inner_upper && has_lower) || w.contains('_') || w.contains("::") || w.contains('.') && w.len() > 2
}

#[cfg(test)]
mod tests {
    use super::*;

    fn split(s: &str) -> Vec<String> {
        let mut v = Vec::new();
        split_identifier(s, &mut v);
        v
    }

    #[test]
    fn identifiers() {
        assert_eq!(split("HTTPServer2Go"), ["http", "server", "2", "go"]);
        assert_eq!(split("snake_case_name"), ["snake", "case", "name"]);
        assert_eq!(split("fooBarBaz"), ["foo", "bar", "baz"]);
        assert_eq!(split("a.b::c"), ["a", "b", "c"]);
        assert_eq!(split("__init__"), ["init"]);
    }

    #[test]
    fn code_words() {
        assert!(looks_like_code("renderStaticScene"));
        assert!(looks_like_code("snake_case"));
        assert!(!looks_like_code("render"));
        assert!(is_stopword("the"));
    }
}
