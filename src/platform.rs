use anyhow::{bail, Result};

pub fn require_linux() -> Result<()> {
    if std::env::consts::OS == "linux" {
        Ok(())
    } else {
        bail!(
            "Linux is required for this first version. Detected: {}.",
            std::env::consts::OS
        )
    }
}
