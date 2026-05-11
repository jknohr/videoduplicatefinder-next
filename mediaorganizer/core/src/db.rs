//! SurrealDB graph database backend.
//!
//! Schema (namespace `vdf`, database `scanner`):
//!
//! ```text
//!   GRAPH
//!
//!   location (node)          ←── directory on disk
//!     ├── path: string
//!     ├── name: string
//!     └── scanned_at: datetime
//!
//!   file (node)              ←── media file on disk
//!     ├── path: string
//!     ├── name: string
//!     ├── size_bytes: int
//!     ├── media_info: object  (duration, width, height, codec, audio)
//!     ├── phashes: array      (pHash samples keyed by timestamp_ms)
//!     ├── iframe_phashes: array<int>
//!     ├── iframe_timestamps: array<float>
//!     ├── audio_fingerprint: array<int>  (one u32 per second of audio)
//!     ├── temporal_avg_gray: option<bytes>
//!     ├── sha256: option<string>
//!     ├── is_image: bool
//!     └── scanned_at: datetime
//!
//!   in_folder (RELATE file → location)
//!     └── (no extra fields)
//!
//!   duplicate_of (RELATE file → file)
//!     ├── similarity: float
//!     ├── method: string
//!     └── clip_offset_secs: option<float>
//! ```
//!
//! Queries traverse the graph naturally:
//!   `SELECT ->duplicate_of->(file.*) FROM file:xyz`  finds all duplicates of a file.
//!   `SELECT <-in_folder<-file.* FROM location:xyz`    lists all files in a folder.

use crate::error::{VdfError, VdfResult};
use crate::ffmpeg::MediaInfo;
use camino::{Utf8Path, Utf8PathBuf};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use surrealdb::engine::local::{Db, RocksDb};
use surrealdb::Surreal;
use tracing::{debug, info};

// ── Domain types ──────────────────────────────────────────────────────────────

/// Unique file identifier: first 16 hex characters of SHA-256(absolute_path).
pub type FileId = String;

/// All data stored per scanned file — maps to the `file` graph node.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileRecord {
    pub id: FileId,
    pub path: Utf8PathBuf,
    pub name: String,
    pub size_bytes: u64,
    pub media_info: Option<MediaInfo>,

    /// Standard pHash samples keyed by timestamp_ms → hash value.
    pub phashes: HashMap<u64, u64>,

    /// I-frame timeline pHash array (parallel to iframe_timestamps).
    pub iframe_phashes: Vec<u64>,

    /// I-frame sample timestamps in seconds.
    pub iframe_timestamps: Vec<f64>,

    /// Chromaprint audio fingerprint: one u32 per second of audio.
    /// Empty vec = no audio track or fingerprinting not run.
    pub audio_fingerprint: Vec<u32>,

    /// Temporal-average (tblend) 32×32 grayscale image (1024 bytes).
    pub temporal_avg_gray: Option<Vec<u8>>,

    /// SHA-256 hex digest for byte-exact deduplication.
    pub sha256: Option<String>,

    pub is_image: bool,
    pub date_created_secs: u64,
    pub date_modified_secs: u64,
    pub scanned_at: u64,
}

impl FileRecord {
    pub fn new(path: Utf8PathBuf, size_bytes: u64) -> Self {
        let name = path
            .file_name()
            .unwrap_or(path.as_str())
            .to_string();
        Self {
            id: file_id(&path),
            path,
            name,
            size_bytes,
            media_info: None,
            phashes: HashMap::new(),
            iframe_phashes: vec![],
            iframe_timestamps: vec![],
            audio_fingerprint: vec![],
            temporal_avg_gray: None,
            sha256: None,
            is_image: false,
            date_created_secs: 0,
            date_modified_secs: 0,
            scanned_at: unix_now(),
        }
    }
}

