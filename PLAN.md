# MediaOrganizer — Full Project Plan

## What We Are Building

A **complete Rust replacement** for the C# Video Duplicate Finder (VDF) codebase. The goal is
feature parity across every C# project — not a subset, not a prototype.

The new binary is called **MediaOrganizer**. It ships as:

- A native GPU desktop app (Linux/Wayland, Windows, macOS) via Dioxus 0.7
- A browser-accessible web UI via Dioxus WASM + Axum
- An iOS/Android app via Dioxus mobile
- A headless CLI for scripting

All four outputs come from **one Rust workspace** — no code duplication.

---

## Repository Layout

```
videoduplicatefinder-next/           ← git root
├── mediaorganizer/                  ← Rust workspace
│   ├── Cargo.toml                   workspace root
│   ├── core/                        detection library (no UI, no CLI)
│   ├── cli/                         headless binary (clap)
│   └── ui/                          ONE crate — feature flags select compile target
│       └── src/
│           ├── main.rs              platform entry points (#[cfg(feature)])
│           ├── app.rs               App root + Route enum
│           ├── views/               all UI views (scan, results, compare, settings, ...)
│           ├── state/               reactive app state
│           └── server/              #[server] functions (compiled only with `server` feature)
├── VDF.Core/                        C# authoritative spec — read before porting anything
├── VDF.GUI/                         Avalonia reference — every view must be ported
├── VDF.Web/                         Blazor Server reference — every page must be ported
├── VDF.CLI/                         CLI reference
└── CLAUDE.md                        session-to-session instructions
```

### The single `ui` crate — three compile targets

```
cargo build -p ui --features desktop   →  native GPU desktop (Wayland/Win/Mac)
cargo build -p ui --features web       →  WASM client + Axum server binary
cargo build -p ui --features mobile    →  iOS / Android
```

There is no `ui_server/`, no `ui_gui/`, no `ui_web/`. Those are outputs, not folders.

The `server` feature is activated automatically by `desktop` and `mobile` (server functions run
in-process). For `web`, DX build tool handles the client/server split automatically.

---

## Phase 1 — Core Detection Library (`core/`) ✅ COMPLETE

Port every algorithm from `VDF.Core/` to Rust. No simplification. No stubs.

### 1a. Error types and config
- [x] `error.rs` — `VdfError` enum, `VdfResult<T>`
- [x] `config.rs` — full `Settings` struct, all fields from `Settings.cs`

### 1b. Perceptual hashing
- [x] `phash.rs` — DCT pHash 32×32 grayscale, Hamming similarity
  - C# ref: `VDF.Core/pHash/PerceptualHash.cs`
  - Algorithm: resize frame to 32×32, compute 8×8 DCT, binarise median, XOR Hamming distance

### 1c. FFmpeg integration
- [x] `ffmpeg.rs` — `probe_media()`, `extract_gray_frames()`, `extract_iframe_timestamps()`
  - C# ref: `VDF.Core/FFTools/FFmpegEngine.cs`
  - Crates: `ffmpeg-the-third` 3.0.2, `ffmpeg-sys-the-third`
  - Hardware accel: VA-API / CUDA / VideoToolbox via `AVHWDeviceContext` unsafe FFI
  - Decoder borrow-scope pattern: create decoder + time_base in one block, drop borrow before packet loop

### 1d. Audio fingerprinting (Chromaprint)
- [x] `audio.rs` — full Chromaprint pipeline
  - C# ref: `VDF.Core/Chromaprint/`
  - Pipeline: FFmpeg decode → SWR resample 11025 Hz mono s16 → Hann-window 4096-sample frames
    → Real FFT (realfft) → 12 chroma bins → 5-tap FIR filter → L2 normalise
    → 32 pairwise comparisons → majority-vote → `Vec<u32>` (one element per second)

### 1e. I-frame comparison (temporal sliding window)
- [x] `comparison.rs` — sliding-window I-frame timeline matching
  - C# ref: `VDF.Core/Utils/TemporalHashUtils.cs`
  - Finds clip B fully contained within file A (partial clip detection)

### 1f. Database
- [x] `db.rs` — SurrealDB 3.0 embedded graph schema
  - C# ref: `VDF.Core/DatabaseWrapper.cs`
  - Backend: `kv-rocksdb` always; never `kv-mem` in production
  - Nodes: `location` (directory), `file` (media file with all hashes)
  - Edges: `in_folder` RELATE, `duplicate_of` RELATE with full evidence fields
  - Async bridge: `let db = &self.db; self.rt.block_on(async move { db.query(...) })`

