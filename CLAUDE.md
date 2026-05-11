# CLAUDE.md — Project Context for All Sessions

## What This Project Is

**Video Duplicate Finder (VDF)** — A media duplicate detection tool that scans directories,
fingerprints every media file (video + image + audio), and finds near-duplicates using
perceptual hashing, I-frame timeline matching, and Chromaprint audio fingerprinting.

The original C# / .NET implementation lives in the root of this repo (Avalonia GUI, Blazor web,
CLI). We are building a full Rust replacement in `vdf-rs/` that is **feature-complete**, not a
prototype. The C# code is the authoritative spec — read it before porting anything.

---

## The Rust Workspace: `vdf-rs/`

```
vdf-rs/
├── Cargo.toml      workspace root
├── vdf_core/       core library — all detection logic, DB, config
├── vdf_ui/         ONE UI codebase — Dioxus 0.7, feature flags select the target
└── vdf_cli/        headless CLI binary (clap) — no UI dependency
```

**CRITICAL — DO NOT CREATE SEPARATE FOLDERS FOR server/gui/web.**
`vdf_ui` is a single crate. Desktop, web, and mobile are **compile targets selected by feature
flags**, not separate crates or directories:

```
cargo build -p vdf_ui --features desktop   → native GPU desktop app (Wayland/Win/Mac)
cargo build -p vdf_ui --features web       → WASM + Axum server binary (browser)
cargo build -p vdf_ui --features mobile    → iOS / Android binary
```

There is no `vdf_server/`, no `vdf_gui/`, no `vdf_web/`. Those names describe compile
outputs, not source folders. The Dioxus component tree, the Axum server functions, and the
platform entry points all live inside `vdf_ui/` behind feature flags.

### Non-negotiable rules for every session

1. **Read the C# source first.** Every file in `vdf-rs/` must be a faithful port of the
   corresponding C# logic. Never simplify, never stub, never write dummy implementations.
   If a feature is complex, implement it completely or leave a `todo!()` with an explanation.

2. **SurrealDB 3.0 is the ONLY database.** It is not optional. There is no file-based fallback,
   no SQLite alternative, no flat storage. The feature flag `kv-rocksdb` is always enabled.

3. **Every implementation must be complete and functional.** No placeholder structs, no empty
   trait impls, no "simplified version" that skips the actual algorithm.

---

## Crate Stack (fixed — do not substitute without discussion)

### Core detection

| Crate | Version | Role |
|-------|---------|------|
| `ffmpeg-the-third` | 3.0.2+ffmpeg-7.1 | Video/audio decode, I-frame scan, SWR resample |
| `ffmpeg-sys-the-third` | paired | FFI layer for hwaccel (VA-API / CUDA / VideoToolbox) |
| `fast_image_resize` | latest | SIMD 32×32 grayscale resize for pHash |
| `realfft` | latest | Real-valued f64 FFT for Chromaprint chroma bins |
| `rayon` | 1.8+ | Work-stealing parallel scan (file hashing, comparison) |
| `sha2` | latest | SHA-256 file identity for byte-exact dedup |
| `ignore` | latest | Recursive directory walk (.gitignore-aware) |
| `camino` | latest | `Utf8PathBuf` — fail-fast on non-UTF-8 paths |

### Database

| Crate | Version | Role |
|-------|---------|------|
| `surrealdb` | 3.0.5 | Embedded graph DB, features = ["kv-rocksdb"] |

The `kv-rocksdb` backend is required — **never use `kv-mem` in production code** (only in tests).

### Async / error / logging

| Crate | Role |
|-------|------|
| `tokio` (current_thread) | Async runtime; bridge to sync `Database` trait via `block_on` |
| `thiserror` | Structured error enums in `vdf_core` |
| `anyhow` | Binary-level error propagation in `vdf_ui` and `vdf_cli` |
| `tracing` + `tracing-subscriber` | Structured logging; live log panel in UI via custom Layer |
| `serde` + `serde_json` | Config, JSON DB content, CLI output |

