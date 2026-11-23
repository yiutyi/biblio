use actix_web::{web, HttpResponse, Result};
use serde::{Deserialize, Serialize};
use std::sync::{Arc, RwLock};
use actix_files::NamedFile;
use actix_web::http::header::{ContentDisposition, DispositionType, CacheControl, CacheDirective, DispositionParam};
use crate::cache::CacheStore;
use crate::library::{LibraryCache, LibraryMetadata};
use crate::db::Book;
use crate::config;
use crate::auth;
#[allow(unused_imports)]
use crate::session;
#[allow(unused_imports)]
use crate::audit;
#[allow(unused_imports)]
use crate::rbac;
use tracing::error;

#[derive(Debug, Serialize)]
pub struct ApiResponse<T> {
    pub success: bool,
    pub data: Option<T>,
    pub error: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct FilterQuery {
    /// Formats joined with '|' (e.g. "EPUB|PDF"). A single value works too.
    pub formats: Option<String>,
    pub search: Option<String>,
    /// Author / tag / series names joined with '|'
    pub authors: Option<String>,
    pub tags: Option<String>,
    pub series: Option<String>,
    /// Primary classification names joined with '|'
    pub categories: Option<String>,
    /// recent (default, newest first) | title | author | series
    pub sort: Option<String>,
    pub offset: Option<usize>,
    pub limit: Option<usize>,
}

#[derive(Debug, Serialize)]
pub struct BooksPage<'a> {
    pub success: bool,
    pub data: &'a [Book],
    pub total: usize,
    pub offset: usize,
    pub error: Option<String>,
}

fn split_list(value: &Option<String>) -> Vec<String> {
    match value {
        Some(v) => v
            .split('|')
            .map(|s| s.trim())
            .filter(|s| !s.is_empty())
            .map(|s| s.to_lowercase())
            .collect(),
        None => Vec::new(),
    }
}

/// Filter the in-memory book set and return one page of it.
///
/// Everything stays server-side: the browser only ever downloads the page it is
/// about to render, instead of the whole library (60 MB at 123k books).
pub async fn get_books(
    cache: web::Data<RwLock<LibraryCache>>,
    path: web::Path<String>,
    query: web::Query<FilterQuery>,
) -> Result<HttpResponse> {
    let library_id = path.into_inner();
    let query = query.into_inner();
    let limit = query.limit.unwrap_or(100).clamp(1, 1000);
    let offset = query.offset.unwrap_or(0);

    let search = query.search.clone();
    let formats = split_list(&query.formats);
    let authors = split_list(&query.authors);
    let tags = split_list(&query.tags);
    let series = split_list(&query.series);
    let categories = split_list(&query.categories);
    let sort = query.sort.clone().unwrap_or_else(|| "recent".to_string());

    // Filtering/sorting 123k rows is CPU work; keep it off the async workers.
    let work = {
        let cache = cache.clone();
        web::block(move || -> std::result::Result<(Vec<Book>, usize), String> {
            let (books, db) = {
                let guard = cache.read().unwrap();
                let db = guard
                    .get_database(&library_id)
                    .ok_or_else(|| "Library not found".to_string())?;
                let books = db
                    .lock()
                    .unwrap()
                    .get_all_books()
                    .map_err(|e| e.to_string())?;
                (books, db)
            };

            let filtering = search
                .as_ref()
                .map(|t| !t.trim().is_empty())
                .unwrap_or(false)
                || !authors.is_empty()
                || !tags.is_empty()
                || !series.is_empty()
                || !formats.is_empty()
                || !categories.is_empty();

            // Fast path: no filters. The ordering is precomputed, so a page is
            // just a slice of indices - no per-request sorting at all.
            if !filtering {
                let total = books.len();
                let order = if sort == "recent" {
                    None
                } else {
                    Some(
                        db.lock()
                            .unwrap()
                            .sort_set(&sort)
                            .map_err(|e| e.to_string())?,
                    )
                };

                let pick = |i: usize| books[i].clone();
                let page: Vec<Book> = match &order {
                    Some(set) => set
                        .order
                        .iter()
                        .skip(offset)
                        .take(limit)
                        .map(|&i| pick(i))
                        .collect(),
                    None => books.iter().skip(offset).take(limit).cloned().collect(),
                };
                return Ok((page, total));
            }

            // Filtered path: keep indices, order them by the precomputed rank
            // when a sort is active, then slice. All field comparisons run
            // against the prebuilt lowercase index, so a filter costs string
            // scans instead of 123k lowercase allocations per request.
            let idx = db
                .lock()
                .unwrap()
                .book_index()
                .map_err(|e| e.to_string())?;

            let mut filtered: Vec<usize> = (0..idx.search.len()).collect();

            if let Some(term) = &search {
                if !term.trim().is_empty() {
                    let needle = term.to_lowercase();
                    filtered.retain(|&i| idx.search[i].contains(&needle));
                }
            }

            if !authors.is_empty() {
                filtered.retain(|&i| {
                    idx.authors[i]
                        .iter()
                        .any(|a| authors.iter().any(|sel| a == sel))
                });
            }

            if !tags.is_empty() {
                filtered.retain(|&i| {
                    idx.tags[i]
                        .iter()
                        .any(|t| tags.iter().any(|sel| t == sel))
                });
            }

            if !series.is_empty() {
                filtered.retain(|&i| {
                    idx.series[i]
                        .as_ref()
                        .map(|s| series.iter().any(|sel| s == sel))
                        .unwrap_or(false)
                });
            }

            if !formats.is_empty() {
                filtered.retain(|&i| {
                    idx.formats[i]
                        .iter()
                        .any(|f| formats.iter().any(|sel| f == sel))
                });
            }

            if !categories.is_empty() {
                filtered.retain(|&i| {
                    books[i]
                        .category
                        .as_ref()
                        .map(|c| categories.iter().any(|sel| c.eq_ignore_ascii_case(sel)))
                        .unwrap_or(false)
                });
            }

            if sort != "recent" {
                let set = db
                    .lock()
                    .unwrap()
                    .sort_set(&sort)
                    .map_err(|e| e.to_string())?;
                filtered.sort_by_key(|&i| set.rank[i]);
            }

            let total = filtered.len();
            let page: Vec<Book> = filtered
                .into_iter()
                .skip(offset)
                .take(limit)
                .map(|i| books[i].clone())
                .collect();
            Ok((page, total))
        })
        .await
    };

    match work {
        Ok(Ok((page, total))) => Ok(HttpResponse::Ok().json(BooksPage {
            success: true,
            data: &page,
            total,
            offset,
            error: None,
        })),
        Ok(Err(message)) if message == "Library not found" => Ok(HttpResponse::NotFound().json(ApiResponse::<Vec<Book>> {
            success: false,
            data: None,
            error: Some(message),
        })),
        Ok(Err(message)) => Ok(HttpResponse::InternalServerError().json(ApiResponse::<Vec<Book>> {
            success: false,
            data: None,
            error: Some(message),
        })),
        Err(e) => Ok(HttpResponse::InternalServerError().json(ApiResponse::<Vec<Book>> {
            success: false,
            data: None,
            error: Some(format!("Blocking task failed: {}", e)),
        })),
    }
}

/// A book plus the library it came from. `Book.id` is only unique within one
/// Calibre library, so cross-library results must carry the source id.
#[derive(Debug, Serialize)]
pub struct ScopedBook {
    #[serde(flatten)]
    pub book: Book,
    pub library_id: String,
}

