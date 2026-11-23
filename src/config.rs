/// Configuration module for Biblio
/// 
/// This module loads configuration from a YAML file at runtime.
/// 
/// Location:
/// - If APP_IN_DOCKER environment variable is set to "true", config.yaml is expected
///   at `/config/config.yaml` (a mounted volume in Docker)
/// - Otherwise, config.yaml is expected in the current working directory
/// 
/// Path Resolution:
/// - Relative paths in config.yaml are resolved relative to the base directory
///   (either `/config` for Docker or current directory for standard Linux)
/// - Absolute paths are used as-is
/// 
/// See `config.yaml.example` for setup instructions.

use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use tracing::{error, info, warn};

/// A single configured Calibre library: explicit display name + path.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LibraryEntry {
    pub name: String,
    pub path: String,
}

/// Runtime configuration loaded from config.yaml
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    /// Legacy: path to a directory containing Calibre libraries (or one library).
    /// Used as fallback when `library_paths` is empty.
    #[serde(default)]
    pub library_path: String,

    /// Explicit list of Calibre libraries. Each item is the library root
    /// (directory holding metadata.db) or a direct path to metadata.db.
    #[serde(default)]
    pub library_paths: Vec<LibraryEntry>,

    /// IP and port for the Biblio server to bind to
    #[serde(default = "default_service_addr")]
    pub service_ip_and_port: String,

    /// Path to the users.ids file containing user credentials
    #[serde(default = "default_users_file")]
    pub users_file_path: String,

    /// Enable HTTPS/TLS
    #[serde(default)]
    pub use_https: bool,

    /// Path to the SSL/TLS certificate file (PEM format)
    #[serde(default = "default_cert")]
    pub certificate_path: String,

    /// Path to the SSL/TLS private key file (PEM format)
    #[serde(default = "default_key")]
    pub private_key_path: String,

    /// Local cache directory (DB snapshots + cover thumbnails).
    /// Relative paths are resolved against the base directory, so the whole
    /// application folder stays self-contained and movable.
    #[serde(default = "default_cache_dir")]
    pub cache_dir: String,

    /// Longest side (pixels) of the generated cover thumbnails served to the grid
    #[serde(default = "default_thumbnail_size")]
    pub thumbnail_size: u32,

    /// Snapshot the Calibre database to local cache: "auto" (only for remote /
    /// different-volume libraries), "always", or "never".
    #[serde(default = "default_db_snapshot")]
    pub db_snapshot: String,

    /// Library-independent category table used when a book has no row in the
    /// category file of the library it now lives in (e.g. it was copied there
    /// and received a new Calibre id).
    #[serde(default = "default_classification_db")]
    pub classification_db: String,
}

fn default_cache_dir() -> String {
    "cache".to_string()
}

fn default_service_addr() -> String {
    "127.0.0.1:8433".to_string()
}

fn default_users_file() -> String {
    "users.ids".to_string()
}

fn default_cert() -> String {
    "cert.pem".to_string()
}

fn default_key() -> String {
    "key.pem".to_string()
}

fn default_thumbnail_size() -> u32 {
    200
}

fn default_db_snapshot() -> String {
    "auto".to_string()
}

fn default_classification_db() -> String {
    "biblio_classification_unified.db".to_string()
}

impl Config {
    /// Determine the base directory for path resolution
    fn get_base_dir() -> PathBuf {
        if std::env::var("APP_IN_DOCKER").unwrap_or_default() == "true" {
            PathBuf::from("/config")
        } else {
            std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."))
        }
    }

    /// Resolve a path: if relative, make it relative to base_dir; if absolute, use as-is
    fn resolve_path(base_dir: &PathBuf, path: &str) -> String {
        let p = PathBuf::from(path);
        if p.is_absolute() {
            path.to_string()
        } else {
            base_dir.join(p).to_string_lossy().to_string()
        }
    }

    /// Load configuration from the YAML file
    pub fn load(path: &str) -> Result<Self, Box<dyn std::error::Error>> {
        let contents = fs::read_to_string(path)?;
        let mut config: Config = serde_yaml_ng::from_str(&contents)?;
        
        // Resolve relative paths
        let base_dir = Self::get_base_dir();
        config.library_path = Self::resolve_path(&base_dir, &config.library_path);
        for entry in &mut config.library_paths {
            entry.path = Self::resolve_path(&base_dir, &entry.path);
        }
        config.users_file_path = Self::resolve_path(&base_dir, &config.users_file_path);
        config.certificate_path = Self::resolve_path(&base_dir, &config.certificate_path);
        config.private_key_path = Self::resolve_path(&base_dir, &config.private_key_path);
        config.cache_dir = Self::resolve_path(&base_dir, &config.cache_dir);
        config.classification_db = Self::resolve_path(&base_dir, &config.classification_db);

        Ok(config)
    }
}

