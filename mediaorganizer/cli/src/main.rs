//! MediaOrganizer CLI — port of VDF.CLI.
//!
//! Commands:
//!   scan          — discover + hash + compare (full pipeline)
//!   compare       — run comparison only on previously hashed files
//!   list          — list duplicate results from the DB
//!   stats         — show DB statistics
//!   db clean      — remove entries for missing/errored files
//!   db clear      — delete all DB entries
//!   mark          — select files for deletion from a duplicate JSON output
//!   blacklist add — add a group to the "not a match" blacklist
//!   blacklist list — show blacklisted groups
//!   blacklist prune — remove entries with missing paths

use anyhow::{Context, Result};
use camino::Utf8PathBuf;
use clap::{Parser, Subcommand, ValueEnum};
use serde::{Deserialize, Serialize};
use std::{
    collections::{HashMap, HashSet},
    path::PathBuf,
    sync::Arc,
};
use tracing::info;
use tracing_subscriber::{EnvFilter, fmt};
use app_core::{
    ScanDatabase, ScanEngine, Settings,
    blacklist,
    db::{Database, DuplicatePair, FileRecord, MatchMethod},
    scan::ScanProgress,
};

// ─── Top-level CLI ────────────────────────────────────────────────────────────

#[derive(Parser)]
#[command(name = "vdf", version, about = "MediaOrganizer — video duplicate finder CLI")]
struct Cli {
    #[command(subcommand)]
    command: Commands,

    /// Log verbosity (error, warn, info, debug, trace)
    #[arg(long, global = true, default_value = "info")]
    log_level: String,
}

#[derive(Subcommand)]
enum Commands {
    /// Scan directories and report duplicate videos (discover + hash + compare)
    Scan(ScanArgs),

    /// Run comparison only on files already hashed into the DB
    Compare(CompareArgs),

    /// List duplicate groups from the database
    List(ListArgs),

    /// Show database statistics
    Stats(StatsArgs),

    /// Database maintenance
    #[command(subcommand)]
    Db(DbCommand),

    /// Mark files for deletion from a JSON results file
    Mark(MarkArgs),

    /// Manage the "not a match" blacklist
    #[command(subcommand)]
    Blacklist(BlacklistCommand),
}

// ─── OutputFormat ─────────────────────────────────────────────────────────────

#[derive(Debug, Clone, ValueEnum)]
enum OutputFormat {
    Text,
    Json,
    Csv,
}

// ─── Shared DB path default ───────────────────────────────────────────────────

fn default_db() -> PathBuf {
    dirs::data_local_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("vdf")
        .join("db")
}

// ─── Scan ─────────────────────────────────────────────────────────────────────

#[derive(Parser)]
struct ScanArgs {
    /// Directories to scan (repeatable)
    #[arg(short, long = "include", required = true, value_name = "DIR")]
    include: Vec<Utf8PathBuf>,

    /// Directories to exclude
    #[arg(short, long = "exclude", value_name = "DIR")]
    exclude: Vec<Utf8PathBuf>,

    /// Minimum similarity threshold (0.0–1.0)
    #[arg(long, default_value = "0.96")]
    min_similarity: f32,

    /// Number of parallel hashing threads (0 = all CPUs)
    #[arg(long, default_value = "0")]
    parallelism: usize,

    /// Enable I-frame timeline fingerprinting
    #[arg(long)]
    iframe_fingerprint: bool,

    /// Seconds between I-frame samples
    #[arg(long, default_value = "30.0")]
    iframe_sample_interval: f64,

    /// Maximum I-frame samples per video
    #[arg(long, default_value = "300")]
    max_iframe_samples: usize,

    /// Fraction of shorter video's frames that must match
    #[arg(long, default_value = "0.40")]
    iframe_match_percent: f32,

    /// Minimum consecutive matching frames
    #[arg(long, default_value = "3")]
    iframe_min_consecutive: usize,

    /// Non-matching frames tolerated per run (0=strict)
    #[arg(long, default_value = "0")]
    iframe_max_gap: usize,

    /// Per-frame pHash similarity threshold
    #[arg(long, default_value = "0.85")]
    iframe_hash_threshold: f32,

    /// Enable audio partial-clip detection
    #[arg(long)]
    partial_clip: bool,

    /// Enable MPEG-7 signature comparison
    #[arg(long)]
    mpeg7: bool,