#[derive(Debug, Serialize)]
pub struct ScopedBooksPage<'a> {
    pub success: bool,
    pub data: &'a [ScopedBook],
    pub total: usize,
    pub offset: usize,
    pub error: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct MultiFilterQuery {
    /// Library ids joined with '|'. Omitted or empty means all libraries.
    pub libs: Option<String>,
    pub formats: Option<String>,
    pub search: Option<String>,
    pub authors: Option<String>,
    pub tags: Option<String>,
    pub series: Option<String>,
    pub categories: Option<String>,
    pub sort: Option<String>,
    pub offset: Option<usize>,
    pub limit: Option<usize>,
}

fn title_sort_key(b: &Book) -> String {
    b.sort.clone().unwrap_or_else(|| b.title.clone()).to_lowercase()
}

fn author_sort_key(b: &Book) -> String {
    b.authors.first().map(|a| a.to_lowercase()).unwrap_or_default()
}

fn series_sort_key(b: &Book) -> (String, i64) {
    (
        b.series
            .as_ref()
            .map(|s| s.to_lowercase())
            .unwrap_or_else(|| "\u{10FFFF}".to_string()),
        b.series_index.map(|x| (x * 1000.0) as i64).unwrap_or(i64::MAX),
    )
}

/// Search/browse across any subset of libraries in one request.
pub async fn get_books_multi(
    cache: web::Data<RwLock<LibraryCache>>,
    query: web::Query<MultiFilterQuery>,
) -> Result<HttpResponse> {
    let query = query.into_inner();
    let limit = query.limit.unwrap_or(100).clamp(1, 1000);
    let offset = query.offset.unwrap_or(0);
    let sort = query.sort.clone().unwrap_or_else(|| "recent".to_string());
    let formats = split_list(&query.formats);
    let authors = split_list(&query.authors);
    let tags = split_list(&query.tags);
    let series = split_list(&query.series);
    let categories = split_list(&query.categories);
    let needle = query
        .search
        .as_ref()
        .map(|t| t.trim().to_lowercase())
        .filter(|t| !t.is_empty());
    let requested: Vec<String> = query
        .libs
        .as_ref()
        .map(|v| {
            v.split('|')
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(String::from)
                .collect()
        })
        .unwrap_or_default();

    let work = {
        let cache = cache.clone();
        web::block(move || -> std::result::Result<(Vec<ScopedBook>, usize), String> {
            let dbs: Vec<_> = {
                let guard = cache.read().unwrap();
                let ids: Vec<String> = if requested.is_empty() {
                    guard.get_libraries().into_iter().map(|l| l.id).collect()
                } else {
                    requested
                };
                ids.into_iter()
                    .filter_map(|id| guard.get_database(&id).map(|db| (id, db)))
                    .collect()
            };
            if dbs.is_empty() {
                return Err("Library not found".to_string());
            }

            let filtering = needle.is_some()
                || !authors.is_empty()
                || !tags.is_empty()
                || !series.is_empty()
                || !formats.is_empty()
                || !categories.is_empty();

            let mut all: Vec<ScopedBook> = Vec::new();
            for (lib_id, db) in dbs {
                let books = db
                    .lock()
                    .unwrap()
                    .get_all_books()
                    .map_err(|e| e.to_string())?;
                let idx = if filtering {
                    Some(db.lock().unwrap().book_index().map_err(|e| e.to_string())?)
                } else {
                    None
                };

                for (i, book) in books.iter().enumerate() {
                    if let Some(idx) = &idx {
                        if let Some(n) = &needle {
                            if !idx.search[i].contains(n) {
                                continue;
                            }
                        }
                        if !authors.is_empty()
                            && !idx.authors[i].iter().any(|a| authors.iter().any(|sel| a == sel))
                        {
                            continue;
                        }
                        if !tags.is_empty()
                            && !idx.tags[i].iter().any(|t| tags.iter().any(|sel| t == sel))
                        {
                            continue;
                        }
                        if !series.is_empty()
                            && !idx.series[i]
                                .as_ref()
                                .map(|s| series.iter().any(|sel| s == sel))
                                .unwrap_or(false)
                        {
                            continue;
                        }
                        if !formats.is_empty()
                            && !idx.formats[i].iter().any(|f| formats.iter().any(|sel| f == sel))
                        {
                            continue;
                        }
                        if !categories.is_empty()
                            && !book
                                .category
                                .as_ref()
                                .map(|c| categories.iter().any(|sel| c.eq_ignore_ascii_case(sel)))
                                .unwrap_or(false)
                        {
                            continue;
                        }
                    }
                    all.push(ScopedBook {
                        book: book.clone(),
                        library_id: lib_id.clone(),
                    });
                }
            }

            // Per-library sort ranks are not comparable, so sort the merged set
            // by field values. Calibre timestamps share one string shape, so a
            // plain descending compare gives the same "recent" order.
            match sort.as_str() {
                "title" => all.sort_by_cached_key(|s| title_sort_key(&s.book)),
                "author" => all.sort_by_cached_key(|s| author_sort_key(&s.book)),
                "series" => all.sort_by_cached_key(|s| series_sort_key(&s.book)),
                _ => all.sort_by(|a, b| b.book.timestamp.cmp(&a.book.timestamp)),
            }

            let total = all.len();
            let page = all.into_iter().skip(offset).take(limit).collect();
            Ok((page, total))
        })
        .await
    };

    match work {
        Ok(Ok((page, total))) => Ok(HttpResponse::Ok().json(ScopedBooksPage {
            success: true,
            data: &page,
            total,
            offset,
            error: None,
        })),
        Ok(Err(message)) if message == "Library not found" => {
            Ok(HttpResponse::NotFound().json(ApiResponse::<Vec<ScopedBook>> {
                success: false,
                data: None,
                error: Some(message),
            }))
        }
        Ok(Err(message)) => Ok(HttpResponse::InternalServerError().json(ApiResponse::<Vec<ScopedBook>> {
            success: false,
            data: None,
            error: Some(message),
        })),
        Err(e) => Ok(HttpResponse::InternalServerError().json(ApiResponse::<Vec<ScopedBook>> {
            success: false,
            data: None,
            error: Some(format!("Blocking task failed: {}", e)),
        })),
    }
}

#[derive(Debug, Deserialize)]
pub struct LoginRequest {
    pub username: String,
    pub password: String,
}

#[derive(Debug, Deserialize)]
pub struct ChangePasswordRequest {
    pub username: String,
    pub current_password: String,
    pub new_password: String,
}

#[derive(Debug, Deserialize)]
pub struct AuditLogQuery {
    pub username: Option<String>,
    pub limit: Option<usize>,
}

#[derive(Debug, Deserialize)]
pub struct ClientDiagnostics {
    pub window_width: u32,
    pub window_height: u32,
    // pub content_area_width: u32,
    // pub content_area_x: i32,
    // pub content_area_y: i32,
    // pub content_area_left: i32,
    // pub content_area_top: i32,
    // pub content_area_right: i32,
    // pub content_area_bottom: i32,
    // pub content_area_flex_direction: Option<String>,
    // pub left_panel_width: u32,
    // pub left_panel_x: i32,
    // pub left_panel_y: i32,
    // pub left_panel_left: i32,
    // pub left_panel_top: i32,
    // pub left_panel_right: i32,
    // pub left_panel_bottom: i32,
    // pub left_panel_height: i32,
    // pub right_panel_width: u32,
    // pub right_panel_x: i32,
    // pub right_panel_y: i32,
    // pub right_panel_left: i32,
    // pub right_panel_top: i32,
    // pub right_panel_right: i32,
    // pub right_panel_bottom: i32,
    // pub right_panel_height: i32,
    // pub splitter_left_x: i32,
    // pub splitter_left_y: i32,
    // pub splitter_right_x: i32,
    // pub splitter_right_y: i32,
    // pub available_width: u32,
    pub min_center_width: u32,
    // pub total_required_width: u32,
    pub adjustment_needed: bool,
    // pub scale_factor: Option<f64>,
    // pub new_left_width: Option<u32>,
    // pub new_right_width: Option<u32>,
}

