use std::path::{Path, PathBuf};
use std::sync::Arc;
use serde::{Deserialize, Serialize};
use uuid::Uuid;
use crate::cache::{should_snapshot, CacheStore};
use crate::db::CalibreDb;
use std::collections::HashMap;
use std::sync::Mutex;
use std::time::Instant;
use tracing::{info, warn, error};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LibraryMetadata {
    pub id: String,
    pub name: String,
    /// Library folder, used to locate book files and covers
    pub path: PathBuf,
    /// Database actually queried (local snapshot when the library is remote)
    pub metadata_db_path: PathBuf,
    pub book_count: usize,
    /// Original `metadata.db` inside the library
    #[serde(skip)]
    pub source_db_path: PathBuf,
    #[serde(skip)]
    pub db_is_snapshot: bool,
}

pub struct LibraryScanner {
    base_path: PathBuf,
    store: Arc<CacheStore>,
    snapshot_mode: String,
    force_snapshot: bool,
}

impl LibraryScanner {
    pub fn new<P: AsRef<Path>>(
        base_path: P,
        store: Arc<CacheStore>,
        snapshot_mode: String,
        force_snapshot: bool,
    ) -> Self {
        LibraryScanner {
            base_path: base_path.as_ref().to_path_buf(),
            store,
            snapshot_mode,
            force_snapshot,
        }
    }

    /// Locate Calibre libraries (directories holding a `metadata.db`).
    ///
    /// Accepted forms of the configured path, so the library can live anywhere:
    ///   * the library folder itself (…/book with metadata.db inside)
    ///   * a folder containing several libraries (…/libraries/book/metadata.db)
    ///   * a direct path to a metadata.db file
    ///
    /// Only the path itself and its direct children are inspected: a full
    /// recursive walk would descend into every author/book folder of a library,
    /// which is extremely expensive on network storage.
    pub fn scan(&self) -> std::io::Result<Vec<LibraryMetadata>> {
        let mut libraries = Vec::new();
        let mut candidates: Vec<PathBuf> = Vec::new();
        let base = self.normalized_base();

        if base.join("metadata.db").exists() {
            candidates.push(base.clone());
        } else if let Ok(entries) = std::fs::read_dir(&base) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_dir() && path.join("metadata.db").exists() {
                    candidates.push(path);
                }
            }
        }

        for path in candidates {
            match self.create_library_metadata(&path) {
                Ok(lib_metadata) => libraries.push(lib_metadata),
                Err(e) => warn!("Failed to create library metadata for {:?}: {}", path, e),
            }
        }

        // Sort by name
        libraries.sort_by(|a, b| a.name.cmp(&b.name));

        Ok(libraries)
    }

    /// Accept both a folder and a direct `metadata.db` path.
    fn normalized_base(&self) -> PathBuf {
        if self.base_path.is_file() {
            if let Some(parent) = self.base_path.parent() {
                return parent.to_path_buf();
            }
        }
        self.base_path.clone()
    }

    fn create_library_metadata(&self, path: &Path) -> std::io::Result<LibraryMetadata> {
        let library_name = path
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .to_string();

        // Create a stable UUID from the library path using UUID v5 (SHA-1 namespace hash)
        // This ensures the same path always produces the same ID
        let path_str = path.to_string_lossy();
        let id = Uuid::new_v5(&Uuid::NAMESPACE_DNS, path_str.as_bytes()).to_string();

        let source_db_path = path.join("metadata.db");

        let mut metadata_db_path = source_db_path.clone();
        let mut db_is_snapshot = false;
        if should_snapshot(&self.snapshot_mode, &source_db_path) {
            match self.store.ensure_snapshot(&source_db_path, &id, self.force_snapshot) {
                Ok(snapshot) => {
                    metadata_db_path = snapshot;
                    db_is_snapshot = true;
                }
                Err(e) => warn!(
                    "Could not snapshot {:?} ({}); querying the original database over the network",
                    source_db_path, e
                ),
            }
        }

        let book_count = self.get_book_count(&metadata_db_path).unwrap_or(0);

        Ok(LibraryMetadata {
            id,
            name: library_name,
            path: path.to_path_buf(),
            metadata_db_path,
            book_count,
            source_db_path,
            db_is_snapshot,
        })
    }

    fn get_book_count(&self, metadata_db_path: &Path) -> Result<usize, Box<dyn std::error::Error>> {
        match CalibreDb::open(metadata_db_path) {
            Ok(db) => match db.count_books() {
                Ok(count) => Ok(count),
                Err(e) => {
                    error!("Failed to count books at {:?}: {}", metadata_db_path, e);
                    Err(Box::new(e))
                }
            },
            Err(e) => {
                error!("Failed to open database at {:?}: {}", metadata_db_path, e);
                Err(Box::new(e))
            }
        }
    }
}

