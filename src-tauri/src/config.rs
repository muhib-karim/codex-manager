use anyhow::{Context, Result};
use base64::{engine::general_purpose::STANDARD as BASE64, Engine};
use rand::rngs::OsRng;
use rand::RngCore;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use tauri::Manager;
use tracing::info;

use crate::models::{AppConfig, ProxyServerConfig, RoutingConfig};

/// Environment variable that overrides the generated master key.
pub const MASTER_KEY_ENV: &str = "CODEX_MANAGER_MASTER_KEY";
const MASTER_KEY_FILE: &str = "master.key";
const CONFIG_FILE: &str = "config.json";
const DB_FILE: &str = "accounts.db";

/// Get the application data directory
pub fn get_data_dir(app_handle: &tauri::AppHandle) -> Result<PathBuf> {
    let path = app_handle
        .path()
        .app_data_dir()
        .map_err(|e| anyhow::anyhow!("Failed to get app data directory: {e}"))?;
    fs::create_dir_all(&path).context("Failed to create app data directory")?;
    Ok(path)
}

/// Get the configuration file path
pub fn get_config_path(app_handle: &tauri::AppHandle) -> Result<PathBuf> {
    let data_dir = get_data_dir(app_handle)?;
    Ok(data_dir.join(CONFIG_FILE))
}

/// Get the database file path
pub fn get_db_path(app_handle: &tauri::AppHandle) -> Result<PathBuf> {
    let data_dir = get_data_dir(app_handle)?;
    Ok(data_dir.join(DB_FILE))
}

/// Read the configuration from a file, or the defaults when it is missing
pub fn read_config_file(path: &Path) -> Result<AppConfig> {
    if !path.exists() {
        return Ok(AppConfig::default());
    }
    let content = fs::read_to_string(path).context("Failed to read configuration file")?;
    let config: AppConfig =
        serde_json::from_str(&content).context("Failed to parse configuration file")?;
    Ok(config)
}

/// Write the configuration to a file
pub fn write_config_file(path: &Path, config: &AppConfig) -> Result<()> {
    let content =
        serde_json::to_string_pretty(config).context("Failed to serialize configuration")?;
    fs::write(path, content).context("Failed to write configuration file")?;
    Ok(())
}

/// Load configuration from file or create default
pub fn load_config(app_handle: &tauri::AppHandle) -> Result<AppConfig> {
    let config_path = get_config_path(app_handle)?;
    if !config_path.exists() {
        info!("Configuration not found, creating default");
        let config = AppConfig::default();
        write_config_file(&config_path, &config)?;
        return Ok(config);
    }
    read_config_file(&config_path)
}

/// Save configuration to file
pub fn save_config(app_handle: &tauri::AppHandle, config: &AppConfig) -> Result<()> {
    let config_path = get_config_path(app_handle)?;
    write_config_file(&config_path, config)
}

/// Update proxy configuration
pub fn update_proxy_config(
    app_handle: &tauri::AppHandle,
    proxy_config: ProxyServerConfig,
) -> Result<()> {
    let mut config = load_config(app_handle)?;
    config.proxy = proxy_config;
    save_config(app_handle, &config)
}

/// Update routing configuration
pub fn update_routing_config(
    app_handle: &tauri::AppHandle,
    routing_config: RoutingConfig,
) -> Result<()> {
    let mut config = load_config(app_handle)?;
    config.routing = routing_config;
    save_config(app_handle, &config)
}

/// Create a file that only the current user can read (0600 on Unix).
/// Fails if the file already exists.
fn write_private_file(path: &Path, contents: &str) -> Result<()> {
    let mut options = fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options
        .open(path)
        .context("Failed to create master key file")?;
    file.write_all(contents.as_bytes())
        .context("Failed to write master key file")?;
    Ok(())
}

/// Resolve the master key used to encrypt stored API keys.
///
/// 1. A non-empty value from the environment (`CODEX_MANAGER_MASTER_KEY`) wins.
/// 2. Otherwise the key file in the data directory is used.
/// 3. On first run a random 256-bit key is generated and written to that file.
///
/// There is deliberately no built-in default key: a constant shared by every
/// install would make the encryption meaningless.
pub fn load_or_create_master_key(data_dir: &Path, env_value: Option<String>) -> Result<String> {
    if let Some(value) = env_value {
        if !value.trim().is_empty() {
            return Ok(value);
        }
    }

    let path = data_dir.join(MASTER_KEY_FILE);
    if path.exists() {
        let stored = fs::read_to_string(&path).context("Failed to read master key file")?;
        let key = stored.trim().to_string();
        if key.is_empty() {
            anyhow::bail!("Master key file is empty");
        }
        return Ok(key);
    }

    let mut bytes = [0u8; 32];
    OsRng.fill_bytes(&mut bytes);
    let key = BASE64.encode(bytes);
    write_private_file(&path, &key)?;
    info!("Generated a new master key file in the app data directory");
    Ok(key)
}