// User management request/response structures
#[derive(Debug, Deserialize)]
pub struct CreateUserRequest {
    pub username: String,
    pub password: String,
    pub role: Option<String>, // admin, librarian, user, reader (default: reader)
    pub email: Option<String>,
    pub admin_username: Option<String>, // Username of the admin making this request
}

#[derive(Debug, Deserialize)]
pub struct UpdateUserRequest {
    pub role: Option<String>,
    pub email: Option<String>,
    pub admin_username: Option<String>, // Username of the admin making this request
}

#[derive(Debug, Deserialize)]
pub struct AdminChangePasswordRequest {
    pub username: String,
    pub new_password: String,
    pub admin_username: Option<String>, // Username of the admin making this request
}

#[derive(Debug, Deserialize)]
pub struct DeleteUserRequest {
    pub admin_username: Option<String>, // Username of the admin making this request
}

#[derive(Debug, Serialize)]
pub struct UserResponse {
    pub username: String,
    pub role: String,
    pub email: Option<String>,
    pub created_at: Option<String>,
}

pub async fn get_libraries(
    cache: web::Data<RwLock<LibraryCache>>,
) -> Result<HttpResponse> {
    let cache = cache.read().unwrap();
    let libraries = cache.get_libraries();
    
    Ok(HttpResponse::Ok().json(ApiResponse {
        success: true,
        data: Some(libraries),
        error: None,
    }))
}

pub async fn get_library(
    cache: web::Data<RwLock<LibraryCache>>,
    path: web::Path<String>,
) -> Result<HttpResponse> {
    let library_id = path.into_inner();
    let cache = cache.read().unwrap();
    
    if let Some(lib) = cache.get_library(&library_id) {
        Ok(HttpResponse::Ok().json(ApiResponse {
            success: true,
            data: Some(lib.clone()),
            error: None,
        }))
    } else {
        Ok(HttpResponse::NotFound().json(ApiResponse::<LibraryMetadata> {
            success: false,
            data: None,
            error: Some("Library not found".to_string()),
        }))
    }
}

pub async fn get_book(
    cache: web::Data<RwLock<LibraryCache>>,
    path: web::Path<(String, i32)>,
) -> Result<HttpResponse> {
    let (library_id, book_id) = path.into_inner();
    let cache = cache.read().unwrap();
    
    if let Some(db) = cache.get_database(&library_id) {
        match db.lock().unwrap().get_book(book_id) {
            Ok(Some(book)) => {
                Ok(HttpResponse::Ok().json(ApiResponse {
                    success: true,
                    data: Some(book),
                    error: None,
                }))
            }
            Ok(None) => {
                Ok(HttpResponse::NotFound().json(ApiResponse::<Book> {
                    success: false,
                    data: None,
                    error: Some("Book not found".to_string()),
                }))
            }
            Err(e) => {
                Ok(HttpResponse::InternalServerError().json(ApiResponse::<Book> {
                    success: false,
                    data: None,
                    error: Some(format!("Database error: {}", e)),
                }))
            }
        }
    } else {
        Ok(HttpResponse::NotFound().json(ApiResponse::<Book> {
            success: false,
            data: None,
            error: Some("Library not found".to_string()),
        }))
    }
}

pub async fn get_authors(
    cache: web::Data<RwLock<LibraryCache>>,
    path: web::Path<String>,
) -> Result<HttpResponse> {
    let library_id = path.into_inner();
    let cache = cache.read().unwrap();
    
    if let Some(db) = cache.get_database(&library_id) {
        match db.lock().unwrap().get_all_authors() {
            Ok(authors) => {
                Ok(HttpResponse::Ok().json(ApiResponse {
                    success: true,
                    data: Some(authors),
                    error: None,
                }))
            }
            Err(e) => {
                Ok(HttpResponse::InternalServerError().json(ApiResponse {
                    success: false,
                    data: None as Option<Vec<String>>,
                    error: Some(format!("Database error: {}", e)),
                }))
            }
        }
    } else {
        Ok(HttpResponse::NotFound().json(ApiResponse {
            success: false,
            data: None as Option<Vec<String>>,
            error: Some("Library not found".to_string()),
        }))
    }
}

#[derive(Debug, Serialize)]
pub struct FormatStat {
    pub format: String,
    pub count: usize,
}

pub async fn get_format_stats(
    cache: web::Data<RwLock<LibraryCache>>,
    path: web::Path<String>,
) -> Result<HttpResponse> {
    let library_id = path.into_inner();
    let cache = cache.read().unwrap();

    if let Some(db) = cache.get_database(&library_id) {
        match db.lock().unwrap().format_stats() {
            Ok(stats) => {
                let data: Vec<FormatStat> = stats
                    .into_iter()
                    .map(|(format, count)| FormatStat { format, count })
                    .collect();
                Ok(HttpResponse::Ok().json(ApiResponse {
                    success: true,
                    data: Some(data),
                    error: None,
                }))
            }
            Err(e) => Ok(HttpResponse::InternalServerError().json(ApiResponse {
                success: false,
                data: None as Option<Vec<FormatStat>>,
                error: Some(format!("Database error: {}", e)),
            })),
        }
    } else {
        Ok(HttpResponse::NotFound().json(ApiResponse {
            success: false,
            data: None as Option<Vec<FormatStat>>,
            error: Some("Library not found".to_string()),
        }))
    }
}

#[derive(Debug, Serialize)]
pub struct CategoryStat {
    pub name: String,
    pub count: usize,
}

pub async fn get_categories(
    cache: web::Data<RwLock<LibraryCache>>,
    path: web::Path<String>,
) -> Result<HttpResponse> {
    let library_id = path.into_inner();
    let db = {
        let guard = cache.read().unwrap();
        guard.get_database(&library_id)
    };

    if let Some(db) = db {
        let stats = web::block(move || db.lock().unwrap().category_stats()).await;
        match stats {
            Ok(Ok(stats)) => {
                let data: Vec<CategoryStat> = stats
                    .iter()
                    .map(|(name, count)| CategoryStat {
                        name: name.clone(),
                        count: *count,
                    })
                    .collect();
                Ok(HttpResponse::Ok().json(ApiResponse {
                    success: true,
                    data: Some(data),
                    error: None,
                }))
            }
            Ok(Err(e)) => Ok(HttpResponse::InternalServerError().json(ApiResponse {
                success: false,
                data: None as Option<Vec<CategoryStat>>,
                error: Some(format!("Database error: {}", e)),
            })),
            Err(e) => Ok(HttpResponse::InternalServerError().json(ApiResponse {
                success: false,
                data: None as Option<Vec<CategoryStat>>,
                error: Some(format!("Blocking task failed: {}", e)),
            })),
        }
    } else {
        Ok(HttpResponse::NotFound().json(ApiResponse {
            success: false,
            data: None as Option<Vec<CategoryStat>>,
            error: Some("Library not found".to_string()),
        }))
    }
}

pub async fn get_tags(
    cache: web::Data<RwLock<LibraryCache>>,
    path: web::Path<String>,
) -> Result<HttpResponse> {
    let library_id = path.into_inner();
    let cache = cache.read().unwrap();

    if let Some(db) = cache.get_database(&library_id) {
        match db.lock().unwrap().get_all_tags() {
            Ok(tags) => {
                Ok(HttpResponse::Ok().json(ApiResponse {
                    success: true,
                    data: Some(tags),
                    error: None,
                }))
            }
            Err(e) => {
                Ok(HttpResponse::InternalServerError().json(ApiResponse {
                    success: false,
                    data: None as Option<Vec<String>>,
                    error: Some(format!("Database error: {}", e)),
                }))
            }
        }
    } else {
        Ok(HttpResponse::NotFound().json(ApiResponse {
            success: false,
            data: None as Option<Vec<String>>,
            error: Some("Library not found".to_string()),
        }))
    }
}

