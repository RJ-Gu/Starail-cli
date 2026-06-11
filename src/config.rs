use std::fs;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use crate::i18n::{self, Language};
use crate::paths::StarailPaths;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct AppConfig {
    pub active_profile: Option<String>,
    pub mixed_port: u16,
    pub controller_address: String,
    pub controller_secret: Option<String>,
    pub latency_test_url: String,
    pub latency_test_timeout: u64,
    pub shell_proxy_enabled: bool,
    pub language: Option<String>,
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            active_profile: None,
            mixed_port: 7890,
            controller_address: "127.0.0.1:9090".to_string(),
            controller_secret: None,
            latency_test_url: "https://www.gstatic.com/generate_204".to_string(),
            latency_test_timeout: 5000,
            shell_proxy_enabled: false,
            language: None,
        }
    }
}

impl AppConfig {
    pub fn load(paths: &StarailPaths) -> Result<Self> {
        paths.init()?;
        let text = fs::read_to_string(&paths.config_file)
            .with_context(|| format!("failed to read {}", paths.config_file.display()))?;

        let config = if text.trim().is_empty() {
            Self::default()
        } else {
            serde_yaml::from_str::<Self>(&text)
                .with_context(|| format!("failed to parse {}", paths.config_file.display()))?
        };

        Ok(config.normalized())
    }

    pub fn save(&self, paths: &StarailPaths) -> Result<()> {
        paths.ensure_dirs()?;
        let text = serde_yaml::to_string(&self.normalized())
            .context("failed to serialize Starail config")?;
        fs::write(&paths.config_file, text)
            .with_context(|| format!("failed to write {}", paths.config_file.display()))?;
        Ok(())
    }

    pub fn controller_base(&self) -> String {
        format!(
            "http://{}",
            self.controller_address.trim().trim_end_matches('/')
        )
    }

    pub fn language(&self) -> Language {
        i18n::configured(self.language.as_deref())
    }

    pub fn normalized(&self) -> Self {
        let mut config = self.clone();

        if config
            .active_profile
            .as_deref()
            .map(str::trim)
            .unwrap_or_default()
            .is_empty()
        {
            config.active_profile = None;
        }

        if config
            .controller_secret
            .as_deref()
            .map(str::trim)
            .unwrap_or_default()
            .is_empty()
        {
            config.controller_secret = None;
        }

        if config.controller_address.trim().is_empty() {
            config.controller_address = Self::default().controller_address;
        }

        if config.latency_test_url.trim().is_empty() {
            config.latency_test_url = Self::default().latency_test_url;
        }

        if config.latency_test_timeout == 0 {
            config.latency_test_timeout = Self::default().latency_test_timeout;
        }

        config.language = config
            .language
            .as_deref()
            .and_then(Language::from_code)
            .map(|language| language.code().to_string());

        config
    }
}

pub fn app_language(paths: &StarailPaths) -> Language {
    AppConfig::load(paths)
        .map(|config| config.language())
        .unwrap_or_else(|_| i18n::detect())
}
