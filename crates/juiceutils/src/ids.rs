#[must_use]
pub fn is_valid_id_component(value: &str) -> bool {
    !value.is_empty()
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

#[must_use]
pub fn is_valid_custom_id(id: &str, min_len: usize, max_len: usize) -> bool {
    !id.is_empty()
        && id.len() >= min_len
        && id.len() <= max_len
        && id
            .chars()
            .all(|c| c.is_alphanumeric() || c == '-' || c == '_')
}

#[must_use]
pub fn normalize_custom_id(id: &str) -> String {
    id.trim().to_lowercase()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn component_accepts_safe_values() {
        assert!(is_valid_id_component("abc-123"));
        assert!(is_valid_id_component("test_file"));
        assert!(is_valid_id_component("ABC123"));
        assert!(is_valid_id_component("my_file-123"));
    }

    #[test]
    fn component_rejects_unsafe_values() {
        assert!(!is_valid_id_component(""));
        assert!(!is_valid_id_component("has space"));
        assert!(!is_valid_id_component("file@name"));
        assert!(!is_valid_id_component("file.txt"));
        assert!(!is_valid_id_component("file/here"));
        assert!(!is_valid_id_component("cafe-é"));
    }

    #[test]
    fn custom_id_enforces_length_bounds() {
        assert!(is_valid_custom_id("abc", 3, 32));
        assert!(is_valid_custom_id("a-_-b", 3, 32));
        assert!(!is_valid_custom_id("ab", 3, 32));
        assert!(!is_valid_custom_id("", 3, 32));
        assert!(!is_valid_custom_id(&"a".repeat(33), 3, 32));
        assert!(is_valid_custom_id(&"a".repeat(32), 3, 32));
    }

    #[test]
    fn custom_id_rejects_bad_chars() {
        assert!(!is_valid_custom_id("has space", 3, 32));
        assert!(!is_valid_custom_id("file.txt", 3, 32));
        assert!(!is_valid_custom_id("a/b", 3, 32));
    }

    #[test]
    fn normalize_trims_and_lowercases() {
        assert_eq!(normalize_custom_id("  AbC-123  "), "abc-123");
        assert_eq!(normalize_custom_id("XYZ"), "xyz");
    }
}