/// A matched duplicate pair — returned when querying `duplicate_of` edges.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DuplicatePair {
    pub file_a: FileId,
    pub file_b: FileId,
    pub similarity: f32,
    pub method: MatchMethod,
    /// Offset in file_a where file_b's content begins (seconds), if known.
    pub clip_offset_secs: Option<f64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MatchMethod {
    FrameSimilarity,
    IframeTimeline,
    AudioFingerprint,
    Mpeg7Signature,
    SsimVerified,
    TemporalAverageHash,
}

// ── Helpers ───────────────────────────────────────────────────────────────────

/// Extract a `file` record ID string from a SurrealDB record-ID value.
/// SurrealDB returns record IDs in the form `"file:⟨id⟩"` or as an object.
fn extract_record_id(v: &serde_json::Value) -> FileId {
    match v {
        serde_json::Value::String(s) => {
            s.split(':').nth(1).unwrap_or(s.as_str()).to_string()
        }
        serde_json::Value::Object(o) => o
            .get("id")
            .and_then(|id| id.as_str())
            .and_then(|s| s.split(':').nth(1))
            .unwrap_or_default()
            .to_string(),
        _ => String::new(),
    }
}

/// Parse a raw JSON row from `SELECT ... FROM duplicate_of` into a `DuplicatePair`.
fn parse_duplicate_row(row: serde_json::Value) -> Option<DuplicatePair> {
    let file_a = extract_record_id(row.get("in")?);
    let file_b = extract_record_id(row.get("out")?);
    let similarity = row.get("similarity")?.as_f64()? as f32;
    let method_str = row.get("method")?.as_str().unwrap_or("");
    let method = match method_str {
        "FrameSimilarity" => MatchMethod::FrameSimilarity,
        "IframeTimeline" => MatchMethod::IframeTimeline,
        "AudioFingerprint" => MatchMethod::AudioFingerprint,
        "Mpeg7Signature" => MatchMethod::Mpeg7Signature,
        "SsimVerified" => MatchMethod::SsimVerified,
        "TemporalAverageHash" => MatchMethod::TemporalAverageHash,
        _ => MatchMethod::FrameSimilarity,
    };
    let clip_offset_secs = row.get("clip_offset_secs").and_then(|v| v.as_f64());
    Some(DuplicatePair { file_a, file_b, similarity, method, clip_offset_secs })
}

// ── Database trait ─────────────────────────────────────────────────────────────

/// Synchronous interface over the SurrealDB async connection.
///
/// All methods block until the underlying async operation completes.
pub trait Database: Send + Sync {
    fn upsert_file(&mut self, record: FileRecord) -> VdfResult<()>;
    fn get_file(&self, id: &str) -> VdfResult<Option<FileRecord>>;
    fn get_file_by_path(&self, path: &Utf8Path) -> VdfResult<Option<FileRecord>>;
    fn all_files(&self) -> VdfResult<Vec<FileRecord>>;
    fn add_duplicate(&mut self, pair: DuplicatePair) -> VdfResult<()>;
    fn all_duplicates(&self) -> VdfResult<Vec<DuplicatePair>>;
    fn clear_duplicates(&mut self) -> VdfResult<()>;
    fn flush(&mut self) -> VdfResult<()>;
    fn db_version(&self) -> u32;
}

// ── SurrealDatabase ───────────────────────────────────────────────────────────

const DB_SCHEMA_VERSION: u32 = 1;
const NS: &str = "vdf";
const DB_NAME: &str = "scanner";

