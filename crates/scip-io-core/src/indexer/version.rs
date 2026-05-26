use semver::Version;

pub fn normalize_version(version: &str) -> String {
    let version = version.trim();
    let version = version.trim_start_matches('v');
    if Version::parse(version).is_ok() {
        return version.to_owned();
    }

    match version.find(|ch: char| ch.is_ascii_digit()) {
        Some(pos) => version[pos..].to_owned(),
        None => version.to_owned(),
    }
}

pub fn version_is_newer(latest: &str, current: &str) -> bool {
    let latest = normalize_version(latest);
    let current = normalize_version(current);

    if latest == current || latest.is_empty() || current.is_empty() {
        return false;
    }

    match (Version::parse(&latest), Version::parse(&current)) {
        (Ok(latest), Ok(current)) => latest > current,
        _ => latest > current,
    }
}

#[cfg(test)]
mod tests {
    use super::{normalize_version, version_is_newer};

    #[test]
    fn normalize_version_strips_leading_v() {
        assert_eq!(normalize_version("v0.12.3"), "0.12.3");
        assert_eq!(normalize_version("  v1.2.3 "), "1.2.3");
        assert_eq!(normalize_version("scip-ruby-v0.4.7"), "0.4.7");
    }

    #[test]
    fn version_is_newer_uses_semver_ordering_when_possible() {
        assert!(version_is_newer("0.12.10", "0.12.3"));
        assert!(!version_is_newer("0.12.3", "v0.12.3"));
        assert!(!version_is_newer("scip-ruby-v0.4.7", "v0.4.7"));
        assert!(!version_is_newer("0.12.3", "0.12.10"));
    }

    #[test]
    fn version_is_newer_falls_back_to_string_ordering_for_date_tags() {
        assert!(version_is_newer("2026-04-01", "2026-03-30"));
        assert!(!version_is_newer("2026-03-30", "2026-04-01"));
    }
}
