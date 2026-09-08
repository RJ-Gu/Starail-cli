use std::fs;
use std::time::Duration;

use anyhow::{bail, Context, Result};
use reqwest::header::{ACCEPT, USER_AGENT};

use crate::config::AppConfig;
use crate::paths::StarailPaths;
use crate::platform;
use crate::profile::{self, ProfileKind, ProfileMeta};
use crate::runtime;
use crate::util::{require_profile_name, slug_from_url, unix_timestamp};

const SUBSCRIPTION_USER_AGENT: &str = "clash.meta";

pub fn add(paths: &StarailPaths, url: &str, name: Option<&str>) -> Result<()> {
    platform::require_linux()?;
    paths.init()?;
    if url.trim().is_empty() {
        bail!("usage: starail subscribe add <url> [name]");
    }

    let name = name
        .filter(|value| !value.trim().is_empty())
        .map(ToOwned::to_owned)
        .unwrap_or_else(|| slug_from_url(url));
    require_profile_name(&name)?;

    if profile::exists(paths, &name) {
        bail!("profile '{name}' already exists.");
    }

    let tmp = paths
        .tmp_dir
        .join(format!("subscription-{}-{}.yaml", name, std::process::id()));
    fetch_subscription_to(url, &tmp)?;
    println!("Validating subscription config with mihomo...");
    println!("mihomo may download GeoIP/MMDB geodata during validation if it is missing.");
    runtime::validate_config(paths, &tmp)?;
    println!("Validation OK.");

    println!("Saving profile '{name}'...");
    fs::rename(&tmp, paths.profile_config_path(&name))
        .with_context(|| format!("failed to save subscription profile '{name}'"))?;
    profile::write_meta(
        paths,
        &ProfileMeta {
            name: name.clone(),
            kind: ProfileKind::Subscription,
            source_url: Some(url.to_string()),
            updated_at: unix_timestamp().to_string(),
        },
    )?;

    let mut app_config = AppConfig::load(paths)?;
    app_config.active_profile = Some(name.clone());
    app_config.save(paths)?;
    println!("Added subscription profile '{name}' and selected it.");
    Ok(())
}

pub fn update(paths: &StarailPaths, name: Option<&str>) -> Result<()> {
    platform::require_linux()?;
    paths.init()?;

    if let Some(name) = name.filter(|value| !value.trim().is_empty()) {
        update_one(paths, name)?;
        return Ok(());
    }

    let mut updated = 0usize;
    for summary in profile::list(paths)? {
        if summary.kind == ProfileKind::Subscription {
            update_one(paths, &summary.name)?;
            updated += 1;
        }
    }

    if updated == 0 {
        println!("No subscription-backed profiles to update.");
    }

    Ok(())
}

fn update_one(paths: &StarailPaths, name: &str) -> Result<()> {
    require_profile_name(name)?;
    if !profile::exists(paths, name) {
        bail!("profile '{name}' does not exist.");
    }

    let meta = profile::read_meta(paths, name)?;
    if meta.kind != ProfileKind::Subscription {
        bail!("profile '{name}' is not subscription-backed.");
    }

    let url = meta
        .source_url
        .as_deref()
        .filter(|value| !value.trim().is_empty())
        .with_context(|| format!("profile '{name}' does not have a saved subscription URL"))?;

    let tmp = paths
        .tmp_dir
        .join(format!("subscription-{name}-{}.yaml", std::process::id()));
    println!("Updating {name}...");
    fetch_subscription_to(url, &tmp)?;
    println!("Validating subscription config with mihomo...");
    println!("mihomo may download GeoIP/MMDB geodata during validation if it is missing.");
    runtime::validate_config(paths, &tmp)?;
    println!("Validation OK.");

    println!("Replacing profile '{name}'...");
    fs::rename(&tmp, paths.profile_config_path(name))
        .with_context(|| format!("failed to replace subscription profile '{name}'"))?;
    profile::write_meta(
        paths,
        &ProfileMeta {
            name: name.to_string(),
            kind: ProfileKind::Subscription,
            source_url: Some(url.to_string()),
            updated_at: unix_timestamp().to_string(),
        },
    )?;
    println!("Updated subscription profile '{name}'.");
    Ok(())
}

