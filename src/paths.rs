use std::env;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

#[derive(Debug, Clone)]
pub struct StarailPaths {
    pub home: PathBuf,
    pub config_file: PathBuf,
    pub bin_dir: PathBuf,
    pub profile_dir: PathBuf,
    pub runtime_dir: PathBuf,
    pub run_dir: PathBuf,
    pub log_dir: PathBuf,
    pub tmp_dir: PathBuf,
    pub core_file: PathBuf,
    pub pid_file: PathBuf,
    pub log_file: PathBuf,
}

impl StarailPaths {
    pub fn new() -> Result<Self> {
        let home = match env::var_os("STARAIL_HOME") {
            Some(value) if !value.is_empty() => PathBuf::from(value),
            _ => user_home()?.join(".starail"),
        };

        let bin_dir = home.join("bin");
        let profile_dir = home.join("profiles");
        let runtime_dir = home.join("runtime");
        let run_dir = home.join("run");
        let log_dir = home.join("logs");
        let tmp_dir = home.join("tmp");

        Ok(Self {
            config_file: home.join("config.yaml"),
            core_file: bin_dir.join("mihomo"),
            pid_file: run_dir.join("mihomo.pid"),
            log_file: log_dir.join("mihomo.log"),
            home,
            bin_dir,
            profile_dir,
            runtime_dir,
            run_dir,
            log_dir,
            tmp_dir,
        })
    }

    pub fn init(&self) -> Result<()> {
        self.ensure_dirs()?;
        if !self.config_file.exists() {
            crate::config::AppConfig::default().save(self)?;
        }
        Ok(())
    }

    pub fn ensure_dirs(&self) -> Result<()> {
        for dir in [
            &self.home,
            &self.bin_dir,
            &self.profile_dir,
            &self.runtime_dir,
            &self.run_dir,
            &self.log_dir,
            &self.tmp_dir,
        ] {
            fs::create_dir_all(dir)
                .with_context(|| format!("failed to create directory {}", dir.display()))?;
        }
        Ok(())
    }

    pub fn profile_config_path(&self, name: &str) -> PathBuf {
        self.profile_dir.join(format!("{name}.yaml"))
    }

    pub fn profile_meta_path(&self, name: &str) -> PathBuf {
        self.profile_dir.join(format!("{name}.meta"))
    }
}

pub fn user_home() -> Result<PathBuf> {
    env::var_os("HOME")
        .filter(|home| !home.is_empty())
        .map(PathBuf::from)
        .context("HOME is required")
}

pub fn display_path(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}