/// SurrealDB schema DDL — defines namespace, database, graph tables and edges.
const SCHEMA_DDL: &str = "
    -- Graph node: directory on disk
    DEFINE TABLE IF NOT EXISTS location SCHEMALESS;
    DEFINE FIELD IF NOT EXISTS path       ON location TYPE string;
    DEFINE FIELD IF NOT EXISTS name       ON location TYPE string;
    DEFINE FIELD IF NOT EXISTS scanned_at ON location TYPE int;

    -- Graph node: media file on disk
    DEFINE TABLE IF NOT EXISTS file SCHEMALESS;
    DEFINE FIELD IF NOT EXISTS path             ON file TYPE string;
    DEFINE FIELD IF NOT EXISTS name             ON file TYPE string;
    DEFINE FIELD IF NOT EXISTS size_bytes       ON file TYPE int;
    DEFINE FIELD IF NOT EXISTS is_image         ON file TYPE bool;
    DEFINE FIELD IF NOT EXISTS sha256           ON file TYPE option<string>;
    DEFINE FIELD IF NOT EXISTS date_created_secs ON file TYPE int;
    DEFINE FIELD IF NOT EXISTS date_modified_secs ON file TYPE int;
    DEFINE FIELD IF NOT EXISTS scanned_at       ON file TYPE int;

    -- Graph edge: file → location (file lives in this directory)
    DEFINE TABLE IF NOT EXISTS in_folder TYPE RELATION;

    -- Graph edge: file → file (duplicate relationship with metadata)
    DEFINE TABLE IF NOT EXISTS duplicate_of TYPE RELATION;
    DEFINE FIELD IF NOT EXISTS similarity       ON duplicate_of TYPE float;
    DEFINE FIELD IF NOT EXISTS method           ON duplicate_of TYPE string;
    DEFINE FIELD IF NOT EXISTS clip_offset_secs ON duplicate_of TYPE option<float>;

    -- Version record
    DEFINE TABLE IF NOT EXISTS meta SCHEMALESS;
";

/// Embedded SurrealDB with RocksDB storage.
///
/// Graph layout: `location` nodes connected to `file` nodes via `in_folder`
/// edges.  Duplicate pairs are `duplicate_of` graph edges between `file` nodes,
/// carrying similarity and method metadata.
///
/// Async calls are bridged to the synchronous `Database` trait via a dedicated
/// single-threaded Tokio runtime.
pub struct SurrealDatabase {
    rt: tokio::runtime::Runtime,
    db: Surreal<Db>,
}

impl SurrealDatabase {
    /// Open (or create) the SurrealDB graph database at `path`.
    pub fn open(path: impl AsRef<std::path::Path>) -> VdfResult<Self> {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|e| VdfError::Database(e.to_string()))?;

        let db_path = path.as_ref().to_path_buf();
        let db = rt
            .block_on(async move {
                // Open RocksDB-backed embedded store
                let db: Surreal<Db> = Surreal::new::<RocksDb>(db_path).await?;

                // Namespace → database → schema
                db.use_ns(NS).use_db(DB_NAME).await?;
                db.query(SCHEMA_DDL).await?;

                // Upsert schema version record
                db.upsert::<Option<serde_json::Value>>(("meta", "version"))
                    .content(serde_json::json!({"version": DB_SCHEMA_VERSION}))
                    .await?;

                Ok::<Surreal<Db>, surrealdb::Error>(db)
            })
            .map_err(|e: surrealdb::Error| VdfError::Database(e.to_string()))?;

        info!("opened SurrealDB (NS={NS} DB={DB_NAME}) at {}", path.as_ref().display());
        Ok(Self { rt, db })
    }

    /// Ensure a `location` node exists for `folder_path` and return its ID.
    fn ensure_location(&self, folder_path: &str) -> VdfResult<String> {
        let loc_id = folder_id(folder_path);
        let name = std::path::Path::new(folder_path)
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or(folder_path)
            .to_string();
        let path_str = folder_path.to_string();
        let loc_id2 = loc_id.clone();
        let now = unix_now();

        let db = &self.db;
        self.rt
            .block_on(async move {
                db.query("UPSERT type::thing('location', $id) SET path = $path, name = $name, scanned_at = $now")
                    .bind(("id", loc_id2))
                    .bind(("path", path_str))
                    .bind(("name", name))
                    .bind(("now", now))
                    .await
                    .map(|_| ())
            })
            .map_err(|e| VdfError::Database(e.to_string()))?;

        Ok(loc_id)
    }
}

