use std::env;
use std::fs;
use std::io::Read;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use anyhow::{bail, Context, Result};
use flate2::read::GzDecoder;
use serde::Deserialize;

use crate::config::app_language;
use crate::i18n::{Language, Message};
use crate::paths::{display_path, StarailPaths};
use crate::platform;

const MIHOMO_REPO: &str = "MetaCubeX/mihomo";
const USER_AGENT: &str = concat!("starail/", env!("CARGO_PKG_VERSION"));

#[derive(Debug, Deserialize)]
struct Release {
    tag_name: String,
    assets: Vec<ReleaseAsset>,
}

struct CoreAsset {
    release_version: String,
    url: String,
}

#[derive(Debug, Deserialize)]
struct ReleaseAsset {
    name: String,
    browser_download_url: String,
}

pub fn install(paths: &StarailPaths) -> Result<()> {
    platform::require_linux()?;
    paths.init()?;
    let language = app_language(paths);

    let asset = latest_core_asset(language)?;
    match managed_core_version(paths) {
        Some(current) => {
            println!(
                "{} {current}",
                language.tr(Message::InstalledMihomoCoreLabel)
            );
            println!(
                "{} {}",
                language.tr(Message::LatestMihomoReleaseLabel),
                asset.release_version
            );
            if versions_match(&current, &asset.release_version) {
                println!("{}", language.tr(Message::MihomoCoreUpToDate));
                return Ok(());
            }
            println!(
                "{}: {current} -> {}.",
                language.tr(Message::UpdatingMihomoCore),
                asset.release_version
            );
        }
        None => {
            println!(
                "{} {}.",
                language.tr(Message::ManagedCoreMissingInstalling),
                asset.release_version
            );
        }
    }

    download_and_install_core(paths, &asset.url, language)?;
    version(paths)
}

fn download_and_install_core(
    paths: &StarailPaths,
    asset_url: &str,
    language: Language,
) -> Result<()> {
    println!("{} {asset_url}", language.tr(Message::Downloading));

    let client = reqwest::blocking::Client::builder()
        .user_agent(USER_AGENT)
        .timeout(Duration::from_secs(120))
        .build()
        .context("failed to create HTTP client")?;

    let bytes = client
        .get(asset_url)
        .send()
        .with_context(|| format!("failed to download {asset_url}"))?
        .error_for_status()
        .with_context(|| format!("download failed: {asset_url}"))?
        .bytes()
        .with_context(|| format!("failed to read downloaded core archive: {asset_url}"))?;

    let mut decoder = GzDecoder::new(&bytes[..]);
    let mut binary = Vec::new();
    decoder
        .read_to_end(&mut binary)
        .context("failed to unpack downloaded mihomo archive")?;

    let candidate = paths
        .tmp_dir
        .join(format!("mihomo-{}.tmp", std::process::id()));
    fs::write(&candidate, binary)
        .with_context(|| format!("failed to write {}", candidate.display()))?;
    fs::set_permissions(&candidate, fs::Permissions::from_mode(0o755))
        .with_context(|| format!("failed to mark {} executable", candidate.display()))?;

    let output = Command::new(&candidate)
        .arg("-v")
        .output()
        .with_context(|| format!("failed to execute downloaded core {}", candidate.display()))?;
    if !output.status.success() {
        let _ = fs::remove_file(&candidate);
        bail!("downloaded file did not look like a working mihomo binary");
    }

    fs::rename(&candidate, &paths.core_file)
        .with_context(|| format!("failed to install core at {}", paths.core_file.display()))?;
    println!(
        "{} {}",
        language.tr(Message::InstalledMihomoCoreAt),
        paths.core_file.display()
    );
    Ok(())
}

pub fn version(paths: &StarailPaths) -> Result<()> {
    paths.init()?;
    let core = require_core(paths)?;
    let status = Command::new(&core)
        .arg("-v")
        .status()
        .with_context(|| format!("failed to execute {}", core.display()))?;
    if status.success() {
        Ok(())
    } else {
        bail!("mihomo version command failed for {}", core.display())
    }
}

pub fn version_string(paths: &StarailPaths) -> Result<String> {
    paths.init()?;
    let core = require_core(paths)?;
    core_version_from_path(&core)
}

fn managed_core_version(paths: &StarailPaths) -> Option<String> {
    if !is_executable(&paths.core_file) {
        return None;
    }

    core_version_from_path(&paths.core_file).ok()
}

