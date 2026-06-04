use std::fs;

use anyhow::{Context, Result};

use crate::config::AppConfig;
use crate::paths::{user_home, StarailPaths};

const START: &str = "# >>> starail proxy >>>";
const END: &str = "# <<< starail proxy <<<";

pub fn on(paths: &StarailPaths) -> Result<()> {
    paths.init()?;
    let mut config = AppConfig::load(paths)?;
    let bashrc = user_home()?.join(".bashrc");
    let existing = fs::read_to_string(&bashrc).unwrap_or_default();
    let mut next = remove_block_text(&existing);
    if !next.is_empty() && !next.ends_with('\n') {
        next.push('\n');
    }
    next.push_str(&block(config.mixed_port));
    fs::write(&bashrc, next).with_context(|| format!("failed to write {}", bashrc.display()))?;

    config.shell_proxy_enabled = true;
    config.save(paths)?;
    println!("Shell proxy block enabled in {}", bashrc.display());
    println!(
        "New shells will use http://127.0.0.1:{}. Existing shells are unchanged.",
        config.mixed_port
    );
    Ok(())
}

pub fn off(paths: &StarailPaths) -> Result<()> {
    paths.init()?;
    let mut config = AppConfig::load(paths)?;
    let bashrc = user_home()?.join(".bashrc");
    let existing = fs::read_to_string(&bashrc).unwrap_or_default();
    fs::write(&bashrc, remove_block_text(&existing))
        .with_context(|| format!("failed to write {}", bashrc.display()))?;

    config.shell_proxy_enabled = false;
    config.save(paths)?;
    println!("Shell proxy block removed from {}", bashrc.display());
    Ok(())
}

pub fn status(paths: &StarailPaths) -> Result<()> {
    paths.init()?;
    let config = AppConfig::load(paths)?;
    let bashrc = user_home()?.join(".bashrc");
    let present = fs::read_to_string(&bashrc)
        .map(|text| block_present_in(&text))
        .unwrap_or(false);

    println!(
        "Shell proxy block: {}",
        if present { "present" } else { "absent" }
    );
    println!(
        "Starail setting: shell_proxy_enabled={}",
        config.shell_proxy_enabled
    );
    Ok(())
}

pub fn block_present(paths: &StarailPaths) -> bool {
    let Ok(bashrc) = user_home().map(|home| home.join(".bashrc")) else {
        return false;
    };
    let _ = paths;
    fs::read_to_string(bashrc)
        .map(|text| block_present_in(&text))
        .unwrap_or(false)
}

fn block(port: u16) -> String {
    let proxy = format!("http://127.0.0.1:{port}");
    format!(
        "{START}\n\
export http_proxy=\"{proxy}\"\n\
export https_proxy=\"{proxy}\"\n\
export all_proxy=\"{proxy}\"\n\
export HTTP_PROXY=\"{proxy}\"\n\
export HTTPS_PROXY=\"{proxy}\"\n\
export ALL_PROXY=\"{proxy}\"\n\
export no_proxy=\"localhost,127.0.0.1,::1\"\n\
export NO_PROXY=\"localhost,127.0.0.1,::1\"\n\
{END}\n"
    )
}

fn block_present_in(text: &str) -> bool {
    text.lines().any(|line| line == START)
}

fn remove_block_text(text: &str) -> String {
    let mut output = Vec::new();
    let mut skip = false;

    for line in text.lines() {
        if line == START {
            skip = true;
            continue;
        }
        if line == END {
            skip = false;
            continue;
        }
        if !skip {
            output.push(line);
        }
    }

    let mut text = output.join("\n");
    if !text.is_empty() {
        text.push('\n');
    }
    text
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn removes_only_starail_block() {
        let text = "before\n# >>> starail proxy >>>\nexport http_proxy=\"x\"\n# <<< starail proxy <<<\nafter\n";
        assert_eq!(remove_block_text(text), "before\nafter\n");
    }
}
