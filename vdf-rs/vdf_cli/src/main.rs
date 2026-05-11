use anyhow::{Context, Result};
use camino::Utf8PathBuf;
use clap::{Parser, Subcommand, ValueEnum};
use std::sync::Arc;
use tracing::info;
use tracing_subscriber::{EnvFilter, fmt};
use vdf_core::{
    ScanDatabase, ScanEngine, Settings,
    db::{Database, MatchMethod},
    scan::ScanProgress,
};

#[derive(Parser)]
#[command(name = "vdf-cli", version, about = "Video Duplicate Finder — command-line interface")]
struct Cli {
    #[command(subcommand)]
    command: Commands,

    /// Log verbosity (error, warn, info, debug, trace)
    #[arg(long, global = true, default_value = "info")]
    log_level: String,
}

#[derive(Subcommand)]
enum Commands {
    /// Scan directories and report duplicate videos
    Scan(ScanArgs),
    /// Show database statistics
    Stats(StatsArgs),
}

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

    /// Number of parallel hashing threads
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
    output: Option<std::path::PathBuf>,

    /// Database path
    #[arg(long, default_value = "vdf-scan.db")]
    db: std::path::PathBuf,
}

#[derive(Parser)]
struct StatsArgs {
    /// Database path
    #[arg(long, default_value = "vdf-scan.db")]
    db: std::path::PathBuf,
}

#[derive(Debug, Clone, ValueEnum)]
enum OutputFormat {
    Text,
    Json,
}

fn main() -> Result<()> {
    let cli = Cli::parse();

    // Init tracing
    let filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new(&cli.log_level));
    fmt().with_env_filter(filter).init();

    match cli.command {
        Commands::Scan(args) => cmd_scan(args),
        Commands::Stats(args) => cmd_stats(args),
    }
}

fn cmd_scan(args: ScanArgs) -> Result<()> {
    let parallelism = if args.parallelism == 0 {
        std::thread::available_parallelism().map(|n| n.get()).unwrap_or(1)
    } else {
        args.parallelism
    };

    rayon::ThreadPoolBuilder::new()
        .num_threads(parallelism)
        .build_global()
        .ok();

    let mut settings = Settings::default();
    settings.include_dirs = args.include;
    settings.exclude_dirs = args.exclude;
    settings.min_similarity = args.min_similarity;
    settings.parallelism = parallelism;
    settings.skip_start_secs = args.skip_start;
    settings.skip_end_secs = args.skip_end;
    settings.iframe_fingerprint = args.iframe_fingerprint;
    settings.iframe_sample_interval_secs = args.iframe_sample_interval;
    settings.max_iframe_samples = args.max_iframe_samples;
    settings.iframe_match_percent = args.iframe_match_percent;
    settings.iframe_min_consecutive = args.iframe_min_consecutive;
    settings.iframe_max_gap = args.iframe_max_gap;
    settings.iframe_hash_threshold = args.iframe_hash_threshold;
    settings.partial_clip_detection = args.partial_clip;

    let db = ScanDatabase::open(&args.db)
        .with_context(|| format!("opening database at {}", args.db.display()))?;

    let progress_cb: Arc<dyn Fn(ScanProgress) + Send + Sync> = Arc::new(|ev| match ev {
        ScanProgress::FileDiscovered { path } => info!("found   {path}"),
        ScanProgress::FileHashed { path, phash } => info!("hashed  {path}  [{phash:#018x}]"),
        ScanProgress::ComparisonStarted { total_pairs } =>
            info!("comparing {total_pairs} pairs…"),
        ScanProgress::DuplicateFound { file_a, file_b, similarity } =>
            info!("MATCH  {:.1}%  {file_a}  ↔  {file_b}", similarity * 100.0),
        ScanProgress::ScanComplete { files, duplicates } =>
            info!("done — {files} files, {duplicates} duplicate groups"),
        ScanProgress::Error { path, msg } =>
            tracing::warn!("error {path}: {msg}"),
    });

    let mut engine = ScanEngine::new(settings, db).with_progress(progress_cb);
    engine.run().context("scan failed")?;

    // Output results
    let duplicates = engine.db.all_duplicates()?;
    let all_files = engine.db.all_files()?;
    let file_by_id: std::collections::HashMap<_, _> =
        all_files.iter().map(|f| (f.id.as_str(), f)).collect();

    match args.format {
        OutputFormat::Text => {
            print_text_results(&duplicates, &file_by_id);
        }
        OutputFormat::Json => {
            let json = serde_json::to_string_pretty(&duplicates)?;
            if let Some(out) = args.output {
                std::fs::write(out, json)?;
            } else {
                println!("{json}");
            }
        }
    }

    engine.db.flush().context("flushing database")?;
    Ok(())
}

fn cmd_stats(args: StatsArgs) -> Result<()> {
    let db = ScanDatabase::open(&args.db)
        .with_context(|| format!("opening database at {}", args.db.display()))?;
    let files = db.all_files()?;
    let dupes = db.all_duplicates()?;
    println!("Database version : {}", db.db_version());
    println!("Scanned files    : {}", files.len());
    println!("Duplicate pairs  : {}", dupes.len());
    let size_total: u64 = files.iter().map(|f| f.size_bytes).sum();
    println!("Total size       : {:.2} GiB", size_total as f64 / (1 << 30) as f64);
    Ok(())
}

fn print_text_results(
    pairs: &[vdf_core::db::DuplicatePair],
    file_by_id: &std::collections::HashMap<&str, &vdf_core::db::FileRecord>,
) {
    if pairs.is_empty() {
        println!("No duplicates found.");
        return;
    }
    println!("\n=== {} duplicate pair(s) ===\n", pairs.len());
    for p in pairs {
        let path_a = file_by_id.get(p.file_a.as_str()).map(|f| f.path.as_str()).unwrap_or("?");
        let path_b = file_by_id.get(p.file_b.as_str()).map(|f| f.path.as_str()).unwrap_or("?");
        let method = match p.method {
            MatchMethod::FrameSimilarity => "frame",
            MatchMethod::IframeTimeline => "i-frame timeline",
            MatchMethod::AudioFingerprint => "audio",
            MatchMethod::Mpeg7Signature => "mpeg7",
            MatchMethod::SsimVerified => "ssim",
            MatchMethod::TemporalAverageHash => "temporal-avg",
        };
        let offset = p.clip_offset_secs
            .map(|s| format!("  offset {s:.1}s"))
            .unwrap_or_default();
        println!("  [{:.1}% via {method}{offset}]", p.similarity * 100.0);
        println!("    A: {path_a}");
        println!("    B: {path_b}");
        println!();
    }
}
