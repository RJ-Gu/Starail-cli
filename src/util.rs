use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{bail, Result};

pub fn require_profile_name(name: &str) -> Result<()> {
    if profile_name_ok(name) {
        Ok(())
    } else {
        bail!(
            "invalid profile name '{}'. Use letters, numbers, dot, dash, or underscore.",
            name
        )
    }
}

pub fn profile_name_ok(name: &str) -> bool {
    !name.is_empty()
        && name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
}

pub fn slug_from_url(url: &str) -> String {
    let without_scheme = url.split_once("://").map(|(_, rest)| rest).unwrap_or(url);
    let clean = without_scheme
        .split(['?', '#'])
        .next()
        .unwrap_or(without_scheme)
        .trim_matches('/');
    let mut pieces = clean.split('/').filter(|piece| !piece.is_empty());
    let host = pieces.next().unwrap_or_default();
    let last = clean
        .rsplit('/')
        .find(|piece| !piece.is_empty())
        .unwrap_or_default();

    let raw = if host.is_empty() {
        last.to_string()
    } else if last.is_empty() || last == host {
        host.to_string()
    } else {
        format!("{host}-{last}")
    };

    let slug = sanitize_slug(&raw);
    if slug.is_empty() {
        format!("subscription-{}", unix_timestamp())
    } else {
        slug
    }
}

pub fn unix_timestamp() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or_default()
}

pub fn json_string(value: &str) -> String {
    serde_json::to_string(value).unwrap_or_else(|_| "\"\"".to_string())
}

fn sanitize_slug(value: &str) -> String {
    value
        .bytes()
        .map(|byte| {
            if byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-') {
                byte as char
            } else {
                '-'
            }
        })
        .collect::<String>()
        .trim_matches('-')
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validates_profile_names() {
        assert!(profile_name_ok("hk-main_01"));
        assert!(profile_name_ok("work.profile"));
        assert!(!profile_name_ok(""));
        assert!(!profile_name_ok("../bad"));
        assert!(!profile_name_ok("bad name"));
    }

    #[test]
    fn slugs_subscription_urls() {
        assert_eq!(
            slug_from_url("https://example.com/sub/path.yaml?token=secret"),
            "example.com-path.yaml"
        );
    }
}