fn core_version_from_path(core: &Path) -> Result<String> {
    let output = Command::new(core)
        .arg("-v")
        .output()
        .with_context(|| format!("failed to execute {}", core.display()))?;
    if !output.status.success() {
        bail!("mihomo version command failed for {}", core.display());
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let text = if stdout.trim().is_empty() {
        stderr.trim()
    } else {
        stdout.trim()
    };
    text.lines()
        .find(|line| !line.trim().is_empty())
        .map(|line| line.trim().to_string())
        .with_context(|| format!("mihomo version output was empty for {}", core.display()))
}

pub fn require_core(paths: &StarailPaths) -> Result<PathBuf> {
    find_core(paths).with_context(|| {
        format!(
            "mihomo core is missing. Run 'starail core install' first. Expected managed core: {}",
            paths.core_file.display()
        )
    })
}

pub fn find_core(paths: &StarailPaths) -> Option<PathBuf> {
    if is_executable(&paths.core_file) {
        return Some(paths.core_file.clone());
    }

    find_in_path("mihomo")
}

fn latest_core_asset(language: Language) -> Result<CoreAsset> {
    let api = format!("https://api.github.com/repos/{MIHOMO_REPO}/releases/latest");
    println!("{}", language.tr(Message::FetchingLatestMihomoRelease));

    let client = reqwest::blocking::Client::builder()
        .user_agent(USER_AGENT)
        .timeout(Duration::from_secs(30))
        .build()
        .context("failed to create HTTP client")?;

    let release = client
        .get(&api)
        .send()
        .with_context(|| format!("failed to query {api}. Check network access and try again"))?
        .error_for_status()
        .with_context(|| format!("failed to query latest mihomo release: {api}"))?
        .json::<Release>()
        .context("failed to parse mihomo release metadata")?;

    let patterns = architecture_patterns()?;
    for pattern in patterns {
        if let Some(asset) = release.assets.iter().find(|asset| {
            asset.name.contains(pattern)
                && asset.browser_download_url.ends_with(".gz")
                && !asset.name.contains("go")
        }) {
            return Ok(CoreAsset {
                release_version: release.tag_name,
                url: asset.browser_download_url.clone(),
            });
        }
    }

    bail!("could not find a linux mihomo .gz asset for this architecture in the latest release")
}

fn versions_match(current: &str, latest: &str) -> bool {
    let current = normalize_version_text(current);
    let latest = normalize_version_text(latest);
    !latest.is_empty() && current.contains(&latest)
}

fn normalize_version_text(value: &str) -> String {
    value.trim().trim_start_matches('v').to_ascii_lowercase()
}

fn architecture_patterns() -> Result<Vec<&'static str>> {
    let arch = uname_arch().unwrap_or_else(|| env::consts::ARCH.to_string());
    match arch.as_str() {
        "x86_64" | "amd64" => Ok(vec!["linux-amd64-compatible", "linux-amd64"]),
        "aarch64" | "arm64" => Ok(vec!["linux-arm64"]),
        "armv7l" | "armv7" => Ok(vec!["linux-armv7"]),
        "armv6l" | "armv6" => Ok(vec!["linux-armv6"]),
        "i386" | "i686" | "x86" => Ok(vec!["linux-386"]),
        _ => bail!("unsupported CPU architecture for automatic core install: {arch}"),
    }
}

fn uname_arch() -> Option<String> {
    let output = Command::new("uname").arg("-m").output().ok()?;
    if !output.status.success() {
        return None;
    }
    String::from_utf8(output.stdout)
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

fn find_in_path(binary: &str) -> Option<PathBuf> {
    let path = env::var_os("PATH")?;
    env::split_paths(&path)
        .map(|dir| dir.join(binary))
        .find(|candidate| is_executable(candidate))
}

fn is_executable(path: &Path) -> bool {
    fs::metadata(path)
        .map(|metadata| metadata.is_file() && metadata.permissions().mode() & 0o111 != 0)
        .unwrap_or(false)
}

pub fn describe(paths: &StarailPaths) -> String {
    find_core(paths)
        .map(|path| display_path(&path))
        .unwrap_or_else(|| "missing".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matches_release_tag_in_core_version_output() {
        assert!(versions_match("Mihomo Meta v1.19.4 linux amd64", "v1.19.4"));
    }

    #[test]
    fn detects_different_core_version() {
        assert!(!versions_match(
            "Mihomo Meta v1.19.3 linux amd64",
            "v1.19.4"
        ));
    }
}