pub async fn get_series(
    cache: web::Data<RwLock<LibraryCache>>,
    path: web::Path<String>,
) -> Result<HttpResponse> {
    let library_id = path.into_inner();
    let cache = cache.read().unwrap();
    
    if let Some(db) = cache.get_database(&library_id) {
        match db.lock().unwrap().get_all_series() {
            Ok(series) => {
                Ok(HttpResponse::Ok().json(ApiResponse {
                    success: true,
                    data: Some(series),
                    error: None,
                }))
            }
            Err(e) => {
                Ok(HttpResponse::InternalServerError().json(ApiResponse {
                    success: false,
                    data: None as Option<Vec<String>>,
                    error: Some(format!("Database error: {}", e)),
                }))
            }
        }
    } else {
        Ok(HttpResponse::NotFound().json(ApiResponse {
            success: false,
            data: None as Option<Vec<String>>,
            error: Some("Library not found".to_string()),
        }))
    }
}

#[derive(Debug, Deserialize)]
pub struct CoverQuery {
    /// "full" returns the original scan; anything else returns a cached thumbnail
    pub size: Option<String>,
}

pub async fn get_book_cover(
    cache: web::Data<RwLock<LibraryCache>>,
    store: web::Data<Arc<CacheStore>>,
    path: web::Path<(String, i32)>,
    query: web::Query<CoverQuery>,
) -> Result<HttpResponse> {
    let (library_id, book_id) = path.into_inner();
    let want_full = query.size.as_deref() == Some("full");

    // Resolve the book folder from `books.path` (one indexed lookup), then do
    // all file work off the async worker threads.
    let resolved = {
        let cache = cache.read().unwrap();
        match (cache.get_library(&library_id), cache.get_database(&library_id)) {
            (Some(lib), Some(db)) => db
                .lock()
                .unwrap()
                .get_book_path(book_id)
                .map(|rel| lib.path.join(rel).join("cover.jpg")),
            _ => None,
        }
    };

    let cover_path = match resolved {
        Some(p) => p,
        None => return Ok(HttpResponse::NotFound().finish()),
    };

    let store = store.get_ref().clone();
    let thumb_size = config::thumbnail_size();
    let library_for_cache = library_id.clone();

    let bytes = web::block(move || -> Option<Vec<u8>> {
        if want_full {
            return std::fs::read(&cover_path).ok();
        }
        // Cache a downscaled JPEG on the local disk; the grid then never re-reads
        // multi-hundred-KB scans from the (possibly remote) library.
        if let Ok(thumb) = store.ensure_thumbnail(&cover_path, &library_for_cache, book_id, thumb_size) {
            if let Ok(data) = std::fs::read(&thumb) {
                return Some(data);
            }
        }
        std::fs::read(&cover_path).ok()
    })
    .await;

    match bytes {
        Ok(Some(data)) => Ok(HttpResponse::Ok()
            .content_type("image/jpeg")
            .insert_header(CacheControl(vec![
                CacheDirective::Public,
                CacheDirective::MaxAge(604_800),
            ]))
            .body(data)),
        _ => Ok(HttpResponse::NotFound().finish()),
    }
}

pub async fn get_book_formats(
    cache: web::Data<RwLock<LibraryCache>>,
    path: web::Path<(String, i32)>,
) -> Result<HttpResponse> {
    let (library_id, book_id) = path.into_inner();
    let cache = cache.read().unwrap();
    
    if let Some(db) = cache.get_database(&library_id) {
        match db.lock().unwrap().get_book_formats(book_id) {
            Ok(formats) => {
                Ok(HttpResponse::Ok().json(ApiResponse {
                    success: true,
                    data: Some(formats),
                    error: None,
                }))
            }
            Err(e) => {
                Ok(HttpResponse::InternalServerError().json(ApiResponse::<Vec<String>> {
                    success: false,
                    data: None,
                    error: Some(format!("Database error: {}", e)),
                }))
            }
        }
    } else {
        Ok(HttpResponse::NotFound().json(ApiResponse::<Vec<String>> {
            success: false,
            data: None,
            error: Some("Library not found".to_string()),
        }))
    }
}

pub async fn get_book_file(
    req: actix_web::HttpRequest,
    cache: web::Data<RwLock<LibraryCache>>,
    path: web::Path<(String, i32, String)>,
) -> Result<HttpResponse> {
    let (library_id, book_id, format) = path.into_inner();
    let format_upper = format.to_uppercase();

    // Locate the requested file inside the one book folder on the blocking
    // pool (a directory scan on a network share must not stall an async worker),
    // then release the cache lock before streaming.
    let locate = {
        let cache = cache.clone();
        let library_id = library_id.clone();
        let format_upper = format_upper.clone();
        web::block(move || -> Option<(std::path::PathBuf, String)> {
            let mut found = None;
            {
                let cache = cache.read().unwrap();
                if let (Some(lib), Some(db)) =
                    (cache.get_library(&library_id), cache.get_database(&library_id))
                {
                    if let Some(rel_path) = db.lock().unwrap().get_book_path(book_id) {
                        let book_path = lib.path.clone().join(rel_path);
                        for entry in std::fs::read_dir(&book_path).ok()?.flatten() {
                            let file_path = entry.path();
                            if !file_path.is_file() {
                                continue;
                            }
                            let matches = file_path
                                .extension()
                                .map(|e| e.to_string_lossy().to_uppercase() == format_upper)
                                .unwrap_or(false);
                            if matches {
                                let name = file_path
                                    .file_name()
                                    .and_then(|n| n.to_str())
                                    .unwrap_or("book")
                                    .to_string();
                                found = Some((file_path, name));
                                break;
                            }
                        }
                    }
                }
            }
            found
        })
        .await
    };
    let target = match locate {
        Ok(target) => target,
        Err(_) => None,
    };

    match target {
        Some((file_path, filename)) => {
            // Streaming (range requests supported) instead of loading the whole
            // book into memory: a 50 MB PDF no longer allocates 50 MB per request.
            let file = NamedFile::open_async(&file_path).await?;
            // A plain `filename=` parameter is latin1-only, so Chinese titles
            // turned into mojibake; browsers fall back to the URL otherwise.
            let header_name = if filename.is_ascii() {
                filename
            } else {
                format!("book.{}", format.to_lowercase())
            };
            let file = file.set_content_disposition(ContentDisposition {
                disposition: DispositionType::Inline,
                parameters: vec![DispositionParam::Filename(header_name)],
            });
            Ok(file.into_response(&req))
        }
        None => Ok(HttpResponse::NotFound().finish()),
    }
}

pub async fn refresh_libraries(
    cache: web::Data<RwLock<LibraryCache>>,
    store: web::Data<Arc<CacheStore>>,
) -> Result<HttpResponse> {
    // Re-read config.yaml so library path edits take effect without restart.
    if let Err(e) = config::reload() {
        error!("Failed to reload configuration: {}", e);
    }
    let entries: Vec<(String, String)> = config::library_entries()
        .into_iter()
        .map(|e| (e.name, e.path))
        .collect();
    let snapshot_mode = config::db_snapshot();
    let store = store.get_ref().clone();

    // Rescan + re-snapshot + reload is a long blocking job: run it on the
    // blocking pool so the server keeps answering while it works.
    let work = {
        let cache = cache.clone();
        web::block(move || -> std::io::Result<Vec<LibraryMetadata>> {
            let mut cache = cache.write().unwrap();
            cache.clear();
            cache.load_named_libraries(&entries, store, snapshot_mode, true)?;
            Ok(cache.get_libraries())
        })
        .await
    };

    match work {
        Ok(Ok(libraries)) => Ok(HttpResponse::Ok().json(ApiResponse {
            success: true,
            data: Some(libraries),
            error: None,
        })),
        Ok(Err(e)) => Ok(HttpResponse::InternalServerError().json(ApiResponse {
            success: false,
            data: None::<Vec<LibraryMetadata>>,
            error: Some(format!("Failed to refresh libraries: {}", e)),
        })),
        Err(e) => Ok(HttpResponse::InternalServerError().json(ApiResponse {
            success: false,
            data: None::<Vec<LibraryMetadata>>,
            error: Some(format!("Blocking task failed: {}", e)),
        })),
    }
}