    /// Exclude hard-linked file pairs from comparison
    #[arg(long)]
    exclude_hard_links: bool,

    /// Also scan image files
    #[arg(long)]
    include_images: bool,

    /// Seconds to skip at video start
    #[arg(long, default_value = "0.0")]
    skip_start: f64,

    /// Seconds to skip at video end
    #[arg(long, default_value = "0.0")]
    skip_end: f64,

    /// Output format
    #[arg(long, default_value = "text")]
    format: OutputFormat,

    /// Write output to file (default: stdout)
    #[arg(long, value_name = "FILE")]
    output: Option<PathBuf>,

    /// Database path
    #[arg(long)]
    db: Option<PathBuf>,
}

// ─── Compare ─────────────────────────────────────────────────────────────────

#[derive(Parser)]
struct CompareArgs {
    /// Minimum similarity threshold (0.0–1.0)
    #[arg(long, default_value = "0.96")]
    min_similarity: f32,

    /// Enable I-frame timeline fingerprinting
    #[arg(long)]
    iframe_fingerprint: bool,

    /// Enable audio partial-clip detection
    #[arg(long)]
    partial_clip: bool,

    /// Enable MPEG-7 signature comparison
    #[arg(long)]
    mpeg7: bool,

    /// Output format
    #[arg(long, default_value = "text")]
    format: OutputFormat,

    /// Write output to file (default: stdout)
    #[arg(long, value_name = "FILE")]
    output: Option<PathBuf>,

    /// Database path
    #[arg(long)]
    db: Option<PathBuf>,
}

// ─── List ─────────────────────────────────────────────────────────────────────

#[derive(Parser)]
struct ListArgs {
    /// Minimum similarity filter (0.0–1.0)
    #[arg(long, default_value = "0.0")]
    min_similarity: f32,

    /// Filter by detection method
    #[arg(long, value_name = "METHOD")]
    method: Option<String>,

    /// Output format
    #[arg(long, default_value = "text")]
    format: OutputFormat,

    /// Database path
    #[arg(long)]
    db: Option<PathBuf>,
}

// ─── Stats ────────────────────────────────────────────────────────────────────

#[derive(Parser)]
struct StatsArgs {
    /// Database path
    #[arg(long)]
    db: Option<PathBuf>,
}

// ─── Db ───────────────────────────────────────────────────────────────────────

#[derive(Subcommand)]
enum DbCommand {
    /// Remove entries for files that no longer exist on disk or have errors
    Clean {
        /// Database path
        #[arg(long)]
        db: Option<PathBuf>,
    },
    /// Delete ALL entries from the database (requires --yes confirmation)
    Clear {
        /// Database path
        #[arg(long)]
        db: Option<PathBuf>,
        /// Skip the confirmation prompt
        #[arg(long)]
        yes: bool,
    },
}

// ─── Mark ─────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, ValueEnum)]
enum DeletionStrategy {
    /// Keep highest-quality file (default)
    LowestQuality,
    /// Keep largest file
    SmallestFile,
    /// Keep longest file
    ShortestDuration,
    /// Keep highest resolution
    WorstResolution,
    /// Only process groups where similarity = 100%
    HundredPercentOnly,
}

#[derive(Parser)]
struct MarkArgs {
    /// JSON results file produced by scan/list --format json
    #[arg(long, short, value_name = "FILE")]
    input: PathBuf,

    /// Selection strategy
    #[arg(long, default_value = "lowest-quality")]
    strategy: DeletionStrategy,

    /// Print what would be deleted without doing anything (default)
    #[arg(long, default_value = "true")]
    dry_run: bool,

    /// Move files to trash (requires trash-cli on Linux)
    #[arg(long)]
    delete: bool,

    /// Permanently delete files (irreversible!)
    #[arg(long)]
    delete_permanent: bool,
}

// ─── Blacklist ────────────────────────────────────────────────────────────────

#[derive(Subcommand)]
enum BlacklistCommand {
    /// Add a group of file paths to the blacklist
    Add {
        /// File paths to blacklist (repeatable)
        #[arg(required = true, value_name = "PATH")]
        paths: Vec<PathBuf>,
        /// Blacklist file path
        #[arg(long, default_value = "blacklist.json")]
        file: PathBuf,
    },
    /// List all blacklisted groups
    List {
        /// Blacklist file path
        #[arg(long, default_value = "blacklist.json")]
        file: PathBuf,
    },
    /// Remove entries where at least one path no longer exists on disk
    Prune {
        /// Blacklist file path
        #[arg(long, default_value = "blacklist.json")]
        file: PathBuf,
    },
}

