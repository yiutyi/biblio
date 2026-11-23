//! Book categories.
//!
//! Each Calibre library may carry its own `biblio_classification.db` (one row
//! per `book_id`). Because a book id is only meaningful inside one library, a
//! second, library-independent table is used as a fallback: a normalized
//! `title + author` key. That fallback is what keeps a book classified after it
//! is copied into another library (where it gets a new id).
//!
//! Both databases are opened read-only. Calibre's own `metadata.db` is never
//! written to.

use rusqlite::{Connection, OpenFlags};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};
use tracing::{info, warn};

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Category {
    pub primary: String,
    pub second: Option<String>,
    pub confidence: Option<f64>,
}

/// Categories keyed by Calibre book id, from a library-local classification db.
pub type LocalCategories = HashMap<i32, Category>;

/// Normalization must stay in sync with `tools/build_unified.py`.
fn drop_parenthesised(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    let mut depth: usize = 0;
    for ch in value.chars() {
        match ch {
            '（' | '(' => depth += 1,
            '）' | ')' => {
                if depth > 0 {
                    depth -= 1;
                } else {
                    out.push(ch);
                }
            }
            _ => {
                if depth == 0 {
                    out.push(ch);
                }
            }
        }
    }
    out
}

pub fn normalize_title(title: &str) -> String {
    let mut value = drop_parenthesised(title);
    for marker in ["《", "》", "〈", "〉", "「", "」", "[", "]", "【", "】"] {
        value = value.replace(marker, "");
    }
    value
        .chars()
        .filter(|c| {
            !c.is_whitespace()
                && !matches!(
                    c,
                    '：' | ':'
                        | '—'
                        | '–'
                        | '-'
                        | '·'
                        | '、'
                        | ','
                        | '，'
                        | '.'
                        | '。'
                        | '!'
                        | '！'
                        | '?'
                        | '？'
                        | '\''
                        | '"'
                        | '“'
                        | '”'
                        | '‘'
                        | '’'
                        | '/'
                        | '\\'
                        | '|'
                        | '~'
                        | '+'
                        | '='
                        | '*'
                        | '#'
                        | '@'
                        | '$'
                        | '%'
                        | '&'
                        | ';'
                        | '；'
                )
        })
        .flat_map(|c| c.to_lowercase())
        .collect()
}

const AUTHOR_ROLES: [&str; 17] = [
    "等著", "等编", "等译", "编著", "主编", "编", "译", "辑录", "整理", "撰", "校注", "注",
    "选编", "编译", "作", "绘", "原著",
];

pub fn normalize_author(authors: &[String]) -> String {
    let mut value = authors.join("; ");
    for marker in ["[", "]", "【", "】", "(", ")", "（", "）"] {
        value = value.replace(marker, "");
    }
    value = drop_parenthesised(&value);
    for role in AUTHOR_ROLES {
        value = value.replace(role, "");
    }
    let cleaned: String = value
        .chars()
        .filter(|c| {
            !c.is_whitespace()
                && !matches!(c, '·' | '•' | ',' | '，' | '、' | ';' | '；' | ':' | '：')
        })
        .flat_map(|c| c.to_lowercase())
        .collect();
    cleaned.chars().take(12).collect()
}

pub fn pair_key(title: &str, authors: &[String]) -> Option<(String, String)> {
    let title_key = normalize_title(title);
    if title_key.chars().count() < 2 {
        return None;
    }
    let author_key = normalize_author(authors);
    Some((title_key, author_key))
}

/// Library-independent table loaded from `biblio_classification_unified.db`.
struct UnifiedTable {
    pairs: HashMap<String, Category>,
    titles: HashMap<String, Category>,
}

static UNIFIED_PATH: OnceLock<PathBuf> = OnceLock::new();
static UNIFIED: OnceLock<Option<Arc<UnifiedTable>>> = OnceLock::new();

/// Point the fallback lookup at the shared database. Called once at start-up;
/// the file itself is only read when a book is missing from its library-local
/// table, so a fully classified library never pays for it.
pub fn init(path: PathBuf) {
    let _ = UNIFIED_PATH.set(path);
}

fn unified() -> &'static Option<Arc<UnifiedTable>> {
    UNIFIED.get_or_init(|| {
        let path = UNIFIED_PATH.get()?;
        if !path.exists() {
            warn!("Unified classification database not found at {:?}", path);
            return None;
        }
        match load_unified(path) {
            Ok(table) => {
                info!(
                    "Loaded unified classification: {} title+author keys, {} title keys",
                    table.pairs.len(),
                    table.titles.len()
                );
                Some(Arc::new(table))
            }
            Err(e) => {
                warn!("Could not read unified classification {:?}: {}", path, e);
                None
            }
        }
    })
}

fn load_unified(path: &Path) -> rusqlite::Result<UnifiedTable> {
    let conn = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
    let mut pairs = HashMap::new();
    {
        let mut stmt = conn.prepare(
            "SELECT title_key, author_key, primary_class, second_class, confidence FROM book_class",
        )?;
        let rows = stmt.query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, Option<String>>(3)?,
                row.get::<_, Option<f64>>(4)?,
            ))
        })?;
        for row in rows {
            let (title_key, author_key, primary, second, confidence) = row?;
            pairs.insert(
                format!("{title_key}\u{1f}{author_key}"),
                Category { primary, second, confidence },
            );
        }
    }

    let mut titles = HashMap::new();
    {
        let mut stmt =
            conn.prepare("SELECT title_key, primary_class FROM title_class")?;
        let rows = stmt.query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })?;
        for row in rows {
            let (title_key, primary) = row?;
            titles.insert(
                title_key,
                Category { primary, second: None, confidence: None },
            );
        }
    }

    Ok(UnifiedTable { pairs, titles })
}

/// Fallback lookup for a book whose own library has no row for it.
pub fn lookup(title: &str, authors: &[String]) -> Option<Category> {
    let table = unified().as_ref()?;
    let (title_key, author_key) = pair_key(title, authors)?;
    if let Some(hit) = table.pairs.get(&format!("{title_key}\u{1f}{author_key}")) {
        return Some(hit.clone());
    }
    table.titles.get(&title_key).cloned()
}

/// Read `<library>/biblio_classification.db`. A missing or unreadable file is
/// not an error: the library simply has no categories of its own.
pub fn load_local(path: &Path) -> Option<LocalCategories> {
    if !path.exists() {
        return None;
    }
    match read_local(path) {
        Ok(map) => {
            info!(
                "Loaded {} book categories from {:?}",
                map.len(),
                path
            );
            Some(map)
        }
        Err(e) => {
            warn!("Could not read classification database {:?}: {}", path, e);
            None
        }
    }
}

fn read_local(path: &Path) -> rusqlite::Result<LocalCategories> {
    let conn = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
    let mut stmt = conn.prepare(
        "SELECT book_id, primary_class, second_class, confidence FROM book_category",
    )?;
    let rows = stmt.query_map([], |row| {
        Ok((
            row.get::<_, i64>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, Option<String>>(2)?,
            row.get::<_, Option<f64>>(3)?,
        ))
    })?;
    let mut map = HashMap::new();
    for row in rows {
        let (book_id, primary, second, confidence) = row?;
        if primary.is_empty() {
            continue;
        }
        map.insert(
            book_id as i32,
            Category { primary, second, confidence },
        );
    }
    Ok(map)
}