pub async fn login(
    req: web::Json<LoginRequest>,
    _users: web::Data<Vec<auth::User>>,
    audit_logger: web::Data<audit::AuditLogger>,
) -> Result<HttpResponse> {
    // Load users from file to get the latest data (including newly created users)
    let users_path = config::users_file_path();
    let file_users = match auth::load_users(&users_path) {
        Ok(users) => users,
        Err(e) => {
            audit_logger.log_event(
                audit::AuditEventType::LoginFailure,
                &req.username,
                "127.0.0.1",
                &format!("Error loading users: {}", e),
                false,
            );
            
            return Ok(HttpResponse::InternalServerError().json(ApiResponse {
                success: false,
                data: None::<serde_json::Value>,
                error: Some(format!("Authentication system error: {}", e)),
            }));
        }
    };

    // Validate username and password
    match auth::authenticate_user(&req.username, &req.password, &file_users) {
        Ok(true) => {
            // Find the user to get their role
            let user_role = file_users.iter()
                .find(|u| u.username == req.username)
                .map(|u| u.role.clone())
                .unwrap_or_else(|| "reader".to_string());
            
            audit_logger.log_event(
                audit::AuditEventType::LoginSuccess,
                &req.username,
                "127.0.0.1",
                "User logged in successfully",
                true,
            );
            
            Ok(HttpResponse::Ok().json(ApiResponse {
                success: true,
                data: Some(serde_json::json!({"username": req.username, "role": user_role})),
                error: None,
            }))
        }
        Ok(false) => {
            audit_logger.log_event(
                audit::AuditEventType::LoginFailure,
                &req.username,
                "127.0.0.1",
                "Invalid credentials",
                false,
            );
            
            Ok(HttpResponse::Unauthorized().json(ApiResponse {
                success: false,
                data: None::<serde_json::Value>,
                error: Some("Invalid credentials".to_string()),
            }))
        }
        Err(e) => {
            audit_logger.log_event(
                audit::AuditEventType::LoginFailure,
                &req.username,
                "127.0.0.1",
                &format!("Authentication error: {}", e),
                false,
            );
            
            Ok(HttpResponse::InternalServerError().json(ApiResponse {
                success: false,
                data: None::<serde_json::Value>,
                error: Some(format!("Authentication error: {}", e)),
            }))
        }
    }
}

pub async fn logout(
    audit_logger: web::Data<audit::AuditLogger>,
) -> Result<HttpResponse> {
    audit_logger.log_event(
        audit::AuditEventType::LogoutSuccess,
        "unknown",
        "127.0.0.1",
        "User logged out",
        true,
    );
    
    Ok(HttpResponse::Ok().json(ApiResponse {
        success: true,
        data: Some(serde_json::json!({"message": "logged out"})),
        error: None,
    }))
}

pub async fn log_client_diagnostics(
    diagnostics: web::Json<ClientDiagnostics>,
) -> Result<HttpResponse> {
    use tracing::info;
    
    info!("📱 Client Diagnostics: {}x{} | Adjustment: {} | Center: {}% ({}px)", 
        diagnostics.window_width, 
        diagnostics.window_height,
        if diagnostics.adjustment_needed { "YES" } else { "NO" },
        55,
        diagnostics.min_center_width
    );
    
    // Detailed geometry logging available for debugging (commented out by default)
    // Uncomment below to see full geometry details
    /*
    info!("  ─── CONTENT AREA ───");
    info!("    Width: {}px", diagnostics.content_area_width);
    info!("    Bounds: x={}, y={}, left={}, top={}, right={}, bottom={}", 
        diagnostics.content_area_x, diagnostics.content_area_y, 
        diagnostics.content_area_left, diagnostics.content_area_top, 
        diagnostics.content_area_right, diagnostics.content_area_bottom);
    if let Some(ref flex_dir) = diagnostics.content_area_flex_direction {
        info!("    Flex-direction: {}", flex_dir);
    }
    
    info!("  ─── LEFT PANEL ───");
    info!("    Width: {}px, Height: {}px", diagnostics.left_panel_width, diagnostics.left_panel_height);
    info!("    Position: x={}, y={} | Bounds: left={}, top={}, right={}, bottom={}", 
        diagnostics.left_panel_x, diagnostics.left_panel_y,
        diagnostics.left_panel_left, diagnostics.left_panel_top, 
        diagnostics.left_panel_right, diagnostics.left_panel_bottom);
    
    info!("  ─── RIGHT PANEL ───");
    info!("    Width: {}px, Height: {}px", diagnostics.right_panel_width, diagnostics.right_panel_height);
    info!("    Position: x={}, y={} | Bounds: left={}, top={}, right={}, bottom={}", 
        diagnostics.right_panel_x, diagnostics.right_panel_y,
        diagnostics.right_panel_left, diagnostics.right_panel_top, 
        diagnostics.right_panel_right, diagnostics.right_panel_bottom);
    
    info!("  ─── SPLITTERS ───");
    info!("    Splitter Left: x={}, y={}", diagnostics.splitter_left_x, diagnostics.splitter_left_y);
    info!("    Splitter Right: x={}, y={}", diagnostics.splitter_right_x, diagnostics.splitter_right_y);
    
    info!("  ─── LAYOUT ANALYSIS ───");
    info!("    Available width: {}px | Min center: {}px", diagnostics.available_width, diagnostics.min_center_width);
    info!("    Total required: {}px", diagnostics.total_required_width);
    */
    
    Ok(HttpResponse::Ok().json(ApiResponse {
        success: true,
        data: Some(serde_json::json!({"message": "diagnostics received"})),
        error: None,
    }))
}

pub async fn get_current_user(
    _users: web::Data<Vec<auth::User>>,
) -> Result<HttpResponse> {
    // Return a generic user response - the frontend maintains auth state via localStorage
    // In a production system with session/JWT, this would return the actual logged-in user
    Ok(HttpResponse::Ok().json(ApiResponse {
        success: true,
        data: Some(serde_json::json!({
            "username": "user",
            "role": "reader",
            "email": null
        })),
        error: None,
    }))
}

// Password Management Endpoints