### 1g. Scan engine
- [x] `scan.rs` — 3-phase scan orchestration
  - C# ref: `VDF.Core/ScanEngine.cs`
  - Phase 1: `discover_files()` — `ignore::WalkBuilder`, extension filter, exclude dirs
  - Phase 2: `hash_files()` — parallel via Rayon, per-file probe + pHash + optional I-frame + optional audio
  - Phase 3: `compare_all()` — O(n²) pairwise, duration pre-filter, pHash → I-frame → audio

### Remaining core features (not yet started)
- [ ] `mpeg7.rs` — MPEG-7 colour layout descriptor
  - C# ref: `VDF.Core/FFTools/` (MPEG-7 signature filter via FFmpeg)
- [ ] `ssim.rs` — SSIM structural similarity verification
  - C# ref: `VDF.Core/Utils/ImageUtils.cs` (high-similarity confirmation step)
- [ ] `temporal_average.rs` — TemporalAverageHash (complement to pHash)
  - C# ref: `VDF.Core/pHash/` (average hash variant)
- [ ] Hardware accel setup helpers (`hwaccel.rs`)
  - VA-API (Linux), CUDA (NVIDIA), VideoToolbox (macOS)
  - Currently stubs in ffmpeg.rs; need full `AVHWDeviceContext` init

---

## Phase 2 — CLI (`cli/`) ✅ COMPLETE

Port `VDF.CLI/` using `clap` derive macros.

- [x] `scan` subcommand — run full scan, output progress JSON
- [x] `list` subcommand — list duplicate clusters from DB
- [x] `show` subcommand — show evidence for a specific file pair

### Remaining CLI features
- [ ] `delete` subcommand — delete lower-quality duplicates by rule
  - C# ref: `VDF.CLI/Actions/` (DeletionStrategy enum: KeepNewest, KeepOldest, KeepLargest, etc.)
- [ ] `export` subcommand — export duplicate list as JSON/CSV
- [ ] `relocate` subcommand — move files to a target directory
- [ ] `blacklist` subcommand — add pairs to permanent ignore list

---

## Phase 3 — UI (`ui/`) 🔄 IN PROGRESS

Single Dioxus 0.7 crate. All views render on desktop, web, and mobile from the same
component tree. Feature flags select the platform runtime — not the components.

### Architecture
- **State**: `Signal<ScanState>` and `Signal<AppState>` provided at root via `use_context_provider`
- **Server functions**: `#[server]` fns in `server/api.rs` — bodies compile only with `server` feature
- **Routing**: `dioxus-router` with named routes: `/`, `/results`, `/compare/:a/:b`, `/settings`, `/database`, `/blacklist`, `/expression-builder`
- **Live log**: custom `tracing::Layer` writing to a capped `VecDeque<LogEntry>` behind `RwLock`, polled each render tick
- **Duplicate clusters**: union-find over `duplicate_of` edges → transitive cluster groups

### Views already scaffolded
- [x] `views/scan.rs` — folder picker, start/stop, progress bar, live log panel
- [x] `views/results.rs` — cluster cards, similarity badge, method badge, sort/filter toolbar
- [x] `views/compare.rs` — side-by-side file cards, pHash score bars, clip offset timeline
- [x] `views/settings.rs` — all `Settings` fields as form controls, save to config JSON

### Views to add (from VDF.GUI and VDF.Web)
- [ ] `views/database.rs` — DatabaseViewer
  - C# ref: `VDF.GUI/Views/DatabaseViewer.xaml`, `ViewModels/DatabaseViewerVM.cs`
  - Browse all scanned files, view hashes, delete individual entries, re-scan single file
  - SurrealQL: `SELECT * FROM file ORDER BY name`; paginated by location
- [ ] `views/blacklist.rs` — Blacklist Manager
  - C# ref: `VDF.GUI/Views/BlacklistManagerView.xaml`, `ViewModels/BlacklistManagerVM.cs`
  - Add/remove file pairs from permanent ignore list
  - DB: `blacklist` RELATE table with `added_at` timestamp
- [ ] `views/expression_builder.rs` — Expression Builder (custom filter rules)
  - C# ref: `VDF.GUI/Views/ExpressionBuilder.xaml`, `ViewModels/ExpressionBuilderVM.cs`
  - Visual query builder for filtering duplicate results by path patterns, size ranges, date ranges
  - Generates SurrealQL WHERE clauses dynamically
- [ ] `views/quality_order.rs` — Quality Order Dialog
  - C# ref: `VDF.GUI/Views/QualityOrderDialog.xaml`, `ViewModels/QualityOrderVM.cs`
  - Define priority ordering for auto-selecting which duplicate to keep
  - Criteria: resolution, bitrate, codec, file size, modification date, path pattern