fn fetch_subscription_to(url: &str, dest: &std::path::Path) -> Result<()> {
    let client = reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(60))
        .build()
        .context("failed to create HTTP client")?;

    let normalized = fetch_and_normalize_subscription(url, |user_agent| {
        download_subscription_bytes(&client, url, user_agent)
    })?;

    println!("Writing temporary config: {}", dest.display());
    fs::write(dest, normalized.bytes)
        .with_context(|| format!("failed to write {}", dest.display()))?;
    Ok(())
}

fn fetch_and_normalize_subscription<F>(url: &str, download: F) -> Result<NormalizedSubscription>
where
    F: FnOnce(&str) -> Result<Vec<u8>>,
{
    println!("Downloading subscription content...");
    println!("Using subscription client profile: {SUBSCRIPTION_USER_AGENT}");
    let bytes = download(SUBSCRIPTION_USER_AGENT)?;

    if bytes.is_empty() {
        bail!("subscription download was empty: {url}");
    }
    println!("Downloaded {} bytes.", bytes.len());

    if is_unsupported_client_response(&bytes) {
        bail!(
            "subscription provider rejected Starail's Mihomo client profile. The subscription likely contains protocols such as hysteria2 or VLESS. Use the provider's Mihomo/Clash.Meta subscription URL or format flag, then try again."
        );
    }

    println!("Parsing downloaded subscription content...");
    let normalized = normalize_subscription_bytes(url, &bytes)?;
    println!("Parsed subscription format: {}.", normalized.format);
    Ok(normalized)
}

fn download_subscription_bytes(
    client: &reqwest::blocking::Client,
    url: &str,
    user_agent: &str,
) -> Result<Vec<u8>> {
    let bytes = client
        .get(url)
        .header(USER_AGENT, user_agent)
        .header(
            ACCEPT,
            "application/yaml, application/x-yaml, text/yaml, text/plain, */*",
        )
        .send()
        .with_context(|| format!("failed to fetch subscription: {url}"))?
        .error_for_status()
        .with_context(|| format!("subscription request failed: {url}"))?
        .bytes()
        .with_context(|| format!("failed to read subscription response: {url}"))?;

    Ok(bytes.to_vec())
}

#[derive(Debug)]
struct NormalizedSubscription {
    bytes: Vec<u8>,
    format: &'static str,
}

fn normalize_subscription_bytes(url: &str, bytes: &[u8]) -> Result<NormalizedSubscription> {
    if looks_like_yaml_config(bytes) {
        return Ok(NormalizedSubscription {
            bytes: bytes.to_vec(),
            format: "Clash/Mihomo YAML",
        });
    }

    let Ok(text) = std::str::from_utf8(bytes) else {
        return Ok(NormalizedSubscription {
            bytes: bytes.to_vec(),
            format: "raw response",
        });
    };

    let Some(decoded) = decode_base64_text(text) else {
        return Ok(NormalizedSubscription {
            bytes: bytes.to_vec(),
            format: "raw response",
        });
    };

    if looks_like_yaml_config(decoded.as_bytes()) {
        return Ok(NormalizedSubscription {
            bytes: decoded.into_bytes(),
            format: "base64-wrapped Clash/Mihomo YAML",
        });
    }

    if decoded.lines().any(|line| line.contains("://")) {
        bail!(
            "subscription returned a base64 proxy-link list, not a Clash/mihomo YAML config: {url}\n\
Starail requested Clash-compatible YAML with a Clash user agent, but this endpoint still returned share links.\n\
Use the provider's Clash/Mihomo subscription URL or add the provider's Clash format flag, then try again."
        );
    }

    Ok(NormalizedSubscription {
        bytes: bytes.to_vec(),
        format: "raw response",
    })
}

fn is_unsupported_client_response(bytes: &[u8]) -> bool {
    if looks_like_yaml_config(bytes) {
        return false;
    }

    response_text_candidates(bytes)
        .iter()
        .any(|text| contains_unsupported_client_message(text))
}

fn response_text_candidates(bytes: &[u8]) -> Vec<String> {
    let Ok(text) = std::str::from_utf8(bytes) else {
        return Vec::new();
    };

    let mut candidates = vec![text.to_string()];
    if let Some(decoded) = decode_base64_text(text) {
        candidates.push(decoded);
    }
    candidates
}

