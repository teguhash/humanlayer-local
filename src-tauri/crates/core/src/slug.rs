/// A branch- and folder-safe slug from a task title, at most 40 chars.
pub fn slugify(title: &str) -> String {
    let mut s = String::new();
    for c in title.chars().flat_map(char::to_lowercase) {
        if c.is_ascii_alphanumeric() {
            s.push(c);
        } else if !s.ends_with('-') && !s.is_empty() {
            s.push('-');
        }
        if s.len() >= 40 {
            break;
        }
    }
    let s = s.trim_matches('-').to_string();
    if s.is_empty() { "task".into() } else { s }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slugs() {
        assert_eq!(slugify("Fix: the Login bug!"), "fix-the-login-bug");
        assert_eq!(slugify("  ***  "), "task");
        assert_eq!(slugify("Ünïcode ok"), "n-code-ok");
        assert!(slugify(&"a b ".repeat(50)).len() <= 40);
    }
}