- [ ] `views/thumbnail_comparer.rs` — Full Thumbnail Comparer
  - C# ref: `VDF.GUI/Views/ThumbnailComparer.xaml`, `ViewModels/ThumbnailComparerVM.cs`
  - Enhanced compare view: scrub timeline, overlay diff mode, frame-by-frame stepping
- [ ] `views/relocate.rs` — Relocate Files Dialog
  - C# ref: `VDF.GUI/Views/RelocateFilesDialog.axaml`, `ViewModels/RelocateFilesDialogVM.cs`
  - Move selected files to a target folder (non-destructive duplicate resolution)
- [ ] `views/choose_algo.rs` — Algorithm Selection
  - C# ref: `VDF.GUI/Views/ChooseAlgoView.axaml`
  - Per-scan algorithm toggle: pHash / I-frame timeline / Chromaprint audio / MPEG-7 / SSIM
  - Links to settings panel with contextual help text for each algorithm

### State to extend
- [ ] `state/filter_state.rs` — active filters (path pattern, size range, date range, folder mode, method)
- [ ] `state/blacklist_state.rs` — in-memory blacklist cache for fast pair lookups
- [ ] `state/selection_state.rs` — selected files set for bulk actions (delete, move, mark)

### Server functions to add (`server/api.rs`)
- [ ] `get_database_entries(page, page_size)` — paginated file list from SurrealDB
- [ ] `delete_file_entry(file_id)` — remove from DB (not from disk)
- [ ] `get_blacklist()` / `add_to_blacklist(a, b)` / `remove_from_blacklist(id)`
- [ ] `rescan_file(path)` — re-hash a single file and update DB
- [ ] `delete_duplicate(path, strategy)` — delete from disk according to DeletionStrategy
- [ ] `relocate_file(path, target_dir)` — move file, update DB path
- [ ] `export_results(format)` — return JSON or CSV of duplicate list

### Web-specific features (from VDF.Web)
- [ ] Auth: `AuthService` — basic password protection for the web target
  - C# ref: `VDF.Web/Services/AuthService.cs`
  - Simple token in cookie; protect all `#[server]` endpoints
- [ ] FFmpeg setup: `FFmpegSetupService` — download/check FFmpeg binary
  - C# ref: `VDF.Web/Services/FFmpegSetupService.cs`
  - On startup, verify `ffmpeg` is on PATH; offer download link if missing
- [ ] Log streaming: SSE endpoint for live scan log
  - C# ref: `VDF.Web/Services/LogService.cs` (SignalR hub)
  - Dioxus replacement: `ServerEvents<ScanProgress>` or Axum SSE endpoint

---

## Phase 4 — Integration and Quality

- [ ] Integration tests: scan against testdata/ (small MP4s), assert pHash output matches C# reference
- [ ] Property tests: sliding-window invariants via `proptest` (A⊂B always found)
- [ ] Benchmark: Rayon parallel scan vs single-thread on 1000 files
- [ ] Binary size audit: desktop binary < 15 MB, web WASM < 5 MB
- [ ] Cross-compile: Windows x86_64 via `cargo-zigbuild`; ARM64 via `cross`

---

## Technology Decisions (locked)

| Layer | Choice | Why |
|-------|--------|-----|
| UI framework | Dioxus 0.7 | Native Wayland, no WebView, single codebase for all targets |
| Database | SurrealDB 3.0 (kv-rocksdb) | Graph traversal for cluster analysis, embedded, async Rust SDK |
| FFmpeg bindings | ffmpeg-the-third 3.0.2 | Only actively maintained Rust FFmpeg wrapper (May 2025) |
| Image resize | fast_image_resize 5 | SIMD-accelerated, no alloc for 32×32 output |
| Audio FFT | realfft | Real-valued FFT, faster than rustfft for audio |
| Parallelism | Rayon 1.8+ | Work-stealing for O(n²) comparison and file hashing |
| Paths | camino | Utf8PathBuf — fail-fast on non-UTF-8 filenames |
| CLI | clap 4 (derive) | Declarative, auto-generates --help |
| Errors | thiserror (core) + anyhow (binaries) | Structured errors in lib, ergonomic propagation in binaries |

---

## What We Are NOT Doing

- No separate `vdf_server/`, `vdf_gui/`, `vdf_web/` crates — those are compile outputs, not source folders
- No SQLite, no flat files, no optional database backend — SurrealDB kv-rocksdb is mandatory
- No simplified/dummy implementations — every port must pass the same test cases as the C# original
- No CubeCL or GPU compute kernels for now — CPU path (fast_image_resize + Rayon + POPCNT) is fast enough
- No Avalonia fixes — the C# codebase is a read-only spec reference, not a live product

---

## Active Branch

`claude/fix-intro-production-M687e`
