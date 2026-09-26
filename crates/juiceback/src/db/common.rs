#[must_use]
pub(crate) fn sort_direction(dir: &str) -> &str {
    if dir.eq_ignore_ascii_case("asc") {
        "ASC"
    } else {
        "DESC"
    }
}