// ─── JSON output types (for `list --format json`) ────────────────────────────

#[derive(Debug, Serialize, Deserialize)]
struct JsonPair {
    similarity: f32,
    method: String,
    clip_offset_secs: Option<f64>,
    file_a: JsonFile,
    file_b: JsonFile,
}

#[derive(Debug, Serialize, Deserialize)]
struct JsonFile {
    id: String,
    path: String,
    name: String,
    size_bytes: u64,
    duration_secs: f64,
    width: Option<u32>,
    height: Option<u32>,
}

// ─── Entry point ──────────────────────────────────────────────────────────────

fn main() -> Result<()> {
    let cli = Cli::parse();

    let filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new(&cli.log_level));
    fmt().with_env_filter(filter).init();

    match cli.command {
        Commands::Scan(args)     => cmd_scan(args),
        Commands::Compare(args)  => cmd_compare(args),
        Commands::List(args)     => cmd_list(args),
        Commands::Stats(args)    => cmd_stats(args),
        Commands::Db(sub)        => cmd_db(sub),
        Commands::Mark(args)     => cmd_mark(args),
        Commands::Blacklist(sub) => cmd_blacklist(sub),
    }
}

// ─── scan ─────────────────────────────────────────────────────────────────────

fn cmd_scan(args: ScanArgs) -> Result<()> {
    let db_path = args.db.unwrap_or_else(default_db);
    let db = ScanDatabase::open(&db_path)
        .with_context(|| format!("opening database at {}", db_path.display()))?;

    let mut settings = Settings::default();
    settings.include_dirs = args.include;
    settings.exclude_dirs = args.exclude;
    settings.min_similarity = args.min_similarity;
    if args.parallelism > 0 {
        settings.parallelism = args.parallelism;
    }
    settings.iframe_fingerprint = args.iframe_fingerprint;
    settings.iframe_sample_interval_secs = args.iframe_sample_interval;
    settings.max_iframe_samples = args.max_iframe_samples;
    settings.iframe_match_percent = args.iframe_match_percent;
    settings.iframe_min_consecutive = args.iframe_min_consecutive;
    settings.iframe_max_gap = args.iframe_max_gap;
    settings.iframe_hash_threshold = args.iframe_hash_threshold;
    settings.partial_clip_detection = args.partial_clip;
    settings.mpeg7_signature = args.mpeg7;
    settings.exclude_hard_links = args.exclude_hard_links;
    settings.include_images = args.include_images;
    settings.skip_start_secs = args.skip_start;
    settings.skip_end_secs = args.skip_end;

    let progress: Arc<dyn Fn(ScanProgress) + Send + Sync> = Arc::new(|ev| match ev {
        ScanProgress::FileDiscovered { path } => info!("found   {path}"),
        ScanProgress::FileHashed { path, phash } => info!("hashed  {path}  [{phash:#018x}]"),
        ScanProgress::ComparisonStarted { total_pairs } => {
            info!("comparing {total_pairs} pairs…")
        }
        ScanProgress::DuplicateFound { file_a, file_b, similarity } => {
            info!("MATCH  {:.1}%  {file_a}  ↔  {file_b}", similarity * 100.0)
        }
        ScanProgress::ScanComplete { files, duplicates } => {
            info!("done — {files} files, {duplicates} duplicate groups")
        }
        ScanProgress::Error { path, msg } => tracing::error!("error {path}: {msg}"),
    });

    let mut engine = ScanEngine::new(settings, db).with_progress(progress);
    engine.run().context("scan failed")?;

    let db2 = ScanDatabase::open(&db_path)?;
    let pairs = db2.all_duplicates()?;
    let files = db2.all_files()?;
    let by_id: HashMap<&str, &FileRecord> =
        files.iter().map(|f| (f.id.as_str(), f)).collect();

    emit_output(&pairs, &by_id, &args.format, args.output.as_deref())
}

// ─── compare ─────────────────────────────────────────────────────────────────