pub async fn change_password(
    req: web::Json<ChangePasswordRequest>,
    _users: web::Data<Vec<auth::User>>,
    audit_logger: web::Data<audit::AuditLogger>,
) -> Result<HttpResponse> {
    // Validate new password strength
    if let Err(e) = auth::validate_password_strength(&req.new_password) {
        audit_logger.log_event(
            audit::AuditEventType::PasswordChange,
            &req.username,
            "127.0.0.1",
            &format!("Failed: {}", e),
            false,
        );
        
        return Ok(HttpResponse::BadRequest().json(ApiResponse {
            success: false,
            data: None::<serde_json::Value>,
            error: Some(e),
        }));
    }

    // Load users from file
    let users_path = config::users_file_path();
    let mut file_users = match auth::load_users(&users_path) {
        Ok(users) => users,
        Err(e) => {
            audit_logger.log_event(
                audit::AuditEventType::PasswordChange,
                &req.username,
                "127.0.0.1",
                &format!("Failed: {}", e),
                false,
            );

            return Ok(HttpResponse::InternalServerError().json(ApiResponse {
                success: false,
                data: None::<serde_json::Value>,
                error: Some(format!("Error loading users: {}", e)),
            }));
        }
    };

    // Authenticate user with current password
    match auth::authenticate_user(&req.username, &req.current_password, &file_users) {
        Ok(true) => {
            // Hash the new password
            match auth::hash_password(&req.new_password) {
                Ok(new_hash) => {
                    // Find and update the user
                    if let Some(user) = file_users.iter_mut().find(|u| u.username == req.username) {
                        user.password_hash = new_hash;

                        // Save updated users to file
                        if let Err(e) = auth::save_users(&file_users, &users_path) {
                            audit_logger.log_event(
                                audit::AuditEventType::PasswordChange,
                                &req.username,
                                "127.0.0.1",
                                &format!("Failed to save: {}", e),
                                false,
                            );

                            return Ok(HttpResponse::InternalServerError().json(ApiResponse {
                                success: false,
                                data: None::<serde_json::Value>,
                                error: Some(format!("Error saving password: {}", e)),
                            }));
                        }

                        audit_logger.log_event(
                            audit::AuditEventType::PasswordChange,
                            &req.username,
                            "127.0.0.1",
                            "Password changed successfully",
                            true,
                        );

                        Ok(HttpResponse::Ok().json(ApiResponse {
                            success: true,
                            data: Some(serde_json::json!({"message": "Password changed successfully"})),
                            error: None,
                        }))
                    } else {
                        audit_logger.log_event(
                            audit::AuditEventType::PasswordChange,
                            &req.username,
                            "127.0.0.1",
                            "Failed: User not found",
                            false,
                        );

                        Ok(HttpResponse::NotFound().json(ApiResponse {
                            success: false,
                            data: None::<serde_json::Value>,
                            error: Some("User not found".to_string()),
                        }))
                    }
                }
                Err(e) => {
                    audit_logger.log_event(
                        audit::AuditEventType::PasswordChange,
                        &req.username,
                        "127.0.0.1",
                        &format!("Failed to hash password: {}", e),
                        false,
                    );

                    Ok(HttpResponse::InternalServerError().json(ApiResponse {
                        success: false,
                        data: None::<serde_json::Value>,
                        error: Some(format!("Error processing password: {}", e)),
                    }))
                }
            }
        }
        Ok(false) => {
            audit_logger.log_event(
                audit::AuditEventType::PasswordChange,
                &req.username,
                "127.0.0.1",
                "Failed: Invalid current password",
                false,
            );

            Ok(HttpResponse::Unauthorized().json(ApiResponse {
                success: false,
                data: None::<serde_json::Value>,
                error: Some("Invalid current password".to_string()),
            }))
        }
        Err(e) => {
            Ok(HttpResponse::InternalServerError().json(ApiResponse {
                success: false,
                data: None::<serde_json::Value>,
                error: Some(format!("Error: {}", e)),
            }))
        }
    }
}


// User Management Endpoints (Admin Only)

/// Helper function to verify if a user has admin role
fn verify_admin_user(admin_username: Option<&str>) -> Result<(), String> {
    let username = admin_username.ok_or("Admin username required")?;
    
    let users_path = config::users_file_path();
    let file_users = auth::load_users(&users_path)
        .map_err(|e| format!("Failed to load users: {}", e))?;
    
    let user = file_users.iter()
        .find(|u| u.username == username)
        .ok_or("Admin user not found")?;
    
    if user.role != "admin" {
        return Err("User does not have admin role".to_string());
    }
    
    Ok(())
}

pub async fn list_users(
    query: web::Query<std::collections::HashMap<String, String>>,
    _users: web::Data<Vec<auth::User>>,
    audit_logger: web::Data<audit::AuditLogger>,
) -> Result<HttpResponse> {
    // Verify admin role (get from query parameter)
    let admin_username = query.get("admin_username").map(|s| s.as_str());
    
    if let Err(e) = verify_admin_user(admin_username) {
        audit_logger.log_event(
            audit::AuditEventType::UnauthorizedAccess,
            admin_username.unwrap_or("unknown"),
            "127.0.0.1",
            &format!("Unauthorized attempt to list users: {}", e),
            false,
        );

        return Ok(HttpResponse::Forbidden().json(ApiResponse {
            success: false,
            data: None::<Vec<UserResponse>>,
            error: Some("Unauthorized: Admin access required".to_string()),
        }));
    }

    // Read users from file to get the latest data (including newly created users)
    let users_path = config::users_file_path();
    match auth::load_users(&users_path) {
        Ok(file_users) => {
            let user_responses: Vec<UserResponse> = file_users
                .iter()
                .map(|u| UserResponse {
                    username: u.username.clone(),
                    role: u.role.clone(),
                    email: u.email.clone(),
                    created_at: u.created_at.clone(),
                })
                .collect();

            Ok(HttpResponse::Ok().json(ApiResponse {
                success: true,
                data: Some(user_responses),
                error: None,
            }))
        }
        Err(e) => {
            Ok(HttpResponse::InternalServerError().json(ApiResponse {
                success: false,
                data: None::<Vec<UserResponse>>,
                error: Some(format!("Error loading users: {}", e)),
            }))
        }
    }
}

pub async fn create_user(
    req: web::Json<CreateUserRequest>,
    users: web::Data<Vec<auth::User>>,
    audit_logger: web::Data<audit::AuditLogger>,
) -> Result<HttpResponse> {
    // Verify admin role
    if let Err(e) = verify_admin_user(req.admin_username.as_deref()) {
        audit_logger.log_event(
            audit::AuditEventType::UnauthorizedAccess,
            req.admin_username.as_deref().unwrap_or("unknown"),
            "127.0.0.1",
            &format!("Unauthorized attempt to create user: {}", e),
            false,
        );

        return Ok(HttpResponse::Forbidden().json(ApiResponse {
            success: false,
            data: None::<serde_json::Value>,
            error: Some("Unauthorized: Admin access required".to_string()),
        }));
    }

    // Validate password strength
    if let Err(e) = auth::validate_password_strength(&req.password) {
        audit_logger.log_event(
            audit::AuditEventType::UserCreated,
            req.admin_username.as_deref().unwrap_or("admin"),
            "127.0.0.1",
            &format!("Failed to create user {}: {}", req.username, e),
            false,
        );

        return Ok(HttpResponse::BadRequest().json(ApiResponse {
            success: false,
            data: None::<serde_json::Value>,
            error: Some(e),
        }));
    }

    // Check if user already exists
    if users.iter().any(|u| u.username == req.username) {
        audit_logger.log_event(
            audit::AuditEventType::UserCreated,
            "admin",
            "127.0.0.1",
            &format!("Failed to create user {}: user already exists", req.username),
            false,
        );

        return Ok(HttpResponse::Conflict().json(ApiResponse {
            success: false,
            data: None::<serde_json::Value>,
            error: Some("User already exists".to_string()),
        }));
    }

    // Hash the password
    match auth::hash_password(&req.password) {
        Ok(password_hash) => {
            let role = req.role.clone().unwrap_or_else(|| "reader".to_string());
            let created_at = chrono::Utc::now().to_rfc3339();
            
            // Create the new user
            let new_user = auth::User {
                username: req.username.clone(),
                password_hash,
                role: role.clone(),
                email: req.email.clone(),
                created_at: Some(created_at.clone()),
            };

            // Load all users from file (not from cache), add the new one, and save back to file
            let users_path = config::users_file_path();
            let mut all_users = match auth::load_users(&users_path) {
                Ok(users) => users,
                Err(e) => {
                    audit_logger.log_event(
                        audit::AuditEventType::UserCreated,
                        req.admin_username.as_deref().unwrap_or("admin"),
                        "127.0.0.1",
                        &format!("Failed to load users from file: {}", e),
                        false,
                    );
                    
                    return Ok(HttpResponse::InternalServerError().json(ApiResponse {
                        success: false,
                        data: None::<serde_json::Value>,
                        error: Some(format!("Error loading users: {}", e)),
                    }));
                }
            };

            all_users.push(new_user);

            // Save to file
            if let Err(e) = auth::save_users(&all_users, &users_path) {
                audit_logger.log_event(
                    audit::AuditEventType::UserCreated,
                    "admin",
                    "127.0.0.1",
                    &format!("Failed to save user {} to file: {}", req.username, e),
                    false,
                );

                return Ok(HttpResponse::InternalServerError().json(ApiResponse {
                    success: false,
                    data: None::<serde_json::Value>,
                    error: Some(format!("Error saving user to file: {}", e)),
                }));
            }

            audit_logger.log_event(
                audit::AuditEventType::UserCreated,
                "admin",
                "127.0.0.1",
                &format!("Created user {} with role {}", req.username, role),
                true,
            );

            let user_response = UserResponse {
                username: req.username.clone(),
                role,
                email: req.email.clone(),
                created_at: Some(created_at),
            };

            Ok(HttpResponse::Created().json(ApiResponse {
                success: true,
                data: Some(user_response),
                error: None,
            }))
        }
        Err(e) => {
            audit_logger.log_event(
                audit::AuditEventType::UserCreated,
                "admin",
                "127.0.0.1",
                &format!("Failed to create user {}: {}", req.username, e),
                false,
            );

            Ok(HttpResponse::InternalServerError().json(ApiResponse {
                success: false,
                data: None::<serde_json::Value>,
                error: Some(format!("Error creating user: {}", e)),
            }))
        }
    }
}

