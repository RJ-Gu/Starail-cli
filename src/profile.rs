use std::fs;
use std::path::Path;

use anyhow::{bail, Context, Result};

use crate::config::AppConfig;
use crate::paths::StarailPaths;
use crate::platform;
use crate::runtime;
use crate::util::{require_profile_name, unix_timestamp};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProfileKind {
    Local,
    Subscription,
}

impl ProfileKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Local => "local",
            Self::Subscription => "subscription",
        }
    }

    fn parse(value: &str) -> Self {
        match value {
            "subscription" => Self::Subscription,
            _ => Self::Local,
        }
    }
}

#[derive(Debug, Clone)]
pub struct ProfileMeta {
    pub name: String,
    pub kind: ProfileKind,
    pub source_url: Option<String>,
    pub updated_at: String,
}

#[derive(Debug, Clone)]
pub struct ProfileSummary {
    pub name: String,
    pub kind: ProfileKind,
    pub source_url: Option<String>,
    pub active: bool,
}

pub fn add_local(paths: &StarailPaths, name: &str, config: &Path) -> Result<()> {
    platform::require_linux()?;
    paths.init()?;
    require_profile_name(name)?;
    if !config.is_file() {
        bail!("config file does not exist: {}", config.display());
    }
    if exists(paths, name) {
        bail!("profile '{name}' already exists. Remove it first or choose another name.");
    }

    runtime::validate_config(paths, config)?;
    fs::copy(config, paths.profile_config_path(name)).with_context(|| {
        format!(
            "failed to copy {} into {}",
            config.display(),
            paths.profile_dir.display()
        )
    })?;

    write_meta(
        paths,
        &ProfileMeta {
            name: name.to_string(),
            kind: ProfileKind::Local,
            source_url: None,
            updated_at: unix_timestamp().to_string(),
        },
    )?;

    let mut app_config = AppConfig::load(paths)?;
    app_config.active_profile = Some(name.to_string());
    app_config.save(paths)?;
    println!("Added local profile '{name}' and selected it.");
    Ok(())
}

pub fn print_list(paths: &StarailPaths) -> Result<()> {
    let profiles = list(paths)?;
    let active = AppConfig::load(paths)?.active_profile;

    println!("Active profile: {}", active.as_deref().unwrap_or("none"));
    println!("{:<2} {:<28} {:<14} SOURCE", "", "NAME", "TYPE");
    for profile in &profiles {
        println!(
            "{:<2} {:<28} {:<14} {}",
            if profile.active { "*" } else { "" },
            profile.name,
            profile.kind.as_str(),
            profile.source_url.as_deref().unwrap_or_default()
        );
    }

    if profiles.is_empty() {
        println!(
            "No profiles yet. Add one with 'starail subscribe add <url>' or 'starail profile add <name> <config>'."
        );
    }

    Ok(())
}

pub fn use_profile(paths: &StarailPaths, name: &str) -> Result<()> {
    paths.init()?;
    require_profile_name(name)?;
    if !exists(paths, name) {
        bail!("profile '{name}' does not exist.");
    }

    let mut app_config = AppConfig::load(paths)?;
    app_config.active_profile = Some(name.to_string());
    app_config.save(paths)?;
    println!("Selected profile '{name}'.");
    Ok(())
}

pub fn remove(paths: &StarailPaths, name: &str) -> Result<()> {
    paths.init()?;
    require_profile_name(name)?;
    if !exists(paths, name) {
        bail!("profile '{name}' does not exist.");
    }

    let config_path = paths.profile_config_path(name);
    let meta_path = paths.profile_meta_path(name);
    if config_path.exists() {
        fs::remove_file(&config_path)
            .with_context(|| format!("failed to remove {}", config_path.display()))?;
    }
    if meta_path.exists() {
        fs::remove_file(&meta_path)
            .with_context(|| format!("failed to remove {}", meta_path.display()))?;
    }

    let mut app_config = AppConfig::load(paths)?;
    if app_config.active_profile.as_deref() == Some(name) {
        app_config.active_profile = None;
        app_config.save(paths)?;
    }

    println!("Removed profile '{name}'.");
    Ok(())
}

pub fn list(paths: &StarailPaths) -> Result<Vec<ProfileSummary>> {
    paths.init()?;
    let active = AppConfig::load(paths)?.active_profile;
    let mut profiles = Vec::new();

    for entry in fs::read_dir(&paths.profile_dir)
        .with_context(|| format!("failed to read {}", paths.profile_dir.display()))?
    {
        let entry = entry?;
        let path = entry.path();
        if path.extension().and_then(|value| value.to_str()) != Some("yaml") {
            continue;
        }

        let Some(name) = path.file_stem().and_then(|value| value.to_str()) else {
            continue;
        };

        let meta = read_meta(paths, name).unwrap_or_else(|_| ProfileMeta {
            name: name.to_string(),
            kind: ProfileKind::Local,
            source_url: None,
            updated_at: String::new(),
        });

        profiles.push(ProfileSummary {
            name: name.to_string(),
            kind: meta.kind,
            source_url: meta.source_url,
            active: active.as_deref() == Some(name),
        });
    }

    profiles.sort_by(|left, right| left.name.cmp(&right.name));
    Ok(profiles)
}

pub fn exists(paths: &StarailPaths, name: &str) -> bool {
    paths.profile_config_path(name).is_file()
}

pub fn read_meta(paths: &StarailPaths, name: &str) -> Result<ProfileMeta> {
    let path = paths.profile_meta_path(name);
    let text =
        fs::read_to_string(&path).with_context(|| format!("failed to read {}", path.display()))?;
    let mut meta_name = name.to_string();
    let mut kind = ProfileKind::Local;
    let mut source_url = None;
    let mut updated_at = String::new();

    for line in text.lines() {
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        match key {
            "name" if !value.trim().is_empty() => meta_name = value.trim().to_string(),
            "type" => kind = ProfileKind::parse(value.trim()),
            "source_url" if !value.trim().is_empty() => source_url = Some(value.trim().to_string()),
            "updated_at" => updated_at = value.trim().to_string(),
            _ => {}
        }
    }

    Ok(ProfileMeta {
        name: meta_name,
        kind,
        source_url,
        updated_at,
    })
}

pub fn write_meta(paths: &StarailPaths, meta: &ProfileMeta) -> Result<()> {
    let text = format!(
        "name={}\ntype={}\nsource_url={}\nupdated_at={}\n",
        meta.name,
        meta.kind.as_str(),
        meta.source_url.as_deref().unwrap_or_default(),
        meta.updated_at
    );
    fs::write(paths.profile_meta_path(&meta.name), text)
        .with_context(|| format!("failed to write profile metadata for '{}'", meta.name))?;
    Ok(())
}