/// Global configuration instance (thread-safe)
static CONFIG: std::sync::OnceLock<Arc<Mutex<Config>>> = std::sync::OnceLock::new();
static CONFIG_PATH: std::sync::OnceLock<String> = std::sync::OnceLock::new();

/// Initialize the global configuration from the YAML file
pub fn init(config_path: &str) -> Result<(), Box<dyn std::error::Error>> {
    let config = Config::load(config_path)?;
    let _ = CONFIG_PATH.set(config_path.to_string());
    info!("Configuration loaded from {}", config_path);
    
    // Determine and log which mode we're running in
    if std::env::var("APP_IN_DOCKER").unwrap_or_default() == "true" {
        info!("Running in Docker mode (APP_IN_DOCKER=true)");
    } else {
        info!("Running in standard mode (APP_IN_DOCKER not set or false)");
    }
    
    // Validate configuration paths exist or warn if they don't
    if config.library_paths.is_empty() && !config.library_path.is_empty()
        && !std::path::Path::new(&config.library_path).exists()
    {
        warn!("Library path does not exist: {}", config.library_path);
    }
    for entry in &config.library_paths {
        if !std::path::Path::new(&entry.path).exists() {
            warn!("Library path '{}' does not exist: {}", entry.name, entry.path);
        }
    }


    if config.use_https {
        if !std::path::Path::new(&config.certificate_path).exists() {
            warn!("Certificate file not found: {}", config.certificate_path);
        }
        if !std::path::Path::new(&config.private_key_path).exists() {
            warn!("Private key file not found: {}", config.private_key_path);
        }
    }
    
    CONFIG.get_or_init(|| Arc::new(Mutex::new(config)));
    Ok(())
}

/// Get the current configuration
pub fn get() -> Arc<Mutex<Config>> {
    CONFIG
        .get()
        .cloned()
        .unwrap_or_else(|| {
            error!("Configuration not initialized. Call config::init() during startup.");
            panic!("Configuration not initialized");
        })
}

/// Convenience function to get a config value (immutable access)
pub fn with<F, R>(f: F) -> R
where
    F: FnOnce(&Config) -> R,
{
    let config = get();
    let cfg = config.lock().unwrap();
    f(&cfg)
}

/// Convenience accessors for direct configuration values
#[allow(dead_code)] // kept for external callers / legacy config consumers
pub fn library_path() -> String {
    with(|cfg| cfg.library_path.clone())
}

/// Effective library entries. Falls back to legacy `library_path` when
/// `library_paths` is empty. Names are derived from the folder name in fallback.
pub fn library_entries() -> Vec<LibraryEntry> {
    with(|cfg| {
        if !cfg.library_paths.is_empty() {
            return cfg.library_paths.clone();
        }
        if cfg.library_path.is_empty() {
            return Vec::new();
        }
        let name = std::path::Path::new(&cfg.library_path)
            .file_name()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_else(|| "Library".to_string());
        vec![LibraryEntry { name, path: cfg.library_path.clone() }]
    })
}

/// Re-read config.yaml into the existing global instance so path edits take
/// effect without restarting. Fails without mutating state on read/parse error.
pub fn reload() -> Result<(), Box<dyn std::error::Error>> {
    let path = CONFIG_PATH
        .get()
        .ok_or("Configuration not initialized")?;
    let fresh = Config::load(path)?;
    let cfg = get();
    *cfg.lock().unwrap() = fresh;
    info!("Configuration reloaded from {}", path);
    Ok(())
}

pub fn service_ip_and_port() -> String {
    with(|cfg| cfg.service_ip_and_port.clone())
}

pub fn users_file_path() -> String {
    with(|cfg| cfg.users_file_path.clone())
}

pub fn use_https() -> bool {
    with(|cfg| cfg.use_https)
}

pub fn certificate_path() -> String {
    with(|cfg| cfg.certificate_path.clone())
}

pub fn private_key_path() -> String {
    with(|cfg| cfg.private_key_path.clone())
}

pub fn cache_dir() -> String {
    with(|cfg| cfg.cache_dir.clone())
}

pub fn thumbnail_size() -> u32 {
    with(|cfg| cfg.thumbnail_size)
}

pub fn db_snapshot() -> String {
    with(|cfg| cfg.db_snapshot.clone())
}

pub fn classification_db() -> String {
    with(|cfg| cfg.classification_db.clone())
}

#[cfg(test)]
mod tests {

    #[test]
    fn test_config_load() {
        // This test would require a test config.yaml file
        // For now, we just verify the functions exist and compile
    }
}