impl Database for SurrealDatabase {
    fn upsert_file(&mut self, record: FileRecord) -> VdfResult<()> {
        let folder = record.path.parent().map(|p| p.as_str()).unwrap_or("").to_string();
        let loc_id = self.ensure_location(&folder)?;
        let file_id_str = record.id.clone();

        // Serialize to JSON value so SurrealDB can deserialize via serde
        let json_val = serde_json::to_value(&record)
            .map_err(|e| VdfError::Database(e.to_string()))?;

        let db = &self.db;
        self.rt
            .block_on(async move {
                // Upsert the file node using SurrealQL with raw JSON content
                db.query("UPSERT type::thing('file', $id) CONTENT $data")
                    .bind(("id", file_id_str.clone()))
                    .bind(("data", json_val))
                    .await?;

                // Graph edge: file → location
                db.query(
                    "RELATE type::thing('file', $fid) -> in_folder -> \
                     type::thing('location', $lid)",
                )
                .bind(("fid", file_id_str))
                .bind(("lid", loc_id))
                .await?;

                Ok::<_, surrealdb::Error>(())
            })
            .map_err(|e| VdfError::Database(e.to_string()))?;

        debug!("upserted file:{}", record.id);
        Ok(())
    }

    fn get_file(&self, id: &str) -> VdfResult<Option<FileRecord>> {
        let id = id.to_string();
        let db = &self.db;
        let raw: Vec<serde_json::Value> = self
            .rt
            .block_on(async move {
                let mut res = db
                    .query("SELECT * FROM type::thing('file', $id)")
                    .bind(("id", id))
                    .await?;
                res.take::<Vec<serde_json::Value>>(0)
            })
            .map_err(|e| VdfError::Database(e.to_string()))?;

        Ok(raw
            .into_iter()
            .next()
            .and_then(|v| serde_json::from_value::<FileRecord>(v).ok()))
    }

    fn get_file_by_path(&self, path: &Utf8Path) -> VdfResult<Option<FileRecord>> {
        self.get_file(&file_id(path.as_str()))
    }

    fn all_files(&self) -> VdfResult<Vec<FileRecord>> {
        let db = &self.db;
        let raw: Vec<serde_json::Value> = self
            .rt
            .block_on(async {
                let mut res = db.query("SELECT * FROM file").await?;
                res.take::<Vec<serde_json::Value>>(0)
            })
            .map_err(|e| VdfError::Database(e.to_string()))?;

        raw.into_iter()
            .map(|v| {
                serde_json::from_value::<FileRecord>(v)
                    .map_err(|e| VdfError::Database(e.to_string()))
            })
            .collect()
    }

    fn add_duplicate(&mut self, pair: DuplicatePair) -> VdfResult<()> {
        let fa = pair.file_a.clone();
        let fb = pair.file_b.clone();
        let sim = pair.similarity;
        let method = format!("{:?}", pair.method);
        let offset = pair.clip_offset_secs;

        let db = &self.db;
        self.rt
            .block_on(async move {
                db.query(
                    "RELATE type::thing('file', $a) -> duplicate_of -> \
                     type::thing('file', $b) \
                     SET similarity = $sim, method = $method, clip_offset_secs = $offset",
                )
                .bind(("a", fa))
                .bind(("b", fb))
                .bind(("sim", sim))
                .bind(("method", method))
                .bind(("offset", offset))
                .await?;
                Ok::<_, surrealdb::Error>(())
            })
            .map_err(|e| VdfError::Database(e.to_string()))?;

        debug!("added duplicate_of: {} → {}", pair.file_a, pair.file_b);
        Ok(())
    }

    fn all_duplicates(&self) -> VdfResult<Vec<DuplicatePair>> {
        let db = &self.db;
        let rows: Vec<serde_json::Value> = self
            .rt
            .block_on(async {
                let mut res = db
                    .query("SELECT in, out, similarity, method, clip_offset_secs FROM duplicate_of")
                    .await?;
                res.take::<Vec<serde_json::Value>>(0)
            })
            .map_err(|e| VdfError::Database(e.to_string()))?;

        let pairs = rows.into_iter().filter_map(parse_duplicate_row).collect();
        Ok(pairs)
    }

    fn clear_duplicates(&mut self) -> VdfResult<()> {
        let db = &self.db;
        self.rt
            .block_on(async {
                db.query("DELETE duplicate_of").await?;
                Ok::<_, surrealdb::Error>(())
            })
            .map_err(|e| VdfError::Database(e.to_string()))?;
        debug!("cleared all duplicate_of edges");
        Ok(())
    }

