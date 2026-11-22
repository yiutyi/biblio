//! Local cache helpers: Calibre database snapshots and cover thumbnails.
//!
//! A Calibre library on a network share (SMB/NFS) makes every SQLite page read
//! and every cover fetch a network round trip. Copying the database once to a
//! local disk, and caching resized covers, turns those repeated network reads
//! into a one-off cost.

use std::collections::HashSet;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::RwLock;
use std::time::{Instant, SystemTime};
use tracing::info;

pub struct CacheStore {
    root: PathBuf,
    /// Thumbnails already produced (or verified) in this process. Checking them
    /// again means one `stat` on the network share per cover request, which costs
    /// more than serving the cached file.
    thumbs_ready: RwLock<HashSet<String>>,
}

impl CacheStore {
    pub fn new<P: AsRef<Path>>(root: P) -> io::Result<Self> {
        let root = root.as_ref().to_path_buf();
        fs::create_dir_all(root.join("db"))?;
        fs::create_dir_all(root.join("thumbs"))?;
        Ok(CacheStore {
            root,
            thumbs_ready: RwLock::new(HashSet::new()),
        })
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn db_dir(&self, library_id: &str) -> PathBuf {
        self.root.join("db").join(sanitize(library_id))
    }

    pub fn snapshot_path(&self, library_id: &str) -> PathBuf {
        self.db_dir(library_id).join("metadata.db")
    }

    pub fn thumb_dir(&self, library_id: &str) -> PathBuf {
        self.root.join("thumbs").join(sanitize(library_id))
    }

    pub fn thumb_path(&self, library_id: &str, book_id: i32) -> PathBuf {
        self.thumb_dir(library_id).join(format!("{}.jpg", book_id))
    }

    /// Produce (or reuse) a local snapshot of the Calibre `metadata.db`.
    ///
    /// `VACUUM INTO` reads through a consistent transaction, so the copy is
    /// valid even while Calibre is running. The snapshot is refreshed when the
    /// source file's size or modification time changes, or when `force` is set.
    pub fn ensure_snapshot(
        &self,
        source_db: &Path,
        library_id: &str,
        force: bool,
    ) -> io::Result<PathBuf> {
        let dest = self.snapshot_path(library_id);
        let stamp = dest.with_extension("stamp");

        let fingerprint = source_fingerprint(source_db)?;
        if !force && dest.exists() {
            if let Ok(previous) = fs::read_to_string(&stamp) {
                if previous.trim() == fingerprint {
                    return Ok(dest);
                }
            }
        }

        fs::create_dir_all(self.db_dir(library_id))?;

        let started = Instant::now();
        let temp = dest.with_extension("tmp");
        let _ = fs::remove_file(&temp);
        let _ = fs::remove_file(&dest);

        let conn = rusqlite::Connection::open_with_flags(
            source_db,
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
        )
        .map_err(|e| io::Error::new(io::ErrorKind::Other, e))?;

        let escaped = temp.to_string_lossy().replace('\'', "''");
        conn.execute_batch(&format!("VACUUM INTO '{}'", escaped))
            .map_err(|e| io::Error::new(io::ErrorKind::Other, e))?;
        drop(conn);

        fs::rename(&temp, &dest)?;
        let _ = fs::write(&stamp, &fingerprint);

        info!(
            "Snapshot of Calibre database ({}) written to {:?} in {:.1}s",
            human_size(fs::metadata(&dest).map(|m| m.len()).unwrap_or(0)),
            dest,
            started.elapsed().as_secs_f64()
        );

        Ok(dest)
    }

    /// Ensure a JPEG thumbnail exists for `source_cover`, and return its path.
    ///
    /// Deliberately does NOT re-stat the source cover: on an SMB share that call
    /// alone costs 50 ms to over a second, per image, which dwarfs decoding.
    /// The cached thumbnail is treated as immutable; delete `cache/thumbs` (or
    /// the whole `cache` folder) to force regeneration.
    pub fn ensure_thumbnail(
        &self,
        source_cover: &Path,
        library_id: &str,
        book_id: i32,
        max_side: u32,
    ) -> io::Result<PathBuf> {
        let dest = self.thumb_path(library_id, book_id);
        let key = format!("{}/{}", library_id, book_id);

        if self.thumbs_ready.read().unwrap().contains(&key) {
            return Ok(dest);
        }

        // Local `stat` only: cheap, and enough to know a usable file is there.
        if fs::metadata(&dest).map(|m| m.len() > 0).unwrap_or(false) {
            self.thumbs_ready.write().unwrap().insert(key);
            return Ok(dest);
        }

        fs::create_dir_all(self.thumb_dir(library_id))?;

        let bytes = fs::read(source_cover)?;
        let thumb = render_thumbnail(&bytes, max_side)?;

        let temp = dest.with_extension("tmp");
        fs::write(&temp, &thumb)?;
        let _ = fs::remove_file(&dest);
        fs::rename(&temp, &dest)?;
        self.thumbs_ready.write().unwrap().insert(key);
        Ok(dest)
    }
}

fn render_thumbnail(bytes: &[u8], max_side: u32) -> io::Result<Vec<u8>> {
    use image::ImageEncoder;

    let img = image::load_from_memory(bytes)
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;

    let (w, h) = (img.width(), img.height());
    let scaled = if w.max(h) <= max_side {
        img
    } else {
        img.thumbnail(max_side, max_side)
    };

    let rgb = scaled.to_rgb8();
    let mut out: Vec<u8> = Vec::with_capacity((rgb.width() * rgb.height() / 8) as usize);
    image::codecs::jpeg::JpegEncoder::new_with_quality(&mut out, 82)
        .write_image(
            rgb.as_raw(),
            rgb.width(),
            rgb.height(),
            image::ExtendedColorType::Rgb8,
        )
        .map_err(|e| io::Error::new(io::ErrorKind::Other, e))?;

    Ok(out)
}

fn source_fingerprint(path: &Path) -> io::Result<String> {
    let meta = fs::metadata(path)?;
    let modified = meta
        .modified()
        .unwrap_or(SystemTime::UNIX_EPOCH)
        .duration_since(SystemTime::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    Ok(format!("{}:{}", meta.len(), modified))
}

fn sanitize(value: &str) -> String {
    value
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() || c == '-' || c == '_' { c } else { '_' })
        .collect()
}

fn human_size(bytes: u64) -> String {
    let mb = bytes as f64 / (1024.0 * 1024.0);
    format!("{:.0} MB", mb)
}

/// Decide whether a library database should be snapshotted locally.
pub fn should_snapshot(mode: &str, source_db: &Path) -> bool {
    match mode {
        "always" => true,
        "never" => false,
        _ => is_remote_path(source_db),
    }
}

/// Best-effort detection of a network-backed path. Local disks (including a
/// second physical drive) return false; SMB/NFS mounts return true.
#[cfg(windows)]
pub fn is_remote_path(path: &Path) -> bool {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Storage::FileSystem::GetDriveTypeW;

    // DRIVE_REMOTE
    const DRIVE_REMOTE: u32 = 4;

    let text = path.to_string_lossy();
    if text.starts_with("\\\\") || text.starts_with("//") {
        return true;
    }

    let root: String = text.chars().take(2).collect();
    if root.is_empty() || !root.ends_with(':') {
        return false;
    }

    let wide: Vec<u16> = std::ffi::OsStr::new(&root)
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();
    let drive_type = unsafe { GetDriveTypeW(wide.as_ptr()) };
    drive_type == DRIVE_REMOTE
}

#[cfg(unix)]
pub fn is_remote_path(path: &Path) -> bool {
    use std::os::unix::ffi::OsStrExt;

    const REMOTE_TYPES: [&str; 8] = [
        "nfs", "nfs4", "cifs", "smb3", "smbfs", "fuse.sshfs", "9p", "davfs",
    ];

    let canonical = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
    let text = canonical.as_os_str().as_bytes();
    let text = String::from_utf8_lossy(text).to_string();

    if let Ok(mounts) = fs::read_to_string("/proc/mounts") {
        let mut best: Option<(usize, &str)> = None;
        for line in mounts.lines() {
            let mut parts = line.split_whitespace();
            let (mount_point, fs_type) = match (parts.next(), parts.next(), parts.next()) {
                (Some(_dev), Some(mp), Some(fst)) => (mp, fst),
                _ => continue,
            };
            if text.starts_with(mount_point) {
                let len = mount_point.len();
                if best.map(|(l, _)| len > l).unwrap_or(true) {
                    best = Some((len, fs_type));
                }
            }
        }
        if let Some((_, fs_type)) = best {
            if REMOTE_TYPES.contains(&fs_type) {
                return true;
            }
        }
    }
    false
}

#[cfg(not(any(windows, unix)))]
pub fn is_remote_path(_path: &Path) -> bool {
    false
}