// In-memory cache of loaded libraries
pub struct LibraryCache {
    libraries: HashMap<String, LibraryMetadata>,
    // One lock per library: the SQLite connection is `Send` but not `Sync`, and
    // a global lock would stall every request behind a single long query.
    databases: HashMap<String, Arc<Mutex<CalibreDb>>>,
}

impl LibraryCache {
    pub fn new() -> Self {
        LibraryCache {
            libraries: HashMap::new(),
            databases: HashMap::new(),
        }
    }

    pub fn clear(&mut self) {
        self.libraries.clear();
        self.databases.clear();
    }

    /// Load (or reload) libraries. `force_snapshot` refreshes local database
    /// snapshots even when the source looks unchanged.
    #[allow(dead_code)] // legacy single-path entry point
    pub fn load_libraries(
        &mut self,
        base_path: &Path,
        store: Arc<CacheStore>,
        snapshot_mode: String,
        force_snapshot: bool,
    ) -> std::io::Result<()> {
        let scanner = LibraryScanner::new(base_path, store, snapshot_mode, force_snapshot);
        let libraries = scanner.scan()?;
        self.register_libraries(libraries)
    }

    /// Load libraries from an explicit list of (name, path) entries.
    /// Each entry may be a library folder, a parent folder of libraries, or a
    /// direct path to metadata.db. Duplicate libraries (same metadata.db path)
    /// are loaded once; the first entry wins.
    pub fn load_named_libraries(
        &mut self,
        entries: &[(String, String)],
        store: Arc<CacheStore>,
        snapshot_mode: String,
        force_snapshot: bool,
    ) -> std::io::Result<()> {
        let mut libraries: Vec<LibraryMetadata> = Vec::new();
        let mut seen = std::collections::HashSet::new();

        for (name, path) in entries {
            let scanner = LibraryScanner::new(path, store.clone(), snapshot_mode.clone(), force_snapshot);
            match scanner.scan() {
                Ok(mut found) => {
                    for lib in found.drain(..) {
                        let key = lib.metadata_db_path.clone();
                        if !seen.insert(key) {
                            continue;
                        }
                        let mut lib = lib;
                        lib.name = name.clone();
                        libraries.push(lib);
                    }
                }
                Err(e) => error!("Failed to scan library '{}' at {:?}: {}", name, path, e),
            }
        }
        libraries.sort_by(|a, b| a.name.cmp(&b.name));
        self.register_libraries(libraries)
    }

    fn register_libraries(&mut self, libraries: Vec<LibraryMetadata>) -> std::io::Result<()> {

        for mut lib in libraries {
            match CalibreDb::open_with_library(&lib.metadata_db_path, Some(&lib.path)) {
                Ok(db) => {
                    // Warm the in-memory caches now so page requests are instant.
                    let started = Instant::now();
                    match db.warm_cache() {
                        Ok(_) => {
                            if let Ok(count) = db.count_books() {
                                lib.book_count = count;
                            }
                        }
                        Err(e) => error!(
                            "Failed to warm cache for library '{}' at {:?}: {}",
                            lib.name, lib.metadata_db_path, e
                        ),
                    }
                    info!(
                        "Loaded library '{}' ({:?}{}) with {} books in {:.1}s",
                        lib.name,
                        lib.metadata_db_path,
                        if lib.db_is_snapshot { ", local snapshot" } else { "" },
                        lib.book_count,
                        started.elapsed().as_secs_f64()
                    );
                    self.databases.insert(lib.id.clone(), Arc::new(Mutex::new(db)));
                    self.libraries.insert(lib.id.clone(), lib);
                }
                Err(e) => {
                    error!("Failed to open database for library '{}' at {:?}: {}", lib.name, lib.metadata_db_path, e);
                }
            }
        }

        Ok(())
    }

    pub fn get_libraries(&self) -> Vec<LibraryMetadata> {
        let mut libs: Vec<_> = self.libraries.values().cloned().collect();
        libs.sort_by(|a, b| a.name.cmp(&b.name));
        libs
    }

    pub fn get_library(&self, id: &str) -> Option<&LibraryMetadata> {
        self.libraries.get(id)
    }

    pub fn get_database(&self, id: &str) -> Option<Arc<Mutex<CalibreDb>>> {
        self.databases.get(id).cloned()
    }

    /// Every library handle, for background warm-up work.
    pub fn all_databases(&self) -> Vec<Arc<Mutex<CalibreDb>>> {
        self.databases.values().cloned().collect()
    }
}