    fn flush(&mut self) -> VdfResult<()> {
        // SurrealDB with RocksDB writes are immediately durable.
        debug!("SurrealDB flush: no-op (writes are durable immediately)");
        Ok(())
    }

    fn db_version(&self) -> u32 {
        DB_SCHEMA_VERSION
    }
}

/// `ScanDatabase` is always `SurrealDatabase`.
pub type ScanDatabase = SurrealDatabase;

// ── Helpers ───────────────────────────────────────────────────────────────────

/// Stable 16-char hex ID for a file path (first 16 chars of SHA-256).
pub fn file_id(path: impl AsRef<str>) -> FileId {
    let hash = Sha256::digest(path.as_ref().as_bytes());
    format!("{:x}", hash)[..16].to_string()
}

/// Stable 16-char hex ID for a directory path.
fn folder_id(path: &str) -> String {
    file_id(path)
}

fn unix_now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

// ── Tests (use SurrealDB in-memory backend) ───────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use surrealdb::engine::local::Mem;

    async fn mem_db() -> Surreal<Db> {
        let db: Surreal<Db> = Surreal::new::<Mem>(()).await.unwrap();
        db.use_ns(NS).use_db(DB_NAME).await.unwrap();
        db.query(SCHEMA_DDL).await.unwrap();
        db
    }

    fn make_surreal_db_mem() -> SurrealDatabase {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let db = rt.block_on(mem_db());
        SurrealDatabase { rt, db }
    }

    #[test]
    fn insert_and_retrieve_file() {
        let mut db = make_surreal_db_mem();
        let rec = FileRecord::new(Utf8PathBuf::from("/test/video.mp4"), 1024);
        let id = rec.id.clone();
        db.upsert_file(rec).unwrap();
        let got = db.get_file(&id).unwrap().unwrap();
        assert_eq!(got.path.as_str(), "/test/video.mp4");
    }

    #[test]
    fn all_files_returns_inserted() {
        let mut db = make_surreal_db_mem();
        db.upsert_file(FileRecord::new("/a.mp4".into(), 100)).unwrap();
        db.upsert_file(FileRecord::new("/b.mp4".into(), 200)).unwrap();
        assert_eq!(db.all_files().unwrap().len(), 2);
    }

    #[test]
    fn duplicate_of_graph_edge() {
        let mut db = make_surreal_db_mem();
        let fa = FileRecord::new("/a.mp4".into(), 100);
        let fb = FileRecord::new("/b.mp4".into(), 200);
        let id_a = fa.id.clone();
        let id_b = fb.id.clone();
        db.upsert_file(fa).unwrap();
        db.upsert_file(fb).unwrap();

        db.add_duplicate(DuplicatePair {
            file_a: id_a,
            file_b: id_b,
            similarity: 0.99,
            method: MatchMethod::FrameSimilarity,
            clip_offset_secs: None,
        })
        .unwrap();

        let pairs = db.all_duplicates().unwrap();
        assert_eq!(pairs.len(), 1);
        assert!((pairs[0].similarity - 0.99).abs() < 1e-4);
    }

    #[test]
    fn clear_duplicates_removes_all_edges() {
        let mut db = make_surreal_db_mem();
        db.upsert_file(FileRecord::new("/x.mp4".into(), 1)).unwrap();
        db.upsert_file(FileRecord::new("/y.mp4".into(), 2)).unwrap();
        db.add_duplicate(DuplicatePair {
            file_a: file_id("/x.mp4"),
            file_b: file_id("/y.mp4"),
            similarity: 0.95,
            method: MatchMethod::IframeTimeline,
            clip_offset_secs: Some(30.0),
        })
        .unwrap();
        assert_eq!(db.all_duplicates().unwrap().len(), 1);
        db.clear_duplicates().unwrap();
        assert_eq!(db.all_duplicates().unwrap().len(), 0);
    }
}
