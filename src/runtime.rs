use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::thread;
use std::time::Duration;

use anyhow::{bail, Context, Result};

use crate::config::{app_language, AppConfig};
use crate::core;
use crate::i18n::Message;
use crate::paths::StarailPaths;
use crate::platform;
use crate::util::{json_string, unix_timestamp};

pub fn start(paths: &StarailPaths, config: Option<&Path>) -> Result<()> {
    platform::require_linux()?;
    paths.init()?;
    let language = app_language(paths);

    if is_running(paths) {
        bail!(
            "mihomo is already running with pid {}. Use 'starail restart' or 'starail stop' first.",
            read_pid(paths).unwrap_or_default()
        );
    }

    let source = match config {
        Some(path) => path.to_path_buf(),
        None => {
            let app_config = AppConfig::load(paths)?;
            let active = app_config
                .active_profile
                .as_deref()
                .with_context(|| {
                    "no active profile selected. Run 'starail profile use <name>' or 'starail start -c <config>'"
                })?
                .to_string();
            let path = paths.profile_config_path(&active);
            if !path.is_file() {
                bail!("active profile '{active}' is missing. Run 'starail profile list'.");
            }
            path
        }
    };

    let core = core::require_core(paths)?;
    let runtime = prepare_runtime_config(paths, &source)?;
    validate_with_core(paths, &core, &runtime)?;

    let mut log = OpenOptions::new()
        .create(true)
        .append(true)
        .open(&paths.log_file)
        .with_context(|| format!("failed to open {}", paths.log_file.display()))?;
    writeln!(
        log,
        "\n[{}] starting {} with {}",
        unix_timestamp(),
        core.display(),
        runtime.display()
    )?;

    let child = Command::new(&core)
        .arg("-f")
        .arg(&runtime)
        .arg("-d")
        .arg(&paths.home)
        .stdin(Stdio::null())
        .stdout(Stdio::from(log.try_clone()?))
        .stderr(Stdio::from(log))
        .spawn()
        .with_context(|| format!("failed to start {}", core.display()))?;

    let pid = child.id();
    fs::write(&paths.pid_file, format!("{pid}\n"))
        .with_context(|| format!("failed to write {}", paths.pid_file.display()))?;
    thread::sleep(Duration::from_secs(1));

    if !process_exists(pid) {
        let _ = fs::remove_file(&paths.pid_file);
        let recent_logs = tail_text(&paths.log_file, 40).unwrap_or_default();
        bail!("mihomo exited during startup. Recent logs:\n{recent_logs}");
    }

    println!("{} {pid}", language.tr(Message::MihomoStartedWithPid));
    println!(
        "{} {}",
        language.tr(Message::RuntimeConfigLabel),
        runtime.display()
    );
    println!(
        "{} {}",
        language.tr(Message::LogFileLabel),
        paths.log_file.display()
    );
    Ok(())
}

pub fn stop(paths: &StarailPaths) -> Result<()> {
    paths.init()?;
    let language = app_language(paths);

    if !is_running(paths) {
        let _ = fs::remove_file(&paths.pid_file);
        println!("{}", language.tr(Message::MihomoNotRunning));
        return Ok(());
    }

    let pid = read_pid(paths).context("pid file exists but did not contain a valid pid")?;
    send_signal(pid, libc::SIGTERM)?;

    for _ in 0..10 {
        if !process_exists(pid) {
            let _ = fs::remove_file(&paths.pid_file);
            println!("{}", language.tr(Message::MihomoStopped));
            return Ok(());
        }
        thread::sleep(Duration::from_millis(300));
    }

    send_signal(pid, libc::SIGKILL)?;
    let _ = fs::remove_file(&paths.pid_file);
    println!("{}", language.tr(Message::MihomoKilledAfterTerm));
    Ok(())
}

pub fn restart(paths: &StarailPaths) -> Result<()> {
    paths.init()?;
    if is_running(paths) {
        stop(paths)?;
    }
    start(paths, None)
}

pub fn check(paths: &StarailPaths, config: &Path) -> Result<()> {
    platform::require_linux()?;
    paths.init()?;
    validate_config(paths, config)?;
    println!("Config OK: {}", config.display());
    Ok(())
}

pub fn logs(paths: &StarailPaths, lines: usize) -> Result<()> {
    paths.init()?;
    if !paths.log_file.is_file() {
        bail!("log file does not exist yet: {}", paths.log_file.display());
    }
    print!("{}", tail_text(&paths.log_file, lines)?);
    Ok(())
}

pub fn validate_config(paths: &StarailPaths, config: &Path) -> Result<()> {
    if !config.is_file() {
        bail!("config file does not exist: {}", config.display());
    }
    let core = core::require_core(paths)?;
    validate_with_core(paths, &core, config)
}

pub fn read_pid(paths: &StarailPaths) -> Option<u32> {
    let text = fs::read_to_string(&paths.pid_file).ok()?;
    text.lines().next()?.trim().parse().ok()
}

pub fn is_running(paths: &StarailPaths) -> bool {
    read_pid(paths).map(process_exists).unwrap_or(false)
}

