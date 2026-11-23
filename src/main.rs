mod db;
mod library;
mod api;
mod config;
mod auth;
mod session;
mod audit;
mod rbac;
mod cache;
mod classification;

use actix_web::{web, App, HttpServer, middleware};
use actix_files::Files;
use library::LibraryCache;
use cache::CacheStore;
use std::sync::{Arc, RwLock};
use tracing::{warn, info, error};
use rustls::{ServerConfig, pki_types::CertificateDer};
use rustls_pemfile::certs;
use std::fs::File;
use std::io::BufReader;

#[actix_web::main]
async fn main() -> std::io::Result<()> {
    // Initialize tracing
    tracing_subscriber::fmt::init();

    // Determine configuration file path based on deployment mode
    let config_path = if std::env::var("APP_IN_DOCKER").unwrap_or_default() == "true" {
        "/config/config.yaml"
    } else {
        "config.yaml"
    };

    // Initialize configuration from YAML file
    if let Err(e) = config::init(config_path) {
        error!("Failed to initialize configuration: {}", e);
        if std::env::var("APP_IN_DOCKER").unwrap_or_default() == "true" {
            error!("Ensure config.yaml exists at /config/config.yaml (mounted volume).");
        } else {
            error!("Ensure config.yaml exists in the working directory. Copy config.yaml.example to get started.");
        }
        return Err(std::io::Error::new(
            std::io::ErrorKind::NotFound,
            format!("Configuration initialization failed: {}", e)
        ));
    }

    // Shared, library-independent category table (read only when a book has no
    // local category row, e.g. after being copied into another library).
    classification::init(std::path::PathBuf::from(config::classification_db()));

    // Load users for authentication
    let users_path = config::users_file_path();
    let users = match auth::load_users(&users_path) {
        Ok(users) => {
            info!("Loaded {} user(s) for authentication", users.len());
            web::Data::new(users)
        }
        Err(e) => {
            error!("Failed to load users: {}", e);
            error!("Authentication will be disabled. Configure users_file_path in config.yaml");
            web::Data::new(vec![])
        }
    };

    // Initialize session store (30 minute timeout)
    let session_store = web::Data::new(session::SessionStore::new(30));

    // Initialize audit logger (keep last 1000 events)
    let audit_logger = web::Data::new(audit::AuditLogger::new(1000));

    // Set up library cache and the local cache (DB snapshots + thumbnails)
    let mut cache = LibraryCache::new();

    let cache_root = config::cache_dir();
    let store = Arc::new(CacheStore::new(&cache_root)?);
    info!("Local cache directory: {:?}", store.root());

    // Load libraries from the configured paths
    let entries: Vec<(String, String)> = config::library_entries()
        .into_iter()
        .map(|e| (e.name, e.path))
        .collect();
    let snapshot_mode = config::db_snapshot();
    if entries.is_empty() {
        warn!("No libraries configured; set library_paths in config.yaml");
    } else {
        cache.load_named_libraries(&entries, store.clone(), snapshot_mode, false)?;
    }

    // Warm the per-library search index and sort orders in the background, so
    // the first filter/sort click costs nothing instead of 0.3-0.5 s.
    {
        let dbs = cache.all_databases();
        std::thread::spawn(move || {
            for db in dbs {
                if let Ok(guard) = db.lock() {
                    let _ = guard.book_index();
                }
                for sort in ["title", "series", "author"] {
                    if let Ok(guard) = db.lock() {
                        let _ = guard.sort_set(sort);
                    }
                }
            }
            info!("Background index warm-up finished");
        });
    }

    let cache = web::Data::new(RwLock::new(cache));
    let store_data = web::Data::new(store.clone());

    // Get service binding address
    let service_ip_and_port = config::service_ip_and_port();

    // Determine protocol and log startup info
    let protocol = if config::use_https() { "https" } else { "http" };
    info!("Starting Biblio server on {}://{}", protocol, service_ip_and_port);

    let server_builder = HttpServer::new(move || {
        App::new()
            .app_data(cache.clone())
            .app_data(store_data.clone())
            .app_data(users.clone())
            .app_data(session_store.clone())
            .app_data(audit_logger.clone())
            .wrap(middleware::Logger::default())
            .configure(api::configure)
            .service(Files::new("/", "./public").index_file("index.html"))
    });

    if config::use_https() {
        info!("Setting up HTTPS with TLS");
        match load_tls_config() {
            Ok(tls_config) => {
                info!("TLS configuration loaded successfully");
                server_builder
                    .bind_rustls_0_23(&service_ip_and_port, tls_config)?
                    .run()
                    .await
            }
            Err(e) => {
                error!("Failed to load TLS configuration: {}", e);
                error!("Falling back to HTTP");
                server_builder.bind(&service_ip_and_port)?.run().await
            }
        }
    } else {
        server_builder.bind(&service_ip_and_port)?.run().await
    }
}

// Helper function to load TLS configuration from certificate and key files
fn load_tls_config() -> std::io::Result<ServerConfig> {
    let cert_path = config::certificate_path();
    let key_path = config::private_key_path();
    
    // Load certificates
    let cert_file = File::open(&cert_path)
        .map_err(|e| std::io::Error::new(
            std::io::ErrorKind::NotFound,
            format!("Failed to open certificate file at {}: {}", cert_path, e)
        ))?;
    let mut cert_reader = BufReader::new(cert_file);
    
    let certs_vec: Vec<CertificateDer> = certs(&mut cert_reader)
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!("Failed to parse certificate file: {}", e)
        ))?;
    
    if certs_vec.is_empty() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "No certificates found in certificate file"
        ));
    }

    // Load private key
    let key_file = File::open(&key_path)
        .map_err(|e| std::io::Error::new(
            std::io::ErrorKind::NotFound,
            format!("Failed to open private key file at {}: {}", key_path, e)
        ))?;
    let mut key_reader = BufReader::new(key_file);
    
    // Try to read private key - rustls-pemfile 2.x provides specific readers
    let key_bytes = std::io::read_to_string(&mut key_reader)
        .map_err(|e| std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!("Failed to read private key file: {}", e)
        ))?;
    
    let private_key = rustls_pemfile::private_key(&mut std::io::Cursor::new(key_bytes))
        .map_err(|e| std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!("Failed to parse private key file: {}", e)
        ))?
        .ok_or_else(|| std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "No private key found in key file"
        ))?;

    // Build and return the TLS configuration
    ServerConfig::builder()
        .with_no_client_auth()
        .with_single_cert(certs_vec, private_key)
        .map_err(|e| std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!("Failed to build TLS configuration: {}", e)
        ))
}