### UI — Dioxus 0.7 (single crate, multiple compile targets)

| Crate | Role |
|-------|------|
| `dioxus` 0.7 | One component tree → desktop (WGPU/Blitz), web (WASM), mobile |
| `axum` | Embedded HTTP/WS server for the `web` feature target |
| `clap` | CLI argument parsing in `vdf_cli` |

---

## Dioxus 0.7 — Architecture

### What matters for this project

Dioxus 0.7 introduced the **Blitz renderer**: WGPU + Firefox Stylo CSS engine + Taffy flexbox
+ Vello vector rendering. There is **no WebView**, no Electron overhead. Desktop binaries are
self-contained and under 6 MB. Linux desktop uses native Wayland — the primary reason we chose
Rust over staying on Avalonia 11.

The key insight: **one component tree compiles to every target**. The same `rsx!{}` components
render on GPU (desktop), in the browser (WASM), and on phone screens (mobile). Feature flags
in `vdf_ui/Cargo.toml` select which platform runtime is linked.

### `vdf_ui/Cargo.toml` feature layout

```toml
[features]
desktop = ["dioxus/desktop"]
web     = ["dioxus/web", "axum", "dioxus/axum"]
mobile  = ["dioxus/mobile"]

[dependencies]
dioxus   = { version = "0.7", default-features = false }
vdf_core = { path = "../vdf_core" }
anyhow   = { workspace = true }
tracing  = { workspace = true }
axum     = { workspace = true, optional = true }
tokio    = { workspace = true }
```

### `vdf_ui/` source layout

```
vdf_ui/
└── src/
    ├── main.rs           platform entry point — #[cfg(feature)] selects launch method
    ├── app.rs            App root component + Route enum
    ├── views/
    │   ├── scan.rs       ScanView: folder picker, progress bar, live log
    │   ├── results.rs    ResultsView: duplicate list, thumbnails, similarity badges
    │   ├── compare.rs    CompareView: side-by-side file comparison, clip offset timeline
    │   └── settings.rs   SettingsView: all Settings fields
    ├── state/
    │   ├── scan_state.rs Store<ScanState> — progress, live log entries
    │   └── app_state.rs  Store<AppState> — loaded duplicates, selected pair
    └── server/           (compiled only with `web` feature)
        └── api.rs        #[server] functions — scan trigger, DB queries via vdf_core
```

### Platform entry points (in `main.rs`)

```rust
#[cfg(feature = "desktop")]
fn main() {
    dioxus::LaunchBuilder::desktop().launch(App);
}

#[cfg(feature = "web")]
#[tokio::main]
async fn main() {
    // Axum serves the Dioxus WASM bundle + server functions
    dioxus::LaunchBuilder::fullstack().launch(App);
}

#[cfg(feature = "mobile")]
fn main() {
    dioxus::LaunchBuilder::mobile().launch(App);
}
```

### Fullstack server functions (web target only)

Real-time scan progress is pushed to the browser via Dioxus `ServerEvents<ScanProgress>`,
replacing the Blazor Server / SignalR approach from the C# codebase:

```rust
#[server]
pub async fn start_scan(settings: Settings) -> Result<(), ServerFnError> {
    // runs on the Axum server, streams ScanProgress events back to the client
    todo!()
}
```

### Stores — reactive state

Use `Store<T>` (not `Signal<T>`) for top-level app state. Stores allow components to subscribe
to only the slice of state they read, avoiding unnecessary re-renders:

```rust
#[component]
fn ScanView() -> Element {
    let mut scan_state = use_context::<Store<ScanState>>();

    rsx! {
        div { class: "scan-view",
            FolderPicker { on_select: move |path| scan_state.write().add_folder(path) }
            button {
                onclick: move |_| {
                    spawn(async move { start_scan(scan_state.read().settings.clone()).await; });
                },
                "Start Scan"
            }
            ProgressBar { value: scan_state.read().progress }
            LiveLog { entries: scan_state.read().log_entries.clone() }
        }
    }
}
```

