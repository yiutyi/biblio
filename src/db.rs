use rusqlite::{Connection, Result as SqlResult, OptionalExtension, OpenFlags};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};
use serde::{Deserialize, Serialize};
use crate::classification;
use tracing::warn;

/// Separator used to join multiple values inside a single SQL column.
const SEP: char = '\u{1f}';

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Book {
    pub id: i32,
    pub title: String,
    pub authors: Vec<String>,
    pub series: Option<String>,
    pub series_index: Option<f64>,
    pub tags: Vec<String>,
    pub comments: Option<String>,
    pub publisher: Option<String>,
    pub pubdate: Option<String>,
    pub rating: Option<i32>,
    pub timestamp: Option<String>,
    pub language: Option<String>,
    pub has_cover: bool,
    pub formats: Vec<String>,
    pub sort: Option<String>,
    /// Category from the library's own `biblio_classification.db`, or (when a
    /// book was copied in and got a new id) from the unified title+author table.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub category: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub category_second: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub category_confidence: Option<f64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Author {
    pub id: i32,
    pub name: String,
    pub sort: Option<String>,
    pub book_count: i32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Tag {
    pub id: i32,
    pub name: String,
    pub book_count: i32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Series {
    pub id: i32,
    pub name: String,
    pub sort: Option<String>,
    pub book_count: i32,
}

pub struct CalibreDb {
    conn: Connection,
    // Whole-library caches, loaded once on first access. A Calibre library on a
    // network share makes the per-row lookup pattern prohibitively slow, so every
    // list endpoint is served from memory after a single aggregate query.
    books: OnceLock<Arc<Vec<Book>>>,
    authors: OnceLock<Arc<Vec<Author>>>,
    tags: OnceLock<Arc<Vec<Tag>>>,
    series: OnceLock<Arc<Vec<Series>>>,
    paths: OnceLock<Arc<HashMap<i32, String>>>,
    // One precomputed ordering per sort key, built on the first request that
    // asks for it. Sorting 123k rows costs 300-500 ms; without this cache every
    // page of the same ordering would pay it again.
    sorts: OnceLock<Mutex<HashMap<String, Arc<SortSet>>>>,
    // Lowercased copies of every searchable/filterable field, built once. A
    // request that filters used to lowercase 123k titles plus their authors on
    // every keystroke; now it is pure string comparison over prebuilt data.
    index: OnceLock<Arc<BookIndex>>,
    // Categories are joined onto Book right after the bulk query, so a filter
    // or a page slice never touches SQLite again.
    local_category_path: Option<PathBuf>,
    categories: OnceLock<Option<Arc<classification::LocalCategories>>>,
    category_stats: OnceLock<Arc<Vec<(String, usize)>>>,
}

fn split_multi(value: Option<String>) -> Vec<String> {
    match value {
        Some(v) => v
            .split(SEP)
            .filter(|s| !s.is_empty())
            .map(|s| s.to_string())
            .collect(),
        None => Vec::new(),
    }
}

impl CalibreDb {
    pub fn open<P: AsRef<Path>>(path: P) -> SqlResult<Self> {
        Self::open_with_library(path, None)
    }

    /// Open a library and remember where its category database lives. Only the
    /// Calibre `metadata.db` is queried; the category file is a sibling that
    /// this application maintains itself.
    pub fn open_with_library<P: AsRef<Path>>(path: P, library_dir: Option<&Path>) -> SqlResult<Self> {
        let conn = Connection::open_with_flags(
            path,
            OpenFlags::SQLITE_OPEN_READ_ONLY,
        )?;
        let local_category_path = library_dir
            .map(|dir| dir.join("biblio_classification.db"))
            .filter(|p| p.exists());
        Ok(CalibreDb {
            conn,
            books: OnceLock::new(),
            authors: OnceLock::new(),
            tags: OnceLock::new(),
            series: OnceLock::new(),
            paths: OnceLock::new(),
            sorts: OnceLock::new(),
            index: OnceLock::new(),
            local_category_path,
            categories: OnceLock::new(),
            category_stats: OnceLock::new(),
        })
    }

    /// Number of books in the library. Uses COUNT(*) so the library list can be
    /// built without materialising every row.
    pub fn count_books(&self) -> SqlResult<usize> {
        if let Some(cache) = self.books.get() {
            return Ok(cache.len());
        }
        let count: i64 = self
            .conn
            .query_row("SELECT COUNT(*) FROM books", [], |row| row.get(0))?;
        Ok(count as usize)
    }

    /// All books in the library, ordered by timestamp (newest first).
    /// Loaded once and kept in memory.
    pub fn get_all_books(&self) -> SqlResult<Arc<Vec<Book>>> {
        if let Some(cache) = self.books.get() {
            return Ok(cache.clone());
        }

        let books = self.query_books()?;
        let cache = Arc::new(books);
        let _ = self.books.set(cache.clone());
        Ok(cache)
    }

    fn query_books(&self) -> SqlResult<Vec<Book>> {
        // One bulk query per relation instead of correlated sub-queries: the old
        // shape ran ~7 sub-queries for every one of the 123k rows (several
        // seconds); a handful of GROUP BY scans plus an in-memory merge is a
        // fraction of that.
        let mut books: Vec<Book> = {
            let mut stmt = self.conn.prepare(
                "SELECT id, title, has_cover, sort, timestamp, pubdate, series_index
                 FROM books
                 ORDER BY timestamp DESC",
            )?;
            stmt.query_map([], |row| {
                Ok(Book {
                    id: row.get(0)?,
                    title: row.get(1)?,
                    has_cover: row.get::<_, Option<i64>>(2)?.unwrap_or(0) != 0,
                    sort: row.get(3).ok().flatten(),
                    timestamp: row.get(4).ok().flatten(),
                    pubdate: row.get(5).ok().flatten(),
                    series_index: row.get(6).ok().flatten(),
                    authors: Vec::new(),
                    tags: Vec::new(),
                    formats: Vec::new(),
                    series: None,
                    comments: None,
                    publisher: None,
                    rating: None,
                    language: None,
                    category: None,
                    category_second: None,
                    category_confidence: None,
                })
            })?
            .collect::<SqlResult<Vec<_>>>()?
        };

        let mut index = HashMap::with_capacity(books.len());
        for (pos, book) in books.iter().enumerate() {
            index.insert(book.id, pos);
        }

        // book id -> concatenated authors
        let mut stmt = self.conn.prepare(
            "SELECT bal.book, group_concat(a.name, char(31))
             FROM books_authors_link bal
             JOIN authors a ON a.id = bal.author
             GROUP BY bal.book",
        )?;
        let rows = stmt.query_map([], |row| {
            Ok((row.get::<_, i32>(0)?, row.get::<_, Option<String>>(1)?))
        })?;
        for row in rows {
            let (book_id, names) = row?;
            if let Some(pos) = index.get(&book_id) {
                books[*pos].authors = split_multi(names);
            }
        }

        // tags
        let mut stmt = self.conn.prepare(
            "SELECT btl.book, group_concat(t.name, char(31))
             FROM books_tags_link btl
             JOIN tags t ON t.id = btl.tag
             GROUP BY btl.book",
        )?;
        let rows = stmt.query_map([], |row| {
            Ok((row.get::<_, i32>(0)?, row.get::<_, Option<String>>(1)?))
        })?;
        for row in rows {
            let (book_id, names) = row?;
            if let Some(pos) = index.get(&book_id) {
                books[*pos].tags = split_multi(names);
            }
        }

        // formats
        let mut stmt = self
            .conn
            .prepare("SELECT book, group_concat(format, char(31)) FROM data GROUP BY book")?;
        let rows = stmt.query_map([], |row| {
            Ok((row.get::<_, i32>(0)?, row.get::<_, Option<String>>(1)?))
        })?;
        for row in rows {
            let (book_id, formats) = row?;
            if let Some(pos) = index.get(&book_id) {
                books[*pos].formats = split_multi(formats);
            }
        }

        // series (one name per book, like the previous LIMIT 1 sub-query)
        let mut stmt = self.conn.prepare(
            "SELECT bsl.book, s.name
             FROM books_series_link bsl
             JOIN series s ON s.id = bsl.series
             GROUP BY bsl.book",
        )?;
        let rows = stmt.query_map([], |row| {
            Ok((row.get::<_, i32>(0)?, row.get::<_, Option<String>>(1)?))
        })?;
        for row in rows {
            let (book_id, name) = row?;
            if let Some(pos) = index.get(&book_id) {
                books[*pos].series = name;
            }
        }

        // comments (first row per book)
        let mut stmt = self
            .conn
            .prepare("SELECT book, text FROM comments GROUP BY book")?;
        let rows = stmt.query_map([], |row| {
            Ok((row.get::<_, i32>(0)?, row.get::<_, Option<String>>(1)?))
        })?;
        for row in rows {
            let (book_id, text) = row?;
            if let Some(pos) = index.get(&book_id) {
                books[*pos].comments = text;
            }
        }

        // publishers
        let mut stmt = self.conn.prepare(
            "SELECT bpl.book, p.name
             FROM books_publishers_link bpl
             JOIN publishers p ON p.id = bpl.publisher
             GROUP BY bpl.book",
        )?;
        let rows = stmt.query_map([], |row| {
            Ok((row.get::<_, i32>(0)?, row.get::<_, Option<String>>(1)?))
        })?;
        for row in rows {
            let (book_id, name) = row?;
            if let Some(pos) = index.get(&book_id) {
                books[*pos].publisher = name;
            }
        }

        // ratings
        let mut stmt = self.conn.prepare(
            "SELECT brl.book, r.rating
             FROM books_ratings_link brl
             JOIN ratings r ON r.id = brl.rating
             GROUP BY brl.book",
        )?;
        let rows = stmt.query_map([], |row| {
            Ok((row.get::<_, i32>(0)?, row.get::<_, Option<i32>>(1)?))
        })?;
        for row in rows {
            let (book_id, rating) = row?;
            if let Some(pos) = index.get(&book_id) {
                books[*pos].rating = rating;
            }
        }

        // languages
        let mut stmt = self.conn.prepare(
            "SELECT bll.book, l.lang_code
             FROM books_languages_link bll
             JOIN languages l ON l.id = bll.lang_code
             GROUP BY bll.book",
        )?;
        let rows = stmt.query_map([], |row| {
            Ok((row.get::<_, i32>(0)?, row.get::<_, Option<String>>(1)?))
        })?;
        for row in rows {
            let (book_id, lang) = row?;
            if let Some(pos) = index.get(&book_id) {
                books[*pos].language = lang;
            }
        }

        self.apply_categories(&mut books);

        Ok(books)
    }

    /// Join the library-local categories and, for books that have no row there
    /// (typically a copy from another library, hence a new id), the shared
    /// title+author fallback.
    fn apply_categories(&self, books: &mut [Book]) {
        let local = self.category_map();
        let mut fallback_hits = 0usize;
        for book in books.iter_mut() {
            if let Some(cat) = local.as_ref().and_then(|m| m.get(&book.id)) {
                book.category = Some(cat.primary.clone());
                book.category_second = cat.second.clone();
                book.category_confidence = cat.confidence;
                continue;
            }
            if let Some(cat) = classification::lookup(&book.title, &book.authors) {
                fallback_hits += 1;
                book.category = Some(cat.primary);
                book.category_second = cat.second;
                book.category_confidence = cat.confidence;
            }
        }
        if fallback_hits > 0 {
            warn!(
                "{} book(s) took their category from the shared title+author table",
                fallback_hits
            );
        }
    }

    fn category_map(&self) -> Option<Arc<classification::LocalCategories>> {
        self.categories
            .get_or_init(|| {
                let path = self.local_category_path.as_deref()?;
                classification::load_local(path).map(Arc::new)
            })
            .clone()
    }

    /// Primary-class counts for the sidebar. Computed once from the in-memory
    /// books; sorted by count so the biggest shelves come first.
    pub fn category_stats(&self) -> SqlResult<Arc<Vec<(String, usize)>>> {
        if let Some(cache) = self.category_stats.get() {
            return Ok(cache.clone());
        }
        let books = self.get_all_books()?;
        let mut counts: HashMap<String, usize> = HashMap::new();
        for book in books.iter() {
            if let Some(category) = &book.category {
                *counts.entry(category.clone()).or_insert(0) += 1;
            }
        }
        let mut rows: Vec<(String, usize)> = counts.into_iter().collect();
        rows.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
        let cache = Arc::new(rows);
        let _ = self.category_stats.set(cache.clone());
        Ok(cache)
    }

    /// Look up a single book from the in-memory cache.
    pub fn get_book(&self, book_id: i32) -> SqlResult<Option<Book>> {
        let books = self.get_all_books()?;
        Ok(books.iter().find(|b| b.id == book_id).cloned())
    }

    /// Relative path of a book directory inside the library
    /// (e.g. "作者/书名 (123)"), taken from the Calibre `books.path` column.
    pub fn get_book_path(&self, book_id: i32) -> Option<String> {
        self.book_paths().ok().and_then(|m| m.get(&book_id).cloned())
    }

    pub fn book_paths(&self) -> SqlResult<Arc<HashMap<i32, String>>> {
        if let Some(cache) = self.paths.get() {
            return Ok(cache.clone());
        }

        let mut stmt = self.conn.prepare("SELECT id, path FROM books")?;
        let rows = stmt
            .query_map([], |row| {
                Ok((
                    row.get::<_, i32>(0)?,
                    row.get::<_, Option<String>>(1)?.unwrap_or_default(),
                ))
            })?
            .collect::<SqlResult<HashMap<i32, String>>>()?;

        let cache = Arc::new(rows);
        let _ = self.paths.set(cache.clone());
        Ok(cache)
    }

    pub fn get_book_authors(&self, book_id: i32) -> SqlResult<Vec<String>> {
        let mut stmt = self.conn.prepare(
            "SELECT a.name FROM authors a 
             INNER JOIN books_authors_link bal ON a.id = bal.author 
             WHERE bal.book = ?"
        )?;
        
        let authors = stmt.query_map([book_id], |row| {
            row.get::<_, String>(0)
        })?.collect::<SqlResult<Vec<_>>>()?;

        Ok(authors)
    }

    pub fn get_book_tags(&self, book_id: i32) -> SqlResult<Vec<String>> {
        let mut stmt = self.conn.prepare(
            "SELECT t.name FROM tags t 
             INNER JOIN books_tags_link btl ON t.id = btl.tag 
             WHERE btl.book = ?"
        )?;
        
        let tags = stmt.query_map([book_id], |row| {
            row.get::<_, String>(0)
        })?.collect::<SqlResult<Vec<_>>>()?;

        Ok(tags)
    }

    pub fn get_book_series(&self, book_id: i32) -> SqlResult<(Option<String>, Option<f64>)> {
        let mut stmt = self.conn.prepare(
            "SELECT s.name, b.series_index FROM series s 
             INNER JOIN books_series_link bsl ON s.id = bsl.series 
             INNER JOIN books b ON bsl.book = b.id
             WHERE bsl.book = ?"
        )?;
        
        let result = stmt.query_row([book_id], |row| {
            let name: String = row.get(0)?;
            let index: f64 = row.get(1)?;
            Ok((Some(name), Some(index)))
        }).optional()?;

        Ok(result.unwrap_or((None, None)))
    }

    pub fn get_all_authors(&self) -> SqlResult<Arc<Vec<Author>>> {
        if let Some(cache) = self.authors.get() {
            return Ok(cache.clone());
        }

        let mut stmt = self.conn.prepare(
            "SELECT a.id, a.name, a.sort, COUNT(ba.book) as book_count 
             FROM authors a 
             LEFT JOIN books_authors_link ba ON a.id = ba.author 
             GROUP BY a.id, a.name, a.sort 
             ORDER BY a.sort"
        )?;
        
        let authors = stmt.query_map([], |row| {
            Ok(Author {
                id: row.get(0)?,
                name: row.get(1)?,
                sort: row.get(2).ok(),
                book_count: row.get(3)?,
            })
        })?
        .collect::<SqlResult<Vec<_>>>()?;

        let cache = Arc::new(authors);
        let _ = self.authors.set(cache.clone());
        Ok(cache)
    }

    pub fn get_all_tags(&self) -> SqlResult<Arc<Vec<Tag>>> {
        if let Some(cache) = self.tags.get() {
            return Ok(cache.clone());
        }

        let mut stmt = self.conn.prepare(
            "SELECT t.id, t.name, COUNT(btl.book) as book_count 
             FROM tags t 
             LEFT JOIN books_tags_link btl ON t.id = btl.tag 
             GROUP BY t.id, t.name 
             ORDER BY t.name"
        )?;
        
        let tags = stmt.query_map([], |row| {
            Ok(Tag {
                id: row.get(0)?,
                name: row.get(1)?,
                book_count: row.get(2)?,
            })
        })?
        .collect::<SqlResult<Vec<_>>>()?;

        let cache = Arc::new(tags);
        let _ = self.tags.set(cache.clone());
        Ok(cache)
    }

    pub fn get_all_series(&self) -> SqlResult<Arc<Vec<Series>>> {
        if let Some(cache) = self.series.get() {
            return Ok(cache.clone());
        }

        let mut stmt = self.conn.prepare(
            "SELECT s.id, s.name, s.sort, COUNT(bs.book) as book_count 
             FROM series s 
             LEFT JOIN books_series_link bs ON s.id = bs.series 
             GROUP BY s.id, s.name, s.sort
             ORDER BY s.sort"
        )?;
        
        let series = stmt.query_map([], |row| {
            Ok(Series {
                id: row.get(0)?,
                name: row.get(1)?,
                sort: row.get(2).ok(),
                book_count: row.get(3)?,
            })
        })?
        .collect::<SqlResult<Vec<_>>>()?;

        let cache = Arc::new(series);
        let _ = self.series.set(cache.clone());
        Ok(cache)
    }

    /// How many books carry each format, for the format filter sidebar
    /// (the browser no longer receives the whole library, so it cannot count).
    pub fn format_stats(&self) -> SqlResult<Vec<(String, usize)>> {
        let mut stmt = self
            .conn
            .prepare("SELECT format, COUNT(*) FROM data GROUP BY format ORDER BY format")?;
        let rows = stmt
            .query_map([], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)? as usize))
            })?
            .collect::<SqlResult<Vec<_>>>()?;
        Ok(rows)
    }

    pub fn get_book_formats(&self, book_id: i32) -> SqlResult<Vec<String>> {        if let Ok(books) = self.get_all_books() {
            if let Some(book) = books.iter().find(|b| b.id == book_id) {
                return Ok(book.formats.clone());
            }
            return Ok(Vec::new());
        }

        let mut stmt = self.conn.prepare(
            "SELECT format FROM data WHERE book = ? ORDER BY format"
        )?;
        
        let formats = stmt.query_map([book_id], |row| {
            row.get::<_, String>(0)
        })?.collect::<SqlResult<Vec<_>>>()?;

        Ok(formats)
    }

    pub fn get_book_comments(&self, book_id: i32) -> SqlResult<Option<String>> {
        let mut stmt = self.conn.prepare(
            "SELECT text FROM comments WHERE book = ?"
        )?;
        
        let comments = stmt.query_row([book_id], |row| {
            row.get::<_, String>(0)
        }).optional()?;

        Ok(comments)
    }

    /// Build (once) the lowercased filter/search index.
    pub fn book_index(&self) -> SqlResult<Arc<BookIndex>> {
        if let Some(hit) = self.index.get() {
            return Ok(hit.clone());
        }

        let books = self.get_all_books()?;
        let n = books.len();
        let mut search: Vec<String> = Vec::with_capacity(n);
        let mut authors: Vec<Vec<String>> = Vec::with_capacity(n);
        let mut tags: Vec<Vec<String>> = Vec::with_capacity(n);
        let mut series: Vec<Option<String>> = Vec::with_capacity(n);
        let mut formats: Vec<Vec<String>> = Vec::with_capacity(n);

        for b in books.iter() {
            let authors_lower: Vec<String> = b.authors.iter().map(|a| a.to_lowercase()).collect();
            let series_lower = b.series.as_ref().map(|s| s.to_lowercase());

            let mut joined = String::with_capacity(b.title.len() + 48);
            joined.push_str(&b.title.to_lowercase());
            joined.push('\u{1}');
            for a in &authors_lower {
                joined.push_str(a);
                joined.push('\u{1}');
            }
            if let Some(s) = &series_lower {
                joined.push_str(s);
            }

            search.push(joined);
            authors.push(authors_lower);
            tags.push(b.tags.iter().map(|t| t.to_lowercase()).collect());
            series.push(series_lower);
            formats.push(b.formats.iter().map(|f| f.to_lowercase()).collect());
        }

        let idx = Arc::new(BookIndex { search, authors, tags, series, formats });
        if self.index.set(idx.clone()).is_err() {
            if let Some(existing) = self.index.get() {
                return Ok(existing.clone());
            }
            return Err(rusqlite::Error::InvalidQuery);
        }
        Ok(idx)
    }

    /// Precomputed ordering for one sort key. `order` lists book indices in
    /// sorted position order (so page N is a plain slice); `rank` maps a book
    /// index back to its position, which lets a filtered subset be ordered by
    /// lookup instead of re-sorting the whole library.
    pub fn sort_set(&self, sort: &str) -> SqlResult<Arc<SortSet>> {
        let books = self.get_all_books()?;

        {
            let cache = self.sorts.get_or_init(|| Mutex::new(HashMap::new()));
            if let Some(hit) = cache.lock().unwrap().get(sort).cloned() {
                return Ok(hit);
            }
        }

        let n = books.len();
        let mut order: Vec<usize> = (0..n).collect();
        match sort {
            "title" => order.sort_by_cached_key(|&i| {
                books[i]
                    .sort
                    .clone()
                    .unwrap_or_else(|| books[i].title.clone())
                    .to_lowercase()
            }),
            "author" => {
                // `authors` carries the "last name first" sort value; map the
                // display name used on books to that.
                let authors = self.get_all_authors()?;
                let sort_names: HashMap<String, String> = authors
                    .iter()
                    .map(|a| {
                        (
                            a.name.to_lowercase(),
                            a.sort.clone().unwrap_or_else(|| a.name.clone()).to_lowercase(),
                        )
                    })
                    .collect();

                order.sort_by_cached_key(|&i| {
                    books[i]
                        .authors
                        .first()
                        .map(|a| {
                            sort_names
                                .get(&a.to_lowercase())
                                .cloned()
                                .unwrap_or_else(|| a.to_lowercase())
                        })
                        .unwrap_or_default()
                });
            }
            "series" => order.sort_by_cached_key(|&i| {
                let b = &books[i];
                let name = b
                    .series
                    .as_ref()
                    .map(|s| s.to_lowercase())
                    .unwrap_or_else(|| "\u{10FFFF}".to_string());
                let index = b.series_index.map(|x| (x * 1000.0) as i64).unwrap_or(i64::MAX);
                (name, index)
            }),
            _ => {} // unknown key: keep the database order
        }

        let mut rank = vec![0usize; n];
        for (pos, &idx) in order.iter().enumerate() {
            rank[idx] = pos;
        }

        let set = Arc::new(SortSet { order, rank });
        let cache = self.sorts.get_or_init(|| Mutex::new(HashMap::new()));
        cache.lock().unwrap().insert(sort.to_string(), set.clone());
        Ok(set)
    }

    /// Load every cache up front so the first page request does not stall.
    pub fn warm_cache(&self) -> SqlResult<()> {
        self.get_all_books()?;
        self.book_paths()?;
        self.get_all_authors()?;
        self.get_all_tags()?;
        self.get_all_series()?;
        Ok(())
    }
}

pub struct SortSet {
    pub order: Vec<usize>,
    pub rank: Vec<usize>,
}

/// Lowercased, per-book copies of every field a filter can touch. Titles,
/// authors and series are additionally concatenated into one search string
/// (fields separated by `\u{1}` so a term cannot match across a boundary),
/// which turns a text search into a single `contains` per book.
pub struct BookIndex {
    pub search: Vec<String>,
    pub authors: Vec<Vec<String>>,
    pub tags: Vec<Vec<String>>,
    pub series: Vec<Option<String>>,
    pub formats: Vec<Vec<String>>,
}