fn cmd_compare(args: CompareArgs) -> Result<()> {
    let db_path = args.db.unwrap_or_else(default_db);
    let db = ScanDatabase::open(&db_path)
        .with_context(|| format!("opening database at {}", db_path.display()))?;

    // Run only the comparison phases on already-hashed files.
    let mut settings = Settings::default();
    settings.min_similarity = args.min_similarity;
    settings.iframe_fingerprint = args.iframe_fingerprint;
    settings.partial_clip_detection = args.partial_clip;
    settings.mpeg7_signature = args.mpeg7;

    let mut engine = ScanEngine::new(settings, db);
    engine.run_compare_only().context("compare failed")?;

    let db2 = ScanDatabase::open(&db_path)?;
    let pairs = db2.all_duplicates()?;
    let files = db2.all_files()?;
    let by_id: HashMap<&str, &FileRecord> =
        files.iter().map(|f| (f.id.as_str(), f)).collect();

    emit_output(&pairs, &by_id, &args.format, args.output.as_deref())
}

// ─── list ─────────────────────────────────────────────────────────────────────

fn cmd_list(args: ListArgs) -> Result<()> {
    let db_path = args.db.unwrap_or_else(default_db);
    let db = ScanDatabase::open(&db_path)
        .with_context(|| format!("opening database at {}", db_path.display()))?;

    let mut pairs = db.all_duplicates()?;
    let files = db.all_files()?;
    let by_id: HashMap<&str, &FileRecord> =
        files.iter().map(|f| (f.id.as_str(), f)).collect();

    if args.min_similarity > 0.0 {
        pairs.retain(|p| p.similarity >= args.min_similarity);
    }
    if let Some(ref method) = args.method {
        let m = method.to_lowercase();
        pairs.retain(|p| format!("{:?}", p.method).to_lowercase().contains(&m));
    }

    emit_output(&pairs, &by_id, &args.format, None)
}

// ─── stats ────────────────────────────────────────────────────────────────────

fn cmd_stats(args: StatsArgs) -> Result<()> {
    let db_path = args.db.unwrap_or_else(default_db);
    let db = ScanDatabase::open(&db_path)
        .with_context(|| format!("opening database at {}", db_path.display()))?;

    let files = db.all_files()?;
    let dupes = db.all_duplicates()?;
    let size_total: u64 = files.iter().map(|f| f.size_bytes).sum();
    let size_dupe: u64 = dupes
        .iter()
        .filter_map(|p| {
            // Estimate wasted space: smaller of the pair
            let a = files.iter().find(|f| f.id == p.file_a).map(|f| f.size_bytes).unwrap_or(0);
            let b = files.iter().find(|f| f.id == p.file_b).map(|f| f.size_bytes).unwrap_or(0);
            if a > 0 && b > 0 { Some(a.min(b)) } else { None }
        })
        .sum();

    println!("Scanned files    : {}", files.len());
    println!("Duplicate pairs  : {}", dupes.len());
    println!(
        "Total size       : {:.2} GiB",
        size_total as f64 / (1u64 << 30) as f64
    );
    println!(
        "Estimated waste  : {:.2} GiB",
        size_dupe as f64 / (1u64 << 30) as f64
    );

    // Method breakdown
    let mut method_counts: HashMap<String, usize> = HashMap::new();
    for p in &dupes {
        *method_counts.entry(format!("{:?}", p.method)).or_default() += 1;
    }
    if !method_counts.is_empty() {
        println!("\nBy detection method:");
        let mut methods: Vec<(&String, &usize)> = method_counts.iter().collect();
        methods.sort_by(|a, b| b.1.cmp(a.1));
        for (method, count) in methods {
            println!("  {method:30} {count}");
        }
    }

    Ok(())
}

// ─── db ───────────────────────────────────────────────────────────────────────