pub async fn update_user(
    path: web::Path<String>,
    req: web::Json<UpdateUserRequest>,
    _users: web::Data<Vec<auth::User>>,
    audit_logger: web::Data<audit::AuditLogger>,
) -> Result<HttpResponse> {
    let username = path.into_inner();

    // Verify admin role
    if let Err(e) = verify_admin_user(req.admin_username.as_deref()) {
        audit_logger.log_event(
            audit::AuditEventType::UnauthorizedAccess,
            req.admin_username.as_deref().unwrap_or("unknown"),
            "127.0.0.1",
            &format!("Unauthorized attempt to update user {}: {}", username, e),
            false,
        );

        return Ok(HttpResponse::Forbidden().json(ApiResponse {
            success: false,
            data: None::<serde_json::Value>,
            error: Some("Unauthorized: Admin access required".to_string()),
        }));
    }

    // Load users from file
    let users_path = config::users_file_path();
    let mut file_users = match auth::load_users(&users_path) {
        Ok(users) => users,
        Err(e) => {
            audit_logger.log_event(
                audit::AuditEventType::UserModified,
                req.admin_username.as_deref().unwrap_or("admin"),
                "127.0.0.1",
                &format!("Failed to update user {}: {}", username, e),
                false,
            );

            return Ok(HttpResponse::InternalServerError().json(ApiResponse {
                success: false,
                data: None::<serde_json::Value>,
                error: Some(format!("Error loading users: {}", e)),
            }));
        }
    };

    // Check if user exists and find their index
    let user_index = file_users.iter().position(|u| u.username == username);
    
    if user_index.is_none() {
        audit_logger.log_event(
            audit::AuditEventType::UserModified,
            "admin",
            "127.0.0.1",
            &format!("Failed to update user {}: user not found", username),
            false,
        );

        return Ok(HttpResponse::NotFound().json(ApiResponse {
            success: false,
            data: None::<serde_json::Value>,
            error: Some("User not found".to_string()),
        }));
    }

    let idx = user_index.unwrap();
    let mut changes = vec![];
    
    // Update role if provided
    if let Some(role) = &req.role {
        file_users[idx].role = role.clone();
        changes.push(format!("role={}", role));
    }
    
    // Update email if provided
    if let Some(email) = &req.email {
        file_users[idx].email = Some(email.clone());
        changes.push(format!("email={}", email));
    }

    // Save updated users to file
    if let Err(e) = auth::save_users(&file_users, &users_path) {
        audit_logger.log_event(
            audit::AuditEventType::UserModified,
            "admin",
            "127.0.0.1",
            &format!("Failed to update user {}: {}", username, e),
            false,
        );

        return Ok(HttpResponse::InternalServerError().json(ApiResponse {
            success: false,
            data: None::<serde_json::Value>,
            error: Some(format!("Error saving user: {}", e)),
        }));
    }

    audit_logger.log_event(
        audit::AuditEventType::UserModified,
        "admin",
        "127.0.0.1",
        &format!("Updated user {}: {}", username, changes.join(", ")),
        true,
    );

    let user = &file_users[idx];
    let user_response = UserResponse {
        username: user.username.clone(),
        role: user.role.clone(),
        email: user.email.clone(),
        created_at: user.created_at.clone(),
    };

    Ok(HttpResponse::Ok().json(ApiResponse {
        success: true,
        data: Some(user_response),
        error: None,
    }))
}

pub async fn delete_user(
    path: web::Path<String>,
    req: web::Json<DeleteUserRequest>,
    _users: web::Data<Vec<auth::User>>,
    audit_logger: web::Data<audit::AuditLogger>,
) -> Result<HttpResponse> {
    let username = path.into_inner();

    // Verify admin role
    if let Err(e) = verify_admin_user(req.admin_username.as_deref()) {
        audit_logger.log_event(
            audit::AuditEventType::UnauthorizedAccess,
            req.admin_username.as_deref().unwrap_or("unknown"),
            "127.0.0.1",
            &format!("Unauthorized attempt to delete user {}: {}", username, e),
            false,
        );

        return Ok(HttpResponse::Forbidden().json(ApiResponse {
            success: false,
            data: None::<serde_json::Value>,
            error: Some("Unauthorized: Admin access required".to_string()),
        }));
    }

    // Prevent deletion of admin users
    if username.to_lowercase() == "admin" {
        audit_logger.log_event(
            audit::AuditEventType::UserDeleted,
            req.admin_username.as_deref().unwrap_or("admin"),
            "127.0.0.1",
            "Failed to delete user admin: cannot delete admin user",
            false,
        );

        return Ok(HttpResponse::BadRequest().json(ApiResponse {
            success: false,
            data: None::<serde_json::Value>,
            error: Some("Cannot delete admin user".to_string()),
        }));
    }

    // Load users from file
    let users_path = config::users_file_path();
    let mut file_users = match auth::load_users(&users_path) {
        Ok(users) => users,
        Err(e) => {
            audit_logger.log_event(
                audit::AuditEventType::UserDeleted,
                req.admin_username.as_deref().unwrap_or("admin"),
                "127.0.0.1",
                &format!("Failed to delete user {}: {}", username, e),
                false,
            );

            return Ok(HttpResponse::InternalServerError().json(ApiResponse {
                success: false,
                data: None::<serde_json::Value>,
                error: Some(format!("Error loading users: {}", e)),
            }));
        }
    };

    // Find and remove the user
    let initial_len = file_users.len();
    file_users.retain(|u| u.username != username);

    if file_users.len() == initial_len {
        // User not found
        audit_logger.log_event(
            audit::AuditEventType::UserDeleted,
            "admin",
            "127.0.0.1",
            &format!("Failed to delete user {}: user not found", username),
            false,
        );

        return Ok(HttpResponse::NotFound().json(ApiResponse {
            success: false,
            data: None::<serde_json::Value>,
            error: Some("User not found".to_string()),
        }));
    }

    // Save updated users to file
    if let Err(e) = auth::save_users(&file_users, &users_path) {
        audit_logger.log_event(
            audit::AuditEventType::UserDeleted,
            "admin",
            "127.0.0.1",
            &format!("Failed to delete user {}: {}", username, e),
            false,
        );

        return Ok(HttpResponse::InternalServerError().json(ApiResponse {
            success: false,
            data: None::<serde_json::Value>,
            error: Some(format!("Error saving users: {}", e)),
        }));
    }

    audit_logger.log_event(
        audit::AuditEventType::UserDeleted,
        "admin",
        "127.0.0.1",
        &format!("Deleted user {}", username),
        true,
    );

    Ok(HttpResponse::Ok().json(ApiResponse {
        success: true,
        data: Some(serde_json::json!({"message": format!("User {} deleted", username)})),
        error: None,
    }))
}

