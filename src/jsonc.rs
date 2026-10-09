//! JSON with comments: `//` and `/* */` comments and trailing commas are
//! blanked out (newlines kept, so parser line numbers still match the file)
//! before the text goes to serde_json.

pub fn strip(src: &str) -> String {
    let b = src.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        match b[i] {
            b'"' => {
                // Copy the string literal verbatim, escapes included.
                out.push(b'"');
                i += 1;
                while i < b.len() {
                    out.push(b[i]);
                    if b[i] == b'\\' && i + 1 < b.len() {
                        out.push(b[i + 1]);
                        i += 2;
                        continue;
                    }
                    i += 1;
                    if b[i - 1] == b'"' {
                        break;
                    }
                }
            }
            b'/' if b.get(i + 1) == Some(&b'/') => {
                while i < b.len() && b[i] != b'\n' {
                    out.push(b' ');
                    i += 1;
                }
            }
            b'/' if b.get(i + 1) == Some(&b'*') => {
                out.extend_from_slice(b"  ");
                i += 2;
                while i < b.len() && !(b[i] == b'*' && b.get(i + 1) == Some(&b'/')) {
                    out.push(if b[i] == b'\n' { b'\n' } else { b' ' });
                    i += 1;
                }
                if i < b.len() {
                    out.extend_from_slice(b"  ");
                    i += 2;
                }
            }
            c => {
                out.push(c);
                i += 1;
            }
        }
    }
    remove_trailing_commas(&mut out);
    // Only ASCII bytes outside strings were replaced, so this stays valid UTF-8.
    String::from_utf8(out).unwrap_or_default()
}

/// Blanks a comma whose next non-space character closes an object or array.
fn remove_trailing_commas(b: &mut [u8]) {
    let mut in_str = false;
    let mut i = 0;
    while i < b.len() {
        let c = b[i];
        if in_str {
            if c == b'\\' {
                i += 1;
            } else if c == b'"' {
                in_str = false;
            }
        } else if c == b'"' {
            in_str = true;
        } else if c == b',' {
            let next = b[i + 1..].iter().copied().find(|c| !c.is_ascii_whitespace());
            if matches!(next, Some(b'}') | Some(b']')) {
                b[i] = b' ';
            }
        }
        i += 1;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strips_comments_and_trailing_commas() {
        let src = "{\n  // line\n  \"a\": \"x // not a comment\", /* block\n spans */ \"b\": [1, 2,],\n  \"c\": \"q\\\"/*\",\n}";
        let v: serde_json::Value = serde_json::from_str(&strip(src)).unwrap();
        assert_eq!(v["a"], "x // not a comment");
        assert_eq!(v["b"], serde_json::json!([1, 2]));
        assert_eq!(v["c"], "q\"/*");
        // Line numbers survive.
        assert_eq!(strip(src).lines().count(), src.lines().count());
    }

    #[test]
    fn keeps_unicode() {
        let v: serde_json::Value = serde_json::from_str(&strip("{\"n\": \"Café ▬\", // é\n}")).unwrap();
        assert_eq!(v["n"], "Café ▬");
    }
}