fn cmd_db(sub: DbCommand) -> Result<()> {
    match sub {
        DbCommand::Clean { db } => {
            let db_path = db.unwrap_or_else(default_db);
            let mut conn = ScanDatabase::open(&db_path)
                .with_context(|| format!("opening database at {}", db_path.display()))?;

            let all = conn.all_files()?;
            let before = all.len();
            let mut removed = 0usize;

            for record in &all {
                let missing = !record.path.exists();
                let errored = record.flags
                    .as_ref()
                    .map(|f| f.metadata_error || f.thumbnail_error || f.scan_error.is_some())
                    .unwrap_or(false);

                if missing || errored {
                    conn.remove_file(&record.id)?;
                    removed += 1;
                }
            }

            eprintln!(
                "Cleanup complete: {} entries removed, {} remaining.",
                removed,
                before - removed
            );
            conn.flush()?;
            Ok(())
        }

        DbCommand::Clear { db, yes } => {
            let db_path = db.unwrap_or_else(default_db);
            let mut conn = ScanDatabase::open(&db_path)
                .with_context(|| format!("opening database at {}", db_path.display()))?;

            let count = conn.all_files()?.len();
            if count == 0 {
                eprintln!("Database is already empty.");
                return Ok(());
            }

            if !yes {
                eprint!(
                    "WARNING: This will permanently delete all {count} entries. Type 'yes' to confirm: "
                );
                let mut input = String::new();
                std::io::stdin().read_line(&mut input)?;
                if input.trim().to_lowercase() != "yes" {
                    eprintln!("Aborted.");
                    return Ok(());
                }
            }

            conn.clear_all()?;
            conn.flush()?;
            eprintln!("Database cleared ({count} entries removed).");
            Ok(())
        }
    }
}

// ─── mark ─────────────────────────────────────────────────────────────────────

fn cmd_mark(args: MarkArgs) -> Result<()> {
    let text = std::fs::read_to_string(&args.input)
        .with_context(|| format!("reading {}", args.input.display()))?;
    let pairs: Vec<JsonPair> = serde_json::from_str(&text)
        .context("parsing JSON results file")?;

    // Reconstruct clusters (group_id is implicit — pairs sharing a file are linked).
    // Simple approach: union-find on file paths.
    let mut parent: HashMap<String, String> = HashMap::new();
    for p in &pairs {
        let pa = p.file_a.path.clone();
        let pb = p.file_b.path.clone();
        parent.entry(pa.clone()).or_insert_with(|| pa.clone());
        parent.entry(pb.clone()).or_insert_with(|| pb.clone());
    }
    let find = |parent: &mut HashMap<String, String>, mut x: String| -> String {
        while parent.get(&x).map(|p| p != &x).unwrap_or(false) {
            let p = parent[&x].clone();
            parent.insert(x.clone(), p.clone());
            x = p;
        }
        x
    };
    for p in &pairs {
        let ra = find(&mut parent, p.file_a.path.clone());
        let rb = find(&mut parent, p.file_b.path.clone());
        if ra != rb {
            parent.insert(rb, ra);
        }
    }

    // Collect clusters: root → [file paths]
    let all_paths: Vec<String> = parent.keys().cloned().collect();
    let mut clusters: HashMap<String, HashSet<String>> = HashMap::new();
    for path in all_paths {
        let root = find(&mut parent, path.clone());
        clusters.entry(root).or_default().insert(path);
    }

    // For HundredPercentOnly, collect which clusters have any non-100% pair
    let low_sim_clusters: HashSet<String> = if matches!(args.strategy, DeletionStrategy::HundredPercentOnly) {
        pairs.iter()
            .filter(|p| p.similarity < 0.9999)
            .flat_map(|p| {
                let ra = find(&mut parent, p.file_a.path.clone());
                vec![ra]
            })
            .collect()
    } else {
        HashSet::new()
    };

    let mut to_delete: Vec<String> = Vec::new();

    for (root, members) in &clusters {
        if members.len() < 2 { continue; }
        if matches!(args.strategy, DeletionStrategy::HundredPercentOnly)
            && low_sim_clusters.contains(root)
        {
            continue;
        }

        // Build file info for the group members from the pairs list.
        let file_infos: HashMap<String, &JsonFile> = pairs.iter()
            .flat_map(|p| [(&p.file_a.path, &p.file_a), (&p.file_b.path, &p.file_b)])
            .filter(|(path, _)| members.contains(*path))
            .map(|(path, info)| (path.clone(), info))
            .collect();

        // Pick keeper based on strategy.
        let keeper = match args.strategy {
            DeletionStrategy::SmallestFile => {
                // Keep largest (delete smallest)
                file_infos.values().max_by_key(|f| f.size_bytes).map(|f| f.path.clone())
            }
            DeletionStrategy::ShortestDuration => {
                // Keep longest
                file_infos.values()
                    .max_by(|a, b| a.duration_secs.partial_cmp(&b.duration_secs).unwrap_or(std::cmp::Ordering::Equal))
                    .map(|f| f.path.clone())
            }
            DeletionStrategy::WorstResolution => {
                // Keep highest resolution
                file_infos.values()
                    .max_by_key(|f| f.width.unwrap_or(0) as u64 * f.height.unwrap_or(0) as u64)
                    .map(|f| f.path.clone())
            }
            _ => {
                // LowestQuality / HundredPercentOnly: keep largest (best proxy for quality)
                file_infos.values()
                    .max_by(|a, b| {
                        let sa = a.width.unwrap_or(0) as u64 * a.height.unwrap_or(0) as u64;
                        let sb = b.width.unwrap_or(0) as u64 * b.height.unwrap_or(0) as u64;
                        sa.cmp(&sb)
                    })
                    .map(|f| f.path.clone())
            }
        };

        for path in members {
            if Some(path) != keeper.as_ref() {
                to_delete.push(path.clone());
            }
        }
    }

    let dry_run = args.dry_run && !args.delete && !args.delete_permanent;

    if dry_run {
        println!("DRY RUN — would delete {} file(s):", to_delete.len());
        for p in &to_delete {
            println!("  {p}");
        }
        return Ok(());
    }

    for path in &to_delete {
        let p = std::path::Path::new(path);
        if args.delete_permanent {
            std::fs::remove_file(p)
                .with_context(|| format!("deleting {path}"))?;
            println!("deleted: {path}");
        } else {
            // Trash — on Linux use `trash-put` via CLI; on macOS/Windows use `trash` crate
            // For now, log the intent; a real implementation would use the `trash` crate.
            eprintln!("(trash not implemented — use --delete-permanent or install the `trash` crate)");
            eprintln!("  would trash: {path}");
        }
    }
    Ok(())
}