pub async fn admin_change_password(
    path: web::Path<String>,
    req: web::Json<AdminChangePasswordRequest>,
    _users: web::Data<Vec<auth::User>>,
    audit_logger: web::Data<audit::AuditLogger>,
) -> Result<HttpResponse> {
    let username = path.into_inner();

    // Verify admin role
    if let Err(e) = verify_admin_user(req.admin_username.as_deref()) {
        audit_logger.log_event(
            audit::AuditEventType::UnauthorizedAccess,
            req.admin_username.as_deref().unwrap_or("unknown"),
            "127.0.0.1",
            &format!("Unauthorized attempt to change password for {}: {}", username, e),
            false,
        );

        return Ok(HttpResponse::Forbidden().json(ApiResponse {
            success: false,
            data: None::<serde_json::Value>,
            error: Some("Unauthorized: Admin access required".to_string()),
        }));
    }

    if username != req.username {
        return Ok(HttpResponse::BadRequest().json(ApiResponse {
            success: false,
            data: None::<serde_json::Value>,
            error: Some("Username mismatch".to_string()),
        }));
    }

    // Validate password strength
    if let Err(e) = auth::validate_password_strength(&req.new_password) {
        audit_logger.log_event(
            audit::AuditEventType::PasswordChange,
            req.admin_username.as_deref().unwrap_or("admin"),
            "127.0.0.1",
            &format!("Failed to reset password for {}: {}", username, e),
            false,
        );

        return Ok(HttpResponse::BadRequest().json(ApiResponse {
            success: false,
            data: None::<serde_json::Value>,
            error: Some(e),
        }));
    }

    // Load users from file
    let users_path = config::users_file_path();
    let mut file_users = match auth::load_users(&users_path) {
        Ok(users) => users,
        Err(e) => {
            audit_logger.log_event(
                audit::AuditEventType::PasswordChange,
                req.admin_username.as_deref().unwrap_or("admin"),
                "127.0.0.1",
                &format!("Failed to reset password for {}: {}", username, e),
                false,
            );

            return Ok(HttpResponse::InternalServerError().json(ApiResponse {
                success: false,
                data: None::<serde_json::Value>,
                error: Some(format!("Error loading users: {}", e)),
            }));
        }
    };

    match auth::hash_password(&req.new_password) {
        Ok(password_hash) => {
            // Find and update the user's password
            if let Some(user) = file_users.iter_mut().find(|u| u.username == username) {
                user.password_hash = password_hash;

                // Save updated users to file
                if let Err(e) = auth::save_users(&file_users, &users_path) {
                    audit_logger.log_event(
                        audit::AuditEventType::PasswordChange,
                        "admin",
                        "127.0.0.1",
                        &format!("Failed to reset password for {}: {}", username, e),
                        false,
                    );

                    return Ok(HttpResponse::InternalServerError().json(ApiResponse {
                        success: false,
                        data: None::<serde_json::Value>,
                        error: Some(format!("Error saving password: {}", e)),
                    }));
                }

                audit_logger.log_event(
                    audit::AuditEventType::PasswordChange,
                    "admin",
                    "127.0.0.1",
                    &format!("Admin reset password for user {}", username),
                    true,
                );

                Ok(HttpResponse::Ok().json(ApiResponse {
                    success: true,
                    data: Some(serde_json::json!({"message": "Password reset successfully"})),
                    error: None,
                }))
            } else {
                audit_logger.log_event(
                    audit::AuditEventType::PasswordChange,
                    "admin",
                    "127.0.0.1",
                    &format!("Failed to reset password for {}: user not found", username),
                    false,
                );

                Ok(HttpResponse::NotFound().json(ApiResponse {
                    success: false,
                    data: None::<serde_json::Value>,
                    error: Some("User not found".to_string()),
                }))
            }
        }
        Err(e) => {
            audit_logger.log_event(
                audit::AuditEventType::PasswordChange,
                "admin",
                "127.0.0.1",
                &format!("Failed to reset password for {}: {}", username, e),
                false,
            );

            Ok(HttpResponse::InternalServerError().json(ApiResponse {
                success: false,
                data: None::<serde_json::Value>,
                error: Some(format!("Error resetting password: {}", e)),
            }))
        }
    }
}

// Audit Log Endpoints

pub async fn get_audit_logs(
    query: web::Query<AuditLogQuery>,
    _users: web::Data<Vec<auth::User>>,
    audit_logger: web::Data<audit::AuditLogger>,
) -> Result<HttpResponse> {
    let limit = query.limit.unwrap_or(100).min(1000);

    let logs = if let Some(username) = &query.username {
        audit_logger.get_user_logs(username, limit)
    } else {
        audit_logger.get_logs(limit)
    };

    Ok(HttpResponse::Ok().json(ApiResponse {
        success: true,
        data: Some(logs),
        error: None,
    }))
}

pub fn configure(cfg: &mut web::ServiceConfig) {
    cfg.service(
        web::scope("/api")
            .route("/auth/login", web::post().to(login))
            .route("/auth/logout", web::post().to(logout))
            .route("/auth/current-user", web::get().to(get_current_user))
            .route("/auth/change-password", web::post().to(change_password))
            .route("/diagnostics/client", web::post().to(log_client_diagnostics))
            .route("/admin/users", web::get().to(list_users))
            .route("/admin/users", web::post().to(create_user))
            .route("/admin/users/{username}", web::put().to(update_user))
            .route("/admin/users/{username}", web::delete().to(delete_user))
            .route("/admin/users/{username}/password", web::post().to(admin_change_password))
            .route("/admin/audit-logs", web::get().to(get_audit_logs))
            .route("/libraries", web::get().to(get_libraries))
            .route("/books", web::get().to(get_books_multi))
            .route("/libraries/refresh", web::post().to(refresh_libraries))
            .route("/libraries/{id}", web::get().to(get_library))
            .route("/libraries/{id}/books", web::get().to(get_books))
            .route("/libraries/{id}/authors", web::get().to(get_authors))
            .route("/libraries/{id}/tags", web::get().to(get_tags))
            .route("/libraries/{id}/formats", web::get().to(get_format_stats))
            .route("/libraries/{id}/categories", web::get().to(get_categories))
            .route("/libraries/{id}/series", web::get().to(get_series))
            .route("/libraries/{id}/books/{book_id}", web::get().to(get_book))
            .route("/libraries/{id}/books/{book_id}/cover", web::get().to(get_book_cover))
            .route("/libraries/{id}/books/{book_id}/formats", web::get().to(get_book_formats))
            .route("/libraries/{id}/books/{book_id}/formats/{format}", web::get().to(get_book_file))
    );
}