fn contains_unsupported_client_message(text: &str) -> bool {
    let lower = text.to_ascii_lowercase();
    lower.contains("当前clash客户端不支持")
        || lower.contains("不支持本机场协议")
        || lower.contains("unsupported protocol")
        || lower.contains("client does not support")
}

fn looks_like_yaml_config(bytes: &[u8]) -> bool {
    let Ok(text) = std::str::from_utf8(bytes) else {
        return false;
    };

    let text = text.trim_start_matches('\u{feff}').trim_start();
    text.starts_with("proxies:")
        || text.starts_with("proxy-groups:")
        || text.starts_with("mixed-port:")
        || text.starts_with("port:")
        || text.starts_with("socks-port:")
        || text.contains("\nproxies:")
        || text.contains("\nproxy-groups:")
}

fn decode_base64_text(text: &str) -> Option<String> {
    let cleaned = text
        .chars()
        .filter(|value| !value.is_ascii_whitespace())
        .collect::<String>();
    if cleaned.is_empty() || !cleaned.bytes().all(is_base64_byte) {
        return None;
    }

    let decoded = decode_base64(cleaned.as_bytes())?;
    String::from_utf8(decoded).ok()
}

fn is_base64_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || matches!(byte, b'+' | b'/' | b'-' | b'_' | b'=')
}

fn decode_base64(input: &[u8]) -> Option<Vec<u8>> {
    let mut output = Vec::with_capacity(input.len() * 3 / 4);
    let mut chunk = [0u8; 4];
    let mut chunk_len = 0usize;
    let mut saw_padding = false;

    for &byte in input {
        let value = match byte {
            b'A'..=b'Z' if !saw_padding => byte - b'A',
            b'a'..=b'z' if !saw_padding => byte - b'a' + 26,
            b'0'..=b'9' if !saw_padding => byte - b'0' + 52,
            b'+' | b'-' if !saw_padding => 62,
            b'/' | b'_' if !saw_padding => 63,
            b'=' => {
                saw_padding = true;
                64
            }
            _ => return None,
        };

        chunk[chunk_len] = value;
        chunk_len += 1;
        if chunk_len == 4 {
            push_base64_chunk(&mut output, chunk)?;
            chunk_len = 0;
        }
    }

    if chunk_len == 1 {
        return None;
    }

    if chunk_len > 0 {
        for value in chunk.iter_mut().skip(chunk_len) {
            *value = 64;
        }
        push_base64_chunk(&mut output, chunk)?;
    }

    Some(output)
}

fn push_base64_chunk(output: &mut Vec<u8>, chunk: [u8; 4]) -> Option<()> {
    if chunk[0] == 64 || chunk[1] == 64 || (chunk[2] == 64 && chunk[3] != 64) {
        return None;
    }

    output.push((chunk[0] << 2) | (chunk[1] >> 4));
    if chunk[2] != 64 {
        output.push((chunk[1] << 4) | (chunk[2] >> 2));
    }
    if chunk[3] != 64 {
        output.push((chunk[2] << 6) | chunk[3]);
    }
    Some(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decodes_base64_wrapped_yaml_subscription() {
        let normalized = normalize_subscription_bytes("https://example.test/sub", b"cHJveGllczoK")
            .expect("base64 yaml should decode");

        assert_eq!(normalized.bytes, b"proxies:\n");
        assert_eq!(normalized.format, "base64-wrapped Clash/Mihomo YAML");
    }

    #[test]
    fn rejects_base64_proxy_link_lists() {
        let error =
            normalize_subscription_bytes("https://example.test/sub", b"c3M6Ly9leGFtcGxlCg==")
                .expect_err("proxy-link list should be rejected");

        assert!(error.to_string().contains("base64 proxy-link list"));
    }

    #[test]
    fn detects_provider_unsupported_client_messages() {
        assert!(is_unsupported_client_response(
            "当前Clash客户端不支持本机场协议，请更换以下支持协议的代理软件".as_bytes()
        ));
    }

    #[test]
    fn downloads_only_once_when_subscription_parsing_fails() {
        let mut downloads = 0;
        let error = fetch_and_normalize_subscription("https://example.test/sub", |_| {
            downloads += 1;
            Ok(b"c3M6Ly9leGFtcGxlCg==".to_vec())
        })
        .expect_err("proxy-link list should be rejected");

        assert_eq!(downloads, 1);
        assert!(error.to_string().contains("base64 proxy-link list"));
    }
}