### Results UI — driven by the graph

Because `duplicate_of` edges carry rich evidence metadata, the UI can show things flat SQL
never could:

- Match method badge (FrameSimilarity / IframeTimeline / AudioFingerprint)
- Clip offset timeline — visualise where file B appears inside file A
- "Also matched by N other files" — graph depth query, no extra joins
- Folder grouping — files sharing a `location` node highlighted together
- Duplicate clusters — graph traversal finds transitive duplicates (A≈B, B≈C → cluster ABC)

---

## SurrealDB 3.0 — Graph Schema and Usage

The database is a **graph**, not a flat table store. Shape data for traversal and analysis.
Every relationship is a first-class edge with its own fields — not a foreign key, not a
junction table.

### Namespace / Database

```surreal
USE NS vdf DB scanner;
```

Always open with `db.use_ns("vdf").use_db("scanner").await?`.

### Node tables

```surreal
-- Directory on disk
DEFINE TABLE IF NOT EXISTS location SCHEMALESS;
DEFINE FIELD IF NOT EXISTS path        ON location TYPE string;
DEFINE FIELD IF NOT EXISTS name        ON location TYPE string;
DEFINE FIELD IF NOT EXISTS scanned_at  ON location TYPE int;

-- Media file
DEFINE TABLE IF NOT EXISTS file SCHEMALESS;
DEFINE FIELD IF NOT EXISTS path               ON file TYPE string;
DEFINE FIELD IF NOT EXISTS name               ON file TYPE string;
DEFINE FIELD IF NOT EXISTS size_bytes         ON file TYPE int;
DEFINE FIELD IF NOT EXISTS media_info         ON file TYPE option<object>;
DEFINE FIELD IF NOT EXISTS phashes            ON file TYPE object;
DEFINE FIELD IF NOT EXISTS iframe_phashes     ON file TYPE array<int>;
DEFINE FIELD IF NOT EXISTS iframe_timestamps  ON file TYPE array<float>;
DEFINE FIELD IF NOT EXISTS audio_fingerprint  ON file TYPE array<int>;
DEFINE FIELD IF NOT EXISTS is_image           ON file TYPE bool;
DEFINE FIELD IF NOT EXISTS sha256             ON file TYPE option<string>;
DEFINE FIELD IF NOT EXISTS scanned_at         ON file TYPE int;
```

### Relation (RELATE) tables — edges carry full evidence

```surreal
-- file lives in a location
DEFINE TABLE IF NOT EXISTS in_folder TYPE RELATION;

-- duplicate evidence — the edge IS the full analysis record
DEFINE TABLE IF NOT EXISTS duplicate_of TYPE RELATION;
DEFINE FIELD IF NOT EXISTS similarity         ON duplicate_of TYPE float;
DEFINE FIELD IF NOT EXISTS method             ON duplicate_of TYPE string;
DEFINE FIELD IF NOT EXISTS clip_offset_secs   ON duplicate_of TYPE option<float>;
DEFINE FIELD IF NOT EXISTS phash_scores       ON duplicate_of TYPE option<array<float>>;
DEFINE FIELD IF NOT EXISTS audio_offset_secs  ON duplicate_of TYPE option<float>;
DEFINE FIELD IF NOT EXISTS consecutive_frames ON duplicate_of TYPE option<int>;
DEFINE FIELD IF NOT EXISTS best_offset_idx    ON duplicate_of TYPE option<int>;
```

The `duplicate_of` edge stores *why* two files match — not just that they do. Add fields as
new detection methods are added. The UI reads this edge data directly to render evidence panels.

### Key SurrealQL traversal patterns

