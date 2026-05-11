use anyhow::{Context, Result, anyhow};
use serde::{Deserialize, Serialize};
use std::{
    env, fs,
    path::{Path, PathBuf},
};

pub const CONFIG_ENV: &str = "AURA_MCP_CONFIG";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Config {
    pub api_endpoint: String,
    pub api_key: Option<String>,
    #[serde(default)]
    pub read_only: bool,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            api_endpoint: aura_api_client::consts::AURA_API_LINK.to_owned(),
            api_key: None,
            read_only: true,
        }
    }
}

impl Config {
    pub fn load(path: &Path) -> Result<Self> {
        let raw = fs::read_to_string(path)
            .with_context(|| format!("failed to read config at {}", path.display()))?;
        toml::from_str(&raw).with_context(|| format!("invalid TOML in {}", path.display()))
    }

    pub fn save(&self, path: &Path) -> Result<()> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)
                .with_context(|| format!("failed to create {}", parent.display()))?;
        }

        let data = toml::to_string_pretty(self).context("failed to serialize config")?;
        write_restrictive(path, data.as_bytes())
            .with_context(|| format!("failed to write {}", path.display()))
    }

    pub fn validate_for_api(&self) -> Result<()> {
        if self.api_endpoint.trim().is_empty() {
            return Err(anyhow!("api_endpoint is empty"));
        }
        if self
            .api_key
            .as_deref()
            .unwrap_or_default()
            .trim()
            .is_empty()
        {
            return Err(anyhow!("api_key is missing"));
        }
        Ok(())
    }
}

pub fn config_path() -> Result<PathBuf> {
    if let Ok(path) = env::var(CONFIG_ENV) {
        return Ok(PathBuf::from(path));
    }
    default_config_path()
}

pub fn default_config_path() -> Result<PathBuf> {
    let home = env::var_os("HOME").ok_or_else(|| anyhow!("HOME is not set"))?;
    Ok(PathBuf::from(home).join(".config/aura/mcp.toml"))
}

pub fn resolve_config_path(env_override: Option<&str>, home: &Path) -> PathBuf {
    env_override
        .map(PathBuf::from)
        .unwrap_or_else(|| home.join(".config/aura/mcp.toml"))
}

#[cfg(unix)]
fn write_restrictive(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};

    let mut opts = fs::OpenOptions::new();
    opts.create(true).truncate(true).write(true).mode(0o600);
    std::io::Write::write_all(&mut opts.open(path)?, bytes)?;
    fs::set_permissions(path, fs::Permissions::from_mode(0o600))?;
    Ok(())
}

#[cfg(not(unix))]
fn write_restrictive(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    fs::write(path, bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp_path(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!("aura-mcp-test-{}-{name}", std::process::id()))
    }

    #[test]
    fn config_roundtrip() {
        let path = tmp_path("roundtrip.toml");
        let _ = fs::remove_file(&path);
        let cfg = Config {
            api_endpoint: "http://localhost:40051".into(),
            api_key: Some("11111111111111111111111111111111".into()),
            read_only: false,
        };

        cfg.save(&path).unwrap();
        assert_eq!(Config::load(&path).unwrap(), cfg);
        let _ = fs::remove_file(path);
    }

    #[test]
    fn path_resolution_prefers_env_override() {
        let home = Path::new("/home/alice");
        assert_eq!(
            resolve_config_path(Some("/tmp/aura.toml"), home),
            PathBuf::from("/tmp/aura.toml")
        );
        assert_eq!(
            resolve_config_path(None, home),
            PathBuf::from("/home/alice/.config/aura/mcp.toml")
        );
    }
}