// ─── blacklist ────────────────────────────────────────────────────────────────

fn cmd_blacklist(sub: BlacklistCommand) -> Result<()> {
    match sub {
        BlacklistCommand::Add { paths, file } => {
            let mut bl = blacklist::load(&file);
            let entry: blacklist::BlacklistEntry = paths
                .iter()
                .map(|p| p.to_string_lossy().into_owned())
                .collect();
            bl.push(entry);
            blacklist::save(&file, &bl)
                .with_context(|| format!("saving blacklist to {}", file.display()))?;
            println!("Blacklist updated — {} groups total.", bl.len());
        }
        BlacklistCommand::List { file } => {
            let bl = blacklist::load(&file);
            if bl.is_empty() {
                println!("Blacklist is empty.");
                return Ok(());
            }
            for (i, entry) in bl.iter().enumerate() {
                println!("Group {}:", i + 1);
                let mut paths: Vec<&String> = entry.iter().collect();
                paths.sort();
                for p in paths {
                    println!("  {p}");
                }
            }
        }
        BlacklistCommand::Prune { file } => {
            let mut bl = blacklist::load(&file);
            let removed = blacklist::prune_missing(&mut bl);
            if removed > 0 {
                blacklist::save(&file, &bl)
                    .with_context(|| format!("saving blacklist to {}", file.display()))?;
                println!("Pruned {removed} entries. {} remaining.", bl.len());
            } else {
                println!("No entries to prune. {} total.", bl.len());
            }
        }
    }
    Ok(())
}

// ─── Output helpers ───────────────────────────────────────────────────────────

fn emit_output(
    pairs: &[DuplicatePair],
    by_id: &HashMap<&str, &FileRecord>,
    format: &OutputFormat,
    output_path: Option<&std::path::Path>,
) -> Result<()> {
    let text = match format {
        OutputFormat::Text => format_text(pairs, by_id),
        OutputFormat::Json => format_json(pairs, by_id)?,
        OutputFormat::Csv  => format_csv(pairs, by_id),
    };

    if let Some(path) = output_path {
        std::fs::write(path, &text)
            .with_context(|| format!("writing output to {}", path.display()))?;
        eprintln!("Results written to: {}", path.display());
    } else {
        print!("{text}");
    }
    Ok(())
}