pub fn prepare_runtime_config(paths: &StarailPaths, source: &Path) -> Result<PathBuf> {
    if !source.is_file() {
        bail!("config file does not exist: {}", source.display());
    }

    let mut app_config = AppConfig::load(paths)?;
    let text = fs::read_to_string(source)
        .with_context(|| format!("failed to read config {}", source.display()))?;

    let profile_port = top_level_value(&text, "mixed-port")
        .as_deref()
        .map(parse_port)
        .transpose()
        .with_context(|| format!("invalid mixed-port in {}", source.display()))?;
    let effective_port = profile_port.unwrap_or(app_config.mixed_port);
    app_config.mixed_port = effective_port;
    app_config.save(paths)?;

    let runtime = paths.runtime_dir.join("config.yaml");
    let mut rendered = strip_runtime_keys(&text);
    if !rendered.ends_with('\n') {
        rendered.push('\n');
    }
    rendered.push_str("\n# Starail runtime settings\n");
    if profile_port.is_none() {
        rendered.push_str(&format!("mixed-port: {effective_port}\n"));
    }
    rendered.push_str(&format!(
        "external-controller: {}\n",
        app_config.controller_address
    ));
    rendered.push_str(&format!(
        "secret: {}\n",
        json_string(app_config.controller_secret.as_deref().unwrap_or_default())
    ));

    fs::write(&runtime, rendered)
        .with_context(|| format!("failed to write runtime config {}", runtime.display()))?;
    Ok(runtime)
}

fn validate_with_core(paths: &StarailPaths, core: &Path, config: &Path) -> Result<()> {
    let output = Command::new(core)
        .arg("-t")
        .arg("-f")
        .arg(config)
        .arg("-d")
        .arg(&paths.home)
        .output()
        .with_context(|| {
            format!(
                "failed to validate {} with {}",
                config.display(),
                core.display()
            )
        })?;

    if output.status.success() {
        return Ok(());
    }

    let mut message = String::from_utf8_lossy(&output.stderr).into_owned();
    if message.trim().is_empty() {
        message = String::from_utf8_lossy(&output.stdout).into_owned();
    }
    let preview = message.lines().take(120).collect::<Vec<_>>().join("\n");
    bail!(
        "Config validation failed for {}\n{}",
        config.display(),
        validation_hint(paths, &preview)
    )
}

fn validation_hint(paths: &StarailPaths, message: &str) -> String {
    if is_geodata_download_error(message) {
        format!(
            "{message}\n\nStarail note: mihomo needs GeoIP/MMDB data for GEOIP rules, but it could not download geoip.metadb during validation. \
The validation now uses Starail's home directory (-d {}), so you can retry after network access to GitHub works, or place a compatible geoip.metadb in that directory.",
            paths.home.display()
        )
    } else {
        message.to_string()
    }
}

fn is_geodata_download_error(message: &str) -> bool {
    let lower = message.to_ascii_lowercase();
    (lower.contains("can't download mmdb")
        || lower.contains("can't initial geoip")
        || lower.contains("can't find mmdb"))
        && (lower.contains("geoip.metadb") || lower.contains("mmdb"))
}

fn top_level_value(text: &str, key: &str) -> Option<String> {
    for line in text.lines() {
        let trimmed = line.trim_end();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        let first = trimmed.as_bytes().first().copied()?;
        if first.is_ascii_whitespace() {
            continue;
        }
        let Some((candidate, value)) = trimmed.split_once(':') else {
            continue;
        };
        if candidate.trim() == key {
            return Some(
                value
                    .trim()
                    .trim_matches('"')
                    .trim_matches('\'')
                    .to_string(),
            );
        }
    }
    None
}

fn strip_runtime_keys(text: &str) -> String {
    text.lines()
        .filter(|line| {
            let trimmed = line.trim_end();
            if trimmed.is_empty() || trimmed.starts_with('#') {
                return true;
            }
            let Some(first) = trimmed.as_bytes().first().copied() else {
                return true;
            };
            if first.is_ascii_whitespace() {
                return true;
            }
            let Some((key, _)) = trimmed.split_once(':') else {
                return true;
            };
            !matches!(key.trim(), "external-controller" | "secret")
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn parse_port(value: &str) -> Result<u16> {
    value
        .parse::<u16>()
        .with_context(|| format!("expected a numeric TCP port, got '{value}'"))
}

fn process_exists(pid: u32) -> bool {
    let signal_result = unsafe { libc::kill(pid as libc::pid_t, 0) };
    signal_result == 0 || Path::new("/proc").join(pid.to_string()).exists()
}

fn send_signal(pid: u32, signal: libc::c_int) -> Result<()> {
    let result = unsafe { libc::kill(pid as libc::pid_t, signal) };
    if result == 0 {
        Ok(())
    } else {
        bail!("failed to send signal {signal} to pid {pid}")
    }
}

fn tail_text(path: &Path, lines: usize) -> Result<String> {
    let mut text = String::new();
    File::open(path)
        .with_context(|| format!("failed to open {}", path.display()))?
        .read_to_string(&mut text)
        .with_context(|| format!("failed to read {}", path.display()))?;

    if lines == 0 {
        return Ok(String::new());
    }

    let mut tail = text.lines().rev().take(lines).collect::<Vec<_>>();
    tail.reverse();
    let mut output = tail.join("\n");
    if !output.is_empty() {
        output.push('\n');
    }
    Ok(output)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recognizes_geodata_download_failures() {
        let message = "can't initial GeoIP: can't download MMDB: Get https://github.com/MetaCubeX/meta-rules-dat/releases/download/latest/geoip.metadb: context deadline exceeded";

        assert!(is_geodata_download_error(message));
    }

    #[test]
    fn leaves_unrelated_validation_errors_alone() {
        assert!(!is_geodata_download_error("yaml: unmarshal errors"));
    }
}