```surreal
-- All duplicates of a file (graph walk, no JOIN)
SELECT ->duplicate_of->(file.*) FROM file:abc123;

-- All files in a folder
SELECT <-in_folder<-file.* FROM location:xyz789;

-- Duplicate clusters (transitive: A≈B≈C)
SELECT * FROM file WHERE ->duplicate_of->file->duplicate_of->file IS NOT NULL;

-- Highest-similarity pairs across the whole library
SELECT in.path, out.path, similarity, method
FROM duplicate_of ORDER BY similarity DESC LIMIT 50;

-- Per-folder stats: file count + how many are duplicated
SELECT location.path, count(files) AS file_count,
       count(files->duplicate_of) AS dup_count
FROM (SELECT <-in_folder<-(file.*) AS files FROM location);

-- Files with the most duplicate edges (likely originals or widely-copied)
SELECT path, count(->duplicate_of) AS times_duplicated
FROM file ORDER BY times_duplicated DESC LIMIT 20;
```

### Async bridge pattern

`SurrealDatabase` wraps `tokio::runtime::Runtime` + `Surreal<Db>` and implements the
synchronous `Database` trait. **Always extract a reference before `block_on`**:

```rust
fn some_method(&self) -> VdfResult<()> {
    let db = &self.db;          // extract reference — &Surreal<Db> is Copy
    self.rt.block_on(async move {
        db.query("...").await?;
        Ok::<_, surrealdb::Error>(())
    }).map_err(|e| VdfError::Database(e.to_string()))
}
```

### SurrealDB bind rules

- `String: SurrealValue` ✓ — pass owned `String`
- `&String: SurrealValue` ✗ — never pass a reference to `bind()`
- `&'static str: SurrealValue` ✓
- `serde_json::Value: SurrealValue` ✓ — use for complex objects
- `res.take::<Vec<FileRecord>>(0)` fails — `FileRecord` doesn't impl `SurrealValue`.
  Use: `res.take::<Vec<serde_json::Value>>(0)` then `serde_json::from_value::<FileRecord>(v)`

---

## ffmpeg-the-third 3.0.2 — API Notes

### Decoder creation (borrow pattern)

`ParametersRef` has no `clone()`. Create decoder + capture `time_base` in one scoped block
so the borrow of `ictx` ends before the packet loop:

```rust
let (mut decoder, time_base) = {
    let stream = ictx.stream(video_stream_idx).unwrap();
    let dec = codec::Context::from_parameters(stream.parameters())
        .map_err(...)?.decoder().video().map_err(...)?;
    let tb = stream.time_base();
    (dec, tb)
};
// ictx borrow released here — safe to call ictx.packets()
```

### Packet iteration

`ictx.packets()` returns `Result<(Stream, Packet), Error>` in 3.x:

```rust
for result in ictx.packets() {
    let (stream, packet) = match result { Ok(p) => p, Err(_) => continue };
    if stream.index() != target_idx { continue; }
}
```

### SwrContext::get argument order

```rust
SwrContext::get(
    decoder.format(),           // in_format: Sample
    decoder.channel_layout(),   // in_channel_layout: ChannelLayoutMask (not &ChannelLayout)
    decoder.rate(),             // in_rate: u32
    Sample::I16(Type::Packed),  // out_format: Sample
    ChannelLayoutMask::MONO,    // out_channel_layout: ChannelLayoutMask
    SAMPLE_RATE,                // out_rate: u32
)
```

### AVERROR(EAGAIN) — no unsafe needed

```rust
Err(ffmpeg_the_third::Error::Other { errno: e })
    if e == ffmpeg_the_third::ffi::AVERROR(libc::EAGAIN) => { break; }
```

---

## Chromaprint Audio Pipeline

Faithful port of `VDF.Core/Chromaprint/`. Same constants, same steps, wire-compatible output.

```
FFmpeg audio decode
  → SWR resample to 11025 Hz mono s16
  → Hann-window 4096-sample frames (hop 1365 samples ≈ 8 frames/sec)
  → Real FFT (realfft crate, f64)
  → Map FFT bins to 12 chroma bins A0=27.5 Hz to A7=3520 Hz
  → 5-tap FIR filter [0.25, 0.50, 1.00, 0.50, 0.25] / 2.50 (needs 3 frames to prime)
  → L2 normalize chroma vector
  → 32 fixed pairwise comparisons (12 adjacent + 12 minor-third + 8 tritone intervals)
  → Majority-vote over ~8 frames → one u32 per second of audio
Output: Vec<u32>, one element per second
```