fn format_text(pairs: &[DuplicatePair], by_id: &HashMap<&str, &FileRecord>) -> String {
    if pairs.is_empty() {
        return "No duplicates found.\n".to_string();
    }
    let mut out = format!("\n=== {} duplicate pair(s) ===\n\n", pairs.len());
    for p in pairs {
        let path_a = by_id.get(p.file_a.as_str()).map(|f| f.path.as_str()).unwrap_or("?");
        let path_b = by_id.get(p.file_b.as_str()).map(|f| f.path.as_str()).unwrap_or("?");
        let method = method_label(p.method);
        let offset = p.clip_offset_secs
            .map(|s| format!("  offset {s:.1}s"))
            .unwrap_or_default();
        out.push_str(&format!(
            "  [{:.1}% via {method}{offset}]\n    A: {path_a}\n    B: {path_b}\n\n",
            p.similarity * 100.0
        ));
    }
    out
}

fn format_json(pairs: &[DuplicatePair], by_id: &HashMap<&str, &FileRecord>) -> Result<String> {
    let records: Vec<JsonPair> = pairs
        .iter()
        .filter_map(|p| {
            let fa = by_id.get(p.file_a.as_str())?;
            let fb = by_id.get(p.file_b.as_str())?;
            Some(JsonPair {
                similarity: p.similarity,
                method: method_label(p.method).to_string(),
                clip_offset_secs: p.clip_offset_secs,
                file_a: to_json_file(fa),
                file_b: to_json_file(fb),
            })
        })
        .collect();
    serde_json::to_string_pretty(&records).context("serialising JSON output")
}

fn format_csv(pairs: &[DuplicatePair], by_id: &HashMap<&str, &FileRecord>) -> String {
    let mut out = String::from(
        "Similarity,Method,ClipOffsetSecs,PathA,SizeA,DurationA,WidthA,HeightA,\
         PathB,SizeB,DurationB,WidthB,HeightB\n",
    );
    for p in pairs {
        let fa = by_id.get(p.file_a.as_str());
        let fb = by_id.get(p.file_b.as_str());
        let path_a = fa.map(|f| f.path.as_str()).unwrap_or("");
        let path_b = fb.map(|f| f.path.as_str()).unwrap_or("");
        let size_a = fa.map(|f| f.size_bytes).unwrap_or(0);
        let size_b = fb.map(|f| f.size_bytes).unwrap_or(0);
        let dur_a  = fa.map(|f| f.duration_secs()).unwrap_or(0.0);
        let dur_b  = fb.map(|f| f.duration_secs()).unwrap_or(0.0);
        let w_a    = fa.and_then(|f| f.width()).unwrap_or(0);
        let w_b    = fb.and_then(|f| f.width()).unwrap_or(0);
        let h_a    = fa.and_then(|f| f.height()).unwrap_or(0);
        let h_b    = fb.and_then(|f| f.height()).unwrap_or(0);
        let offset = p.clip_offset_secs.map(|s| format!("{s:.3}")).unwrap_or_default();
        out.push_str(&format!(
            "{:.4},{},{},{},{},{:.3},{},{},{},{},{:.3},{},{}\n",
            p.similarity,
            method_label(p.method),
            offset,
            csv_escape(path_a),
            size_a,
            dur_a,
            w_a,
            h_a,
            csv_escape(path_b),
            size_b,
            dur_b,
            w_b,
            h_b,
        ));
    }
    out
}

fn csv_escape(value: &str) -> String {
    if value.contains(',') || value.contains('"') || value.contains('\n') {
        format!("\"{}\"", value.replace('"', "\"\""))
    } else {
        value.to_string()
    }
}

fn to_json_file(r: &FileRecord) -> JsonFile {
    JsonFile {
        id: r.id.clone(),
        path: r.path.to_string(),
        name: r.name.clone(),
        size_bytes: r.size_bytes,
        duration_secs: r.duration_secs(),
        width: r.width(),
        height: r.height(),
    }
}

fn method_label(m: MatchMethod) -> &'static str {
    match m {
        MatchMethod::FrameSimilarity    => "frame-phash",
        MatchMethod::IframeTimeline     => "i-frame-timeline",
        MatchMethod::AudioFingerprint   => "audio-chromaprint",
        MatchMethod::Mpeg7Signature     => "mpeg7-signature",
        MatchMethod::SsimVerified       => "ssim-verified",
        MatchMethod::TemporalAverageHash => "temporal-avg-hash",
    }
}