/// Get the master key for this installation
pub fn get_master_key(app_handle: &tauri::AppHandle) -> Result<String> {
    let data_dir = get_data_dir(app_handle)?;
    load_or_create_master_key(&data_dir, std::env::var(MASTER_KEY_ENV).ok())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::RoutingStrategy;

    /// Unique scratch directory in the OS temp dir; removed when dropped.
    struct TempDir {
        path: PathBuf,
    }

    impl TempDir {
        fn new() -> Self {
            let name = format!("codex-manager-test-{}", uuid::Uuid::new_v4());
            let path = std::env::temp_dir().join(name);
            fs::create_dir_all(&path).unwrap();
            Self { path }
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.path);
        }
    }

    // Regression: the backend used to fall back to a hard-coded key that
    // was identical on every install.
    #[test]
    fn master_key_is_random_per_install_and_reused() {
        let first_dir = TempDir::new();
        let second_dir = TempDir::new();

        let first = load_or_create_master_key(&first_dir.path, None).unwrap();
        let again = load_or_create_master_key(&first_dir.path, None).unwrap();
        let other = load_or_create_master_key(&second_dir.path, None).unwrap();

        assert_eq!(first, again);
        assert_ne!(first, other);
        assert!(first.len() >= 40);
        assert!(first_dir.path.join(MASTER_KEY_FILE).exists());
    }

    #[test]
    fn environment_value_overrides_the_key_file() {
        let dir = TempDir::new();
        let key = load_or_create_master_key(&dir.path, Some("from-env".to_string())).unwrap();
        assert_eq!(key, "from-env");
        // Nothing is written when the environment provides the key.
        assert!(!dir.path.join(MASTER_KEY_FILE).exists());
    }

    #[test]
    fn blank_environment_value_is_ignored() {
        let dir = TempDir::new();
        let key = load_or_create_master_key(&dir.path, Some("   ".to_string())).unwrap();
        assert_ne!(key.trim(), "");
        assert!(dir.path.join(MASTER_KEY_FILE).exists());
    }

    #[test]
    fn empty_key_file_is_an_error() {
        let dir = TempDir::new();
        fs::write(dir.path.join(MASTER_KEY_FILE), "\n").unwrap();
        assert!(load_or_create_master_key(&dir.path, None).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn master_key_file_is_private_to_the_user() {
        use std::os::unix::fs::PermissionsExt;
        let dir = TempDir::new();
        load_or_create_master_key(&dir.path, None).unwrap();
        let metadata = fs::metadata(dir.path.join(MASTER_KEY_FILE)).unwrap();
        assert_eq!(metadata.permissions().mode() & 0o777, 0o600);
    }

    #[test]
    fn config_file_round_trips_and_defaults_when_missing() {
        let dir = TempDir::new();
        let path = dir.path.join(CONFIG_FILE);

        let defaults = read_config_file(&path).unwrap();
        assert_eq!(defaults.proxy.bind_addr, "127.0.0.1:8080");
        assert_eq!(defaults.routing.strategy, RoutingStrategy::LeastUtilized);

        let config = AppConfig {
            proxy: ProxyServerConfig {
                bind_addr: "127.0.0.1:9090".to_string(),
                ..ProxyServerConfig::default()
            },
            routing: RoutingConfig {
                strategy: RoutingStrategy::Sticky,
                ..RoutingConfig::default()
            },
        };
        write_config_file(&path, &config).unwrap();

        let loaded = read_config_file(&path).unwrap();
        assert_eq!(loaded.routing.strategy, RoutingStrategy::Sticky);
        assert_eq!(loaded.proxy.bind_addr, "127.0.0.1:9090");
    }

    #[test]
    fn malformed_config_file_is_an_error() {
        let dir = TempDir::new();
        let path = dir.path.join(CONFIG_FILE);
        fs::write(&path, "{ not json").unwrap();
        assert!(read_config_file(&path).is_err());
    }
}