---

## Scan Engine Architecture

```
Phase 1: discover_files()
  Walk include_dirs with ignore::WalkBuilder, filter by extension, skip exclude_dirs

Phase 2: hash_files() — parallel via rayon
  Per file: probe_media → extract_gray_frames → compute_phash
  Optional: iframe timestamps + phash per I-frame
  Optional: Chromaprint audio fingerprint
  → SurrealDB upsert file node + RELATE in_folder

Phase 3: compare_all() — O(n²) pairwise
  Duration pre-filter → pHash compare → I-frame timeline → audio fingerprint
  Each match → RELATE file:a -> duplicate_of -> file:b SET all evidence fields
```

---

## Settings Reference

`config::Settings` is a faithful port of `VDF.Core/Settings.cs`. Key fields:

| Field | Default | Notes |
|-------|---------|-------|
| `min_similarity` | 0.95 | pHash threshold |
| `percent_duration_difference` | 20.0 | Duration tolerance as % of longer file |
| `duration_diff_min_secs` | 0.0 | Minimum absolute tolerance |
| `duration_diff_max_secs` | 0.0 | Maximum absolute tolerance (0 = unlimited) |
| `iframe_fingerprint` | false | Enable I-frame timeline comparison |
| `partial_clip_detection` | false | Enable Chromaprint audio comparison |
| `partial_clip_min_similarity` | 0.99 | Audio fingerprint threshold |
| `include_images` | false | Also scan image extensions |
| `folder_match_mode` | None | None / SameFolderOnly / DifferentFolderOnly |
| `skip_start_secs` | 0.0 | Skip N seconds from video start |
| `skip_end_secs` | 0.0 | Skip N seconds from video end |
| `thumbnail_count` | 5 | pHash sample frames per video |

---

## Implementation Status

| Crate / File | Status | Notes |
|-------------|--------|-------|
| `vdf_core/src/error.rs` | Complete | `VdfError` enum, `VdfResult<T>` |
| `vdf_core/src/config.rs` | Complete | All Settings fields from C# |
| `vdf_core/src/phash.rs` | Complete | DCT pHash, Hamming similarity |
| `vdf_core/src/comparison.rs` | Complete | Sliding-window I-frame matching |
| `vdf_core/src/ffmpeg.rs` | Complete | probe_media, gray frames, iframe timestamps |
| `vdf_core/src/audio.rs` | Complete | Full Chromaprint pipeline |
| `vdf_core/src/db.rs` | Complete | SurrealDB 3.0 graph schema + CRUD + RELATE |
| `vdf_core/src/scan.rs` | Complete | 3-phase scan engine |
| `vdf_core/src/lib.rs` | Complete | Re-exports |
| `vdf_cli/src/main.rs` | Complete | scan / list / show commands |
| `vdf_ui/` | Not started | Dioxus 0.7 — one crate, desktop+web+mobile features |

---

## Git

Active branch: `claude/fix-intro-production-M687e`

```bash
git add <specific files>
git commit -m "..."
git push -u origin claude/fix-intro-production-M687e
```

---

## C# Source Reference

```
VDF.Core/
  Chromaprint/       Audio fingerprinting pipeline
  FFmpeg/            FFmpegEngine — frame extraction, media probe
  Models/            FileEntry.proto, Settings.cs, DuplicateItem.cs
  Utils/             TemporalHashUtils.cs (I-frame sliding window)
  PerceptualHash.cs  DCT pHash 32×32
  Scanner.cs         Main scan orchestration
VDF.GUI/             Avalonia 11 desktop (feature reference only)
VDF.Web/             Blazor Server (feature reference only)
VDF.CLI/             CLI reference implementation
```

Read the C# source end-to-end before porting any file. Do not guess at behavior.
