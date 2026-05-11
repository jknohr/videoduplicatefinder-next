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
├── Cargo.toml            workspace root
├── vdf_core/             core library — all detection logic, DB, config
├── vdf_cli/              CLI binary (clap)
├── vdf_server/           (pending) Axum + Dioxus web backend
└── vdf_gui/              (pending) Dioxus 0.7 desktop binary
```

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
| `anyhow` | Binary-level error propagation (CLI, server, GUI) |
| `tracing` + `tracing-subscriber` | Structured logging; in-app live log panel via custom Layer |
| `serde` + `serde_json` | Config, JSON DB content, CLI output |

### UI (target Dioxus 0.7)

| Crate | Role |
|-------|------|
| `dioxus` 0.7 | Desktop + Web + Mobile from one component tree |
| `axum` | HTTP/WS backend for fullstack/web target |
| `clap` | CLI argument parsing |

---

## SurrealDB 3.0 — Graph Schema and Usage

The database is a **graph**, not a flat table store. Shape data for traversal and analysis.

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

### Relation (RELATE) tables — NOT flat join tables

```surreal
-- file lives in a location
DEFINE TABLE IF NOT EXISTS in_folder TYPE RELATION;

-- duplicate evidence — the edge IS the analysis record
DEFINE TABLE IF NOT EXISTS duplicate_of TYPE RELATION;
DEFINE FIELD IF NOT EXISTS similarity         ON duplicate_of TYPE float;
DEFINE FIELD IF NOT EXISTS method             ON duplicate_of TYPE string;
DEFINE FIELD IF NOT EXISTS clip_offset_secs   ON duplicate_of TYPE option<float>;
-- extend with richer evidence as needed:
DEFINE FIELD IF NOT EXISTS phash_scores       ON duplicate_of TYPE option<array<float>>;
DEFINE FIELD IF NOT EXISTS audio_offset_secs  ON duplicate_of TYPE option<float>;
DEFINE FIELD IF NOT EXISTS consecutive_frames ON duplicate_of TYPE option<int>;
DEFINE FIELD IF NOT EXISTS best_offset_idx    ON duplicate_of TYPE option<int>;
```

### Key SurrealQL traversal patterns

```surreal
-- All duplicates of a file (graph walk, not JOIN)
SELECT ->duplicate_of->(file.*) FROM file:abc123;

-- All files in a folder
SELECT <-in_folder<-file.* FROM location:xyz789;

-- Duplicate clusters (files with multiple duplicate_of edges)
SELECT *, count(->duplicate_of) AS dup_count FROM file WHERE count(->duplicate_of) > 0;

-- Highest-similarity duplicate pairs
SELECT in.path, out.path, similarity, method
FROM duplicate_of
ORDER BY similarity DESC LIMIT 50;

-- Files grouped by folder with duplicate count
SELECT location.path, count(files) AS file_count,
       count(files->duplicate_of) AS dup_count
FROM (SELECT <-in_folder<-(file.*) AS files FROM location);
```

### Async bridge pattern

`SurrealDatabase` wraps `tokio::runtime::Runtime` + `Surreal<Db>` and implements the
synchronous `Database` trait. **Always extract a reference before `block_on`**:

```rust
fn some_method(&self) -> VdfResult<()> {
    let db = &self.db;          // borrow before block_on
    self.rt.block_on(async move {
        db.query("...").await?;
        Ok::<_, surrealdb::Error>(())
    }).map_err(|e| VdfError::Database(e.to_string()))
}
```

`async move` captures `db: &Surreal<Db>` — references are `Copy`, so no conflict with
`self.rt.block_on()` borrowing `&self.rt`.

### SurrealDB bind rules

- `String: SurrealValue` ✓ — pass owned `String`
- `&String: SurrealValue` ✗ — never pass a reference to `bind()`
- `&'static str: SurrealValue` ✓
- `serde_json::Value: SurrealValue` ✓ — use for complex objects
- `res.take::<Vec<FileRecord>>(0)` fails — `FileRecord` doesn't impl `SurrealValue`.
  Instead: `res.take::<Vec<serde_json::Value>>(0)` then `serde_json::from_value::<FileRecord>(v)`

---

## ffmpeg-the-third 3.0.2 — API Notes

This is an active fork of `ffmpeg-next`. All examples online for `ffmpeg-next` translate
directly.

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
    let (stream, packet) = match result {
        Ok(p) => p,
        Err(_) => continue,
    };
    if stream.index() != target_idx { continue; }
    // ...
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

### AVERROR(EAGAIN) match

`AVERROR` is a `const fn` in ffmpeg-the-third 3.x — no `unsafe` block needed:

```rust
Err(ffmpeg_the_third::Error::Other { errno: e })
    if e == ffmpeg_the_third::ffi::AVERROR(libc::EAGAIN) => { break; }
```

---

## Chromaprint Audio Pipeline

Faithful port of `VDF.Core/Chromaprint/`. Match the C# exactly — same constants, same
algorithm steps, same 32-bit fingerprint format (wire-compatible with AcoustID).

```
FFmpeg audio decode
  → SWR resample to 11025 Hz mono s16
  → Hann-window 4096-sample frames (hop 1365 samples ≈ 8 frames/sec)
  → Real FFT (realfft crate)
  → Map FFT bins to 12 chroma bins A0=27.5 Hz to A7=3520 Hz
  → 5-tap FIR filter [0.25, 0.50, 1.00, 0.50, 0.25] / 2.50 (needs 3 frames to prime)
  → L2 normalize chroma vector
  → 32 fixed pairwise comparisons (12 adjacent + 12 minor-third + 8 tritone intervals)
  → Majority-vote over ~8 frames → one u32 per second of audio
Output: Vec<u32>, one element per second
```

---

## Dioxus 0.7 — UI Layer

Target Dioxus **0.7** for all UI work. Key facts for this project:

### What changed in 0.7

- **Native GPU renderer (Blitz)**: Built on WGPU + Firefox Stylo CSS engine + Taffy flexbox +
  Vello vector rendering. **No WebView**, no Electron-style overhead. Self-contained desktop
  binaries under 6 MB. Full native Wayland on Linux (the main reason we chose Rust over
  staying on Avalonia 11).

- **One component tree → all targets**: Write components once, compile to:
  ```
  cargo build --features desktop   → native GPU app (Windows, macOS, Linux/Wayland)
  cargo build --features web        → WASM + static assets (browser)
  cargo build --features mobile     → iOS / Android
  ```
  The CLI binary (`vdf_cli`) has no Dioxus dependency at all.

- **Fullstack with Axum**: The web target uses Axum as the HTTP/WS server.
  Server functions use the `#[server]` macro. Real-time scan progress uses
  `ServerEvents<ScanProgress>` (server-sent events) or `Websocket` for bidirectional
  communication. This replaces the Blazor Server / SignalR approach from the C# codebase.

- **Stores**: New primitive for nested reactive state with fine-grained reactivity.
  Use `Store<T>` instead of `Signal<T>` for the scan state, duplicate list, and settings —
  lets components subscribe to only the slice they need.

- **Hot-patching**: During development, `dx serve` patches running Rust code at runtime
  without losing app state. Press `d` to attach the LLDB debugger.

- **Signals + `use_resource`**: Async data fetching hooks work the same as 0.6 but
  integrate with the new fullstack streaming APIs.

### Workspace layout for UI

```
vdf_server/   →  axum + dioxus web (fullstack binary)
vdf_gui/      →  dioxus desktop binary (imports vdf_core, spawns Tokio for DB calls)
```

Both binaries share the same Dioxus component library. Use a `vdf_ui` crate (or module)
for shared components compiled into both targets.

### UI architecture for VDF

```
vdf_gui/
└── src/
    ├── main.rs          LaunchBuilder::new().with_cfg(desktop config).launch(App)
    ├── app.rs           App root: Route enum, sidebar nav
    ├── views/
    │   ├── scan.rs      ScanView: folder picker, progress bar, live log stream
    │   ├── results.rs   ResultsView: duplicate list with thumbnails, similarity badges
    │   ├── compare.rs   CompareView: side-by-side file comparison
    │   └── settings.rs  SettingsView: all Settings fields
    └── state/
        ├── scan_state.rs   Store<ScanState> — current scan progress, live log
        └── app_state.rs    Store<AppState> — loaded duplicates, selected pair
```

### Dioxus component skeleton (0.7 style)

```rust
use dioxus::prelude::*;

#[component]
fn ScanView() -> Element {
    let mut scan_state = use_context::<Store<ScanState>>();

    rsx! {
        div { class: "scan-view",
            FolderPicker { on_select: move |path| scan_state.write().add_folder(path) }
            button {
                onclick: move |_| {
                    spawn(async move {
                        // call server function or direct vdf_core via spawn_blocking
                        start_scan(scan_state.read().settings.clone()).await;
                    });
                },
                "Start Scan"
            }
            ProgressBar { value: scan_state.read().progress }
            LiveLog { entries: scan_state.read().log_entries.clone() }
        }
    }
}
```

### Results view — graph-informed UI

Because `duplicate_of` edges carry rich metadata, the results UI can show:

- Match method badge (FrameSimilarity / IframeTimeline / AudioFingerprint)
- Similarity percentage
- Clip offset (partial clip detection: file B starts at X seconds into file A)
- Traversal: "Also duplicated by N other files" (graph depth query)
- Folder grouping: files in the same `location` node highlighted together

---

## Scan Engine Architecture

```
Phase 1: discover_files()
  - Walk include_dirs with ignore::WalkBuilder
  - Filter by VIDEO_EXTENSIONS + IMAGE_EXTENSIONS
  - Skip exclude_dirs
  - Emit ScanProgress::FileDiscovered per file

Phase 2: hash_files() — parallel via rayon
  - Per file: ffmpeg::probe_media → ffmpeg::extract_gray_frames → compute_phash
  - Optional: ffmpeg::extract_iframe_timestamps + phash per I-frame
  - Optional: audio::compute_fingerprint (Chromaprint)
  - Serialize FileRecord to JSON → SurrealDB upsert + in_folder RELATE

Phase 3: compare_all() — O(n²) pairwise
  - Duration pre-filter (Settings::duration_tolerance_secs)
  - FolderMatchMode filter (SameFolderOnly / DifferentFolderOnly / None)
  - Standard pHash (compare_phash) → MatchMethod::FrameSimilarity
  - I-frame timeline (compare_iframe_timeline + sliding_window_compare) → IframeTimeline
  - Audio fingerprint (compare_audio + fingerprint_similarity) → AudioFingerprint
  - Each match → RELATE file:a -> duplicate_of -> file:b SET similarity, method, ...
```

---

## Settings Reference

`config::Settings` is a faithful port of `VDF.Core/Settings.cs`. Key fields:

| Field | Default | Notes |
|-------|---------|-------|
| `min_similarity` | 0.95 | Threshold for frame pHash match |
| `percent_duration_difference` | 20.0 | Duration tolerance as % of longer file |
| `duration_diff_min_secs` | 0.0 | Minimum absolute tolerance |
| `duration_diff_max_secs` | 0.0 | Maximum absolute tolerance |
| `iframe_fingerprint` | false | Enable I-frame timeline comparison |
| `partial_clip_detection` | false | Enable Chromaprint audio comparison |
| `partial_clip_min_similarity` | 0.99 | Audio fingerprint threshold |
| `include_images` | false | Also scan image extensions |
| `folder_match_mode` | None | None / SameFolderOnly / DifferentFolderOnly |
| `skip_start_secs` | 0.0 | Skip this many seconds from video start |
| `skip_end_secs` | 0.0 | Skip this many seconds from video end |
| `thumbnail_count` | 5 | Number of pHash sample frames per video |

`duration_tolerance_secs(longer_dur)` implements the C# `GetDurationToleranceSeconds`:
- If `percent_duration_difference > 0`: `tol = longer_dur * pct / 100`, clamped by min/max
- Otherwise: `max(duration_diff_min_secs, duration_diff_max_secs)`

---

## What Has Been Implemented

| File | Status | Notes |
|------|--------|-------|
| `vdf_core/src/error.rs` | Complete | `VdfError` enum, `VdfResult<T>` |
| `vdf_core/src/config.rs` | Complete | All Settings fields from C# source |
| `vdf_core/src/phash.rs` | Complete | DCT pHash, Hamming similarity |
| `vdf_core/src/comparison.rs` | Complete | Sliding-window I-frame timeline matching |
| `vdf_core/src/ffmpeg.rs` | Complete | probe_media, extract_gray_frames, iframe timestamps |
| `vdf_core/src/audio.rs` | Complete | Full Chromaprint pipeline |
| `vdf_core/src/db.rs` | Complete | SurrealDB 3.0 graph schema + all CRUD + RELATE |
| `vdf_core/src/scan.rs` | Complete | 3-phase scan engine |
| `vdf_core/src/lib.rs` | Complete | Re-exports |
| `vdf_cli/src/main.rs` | Complete | Full CLI with scan/list/show commands |
| `vdf_server/` | Not started | Axum + Dioxus web |
| `vdf_gui/` | Not started | Dioxus 0.7 desktop |

---

## Git

Active branch: `claude/fix-intro-production-M687e`

All work must be committed to this branch. Push after each meaningful change.

```bash
git add <specific files>     # never git add -A (could catch .env, large binaries)
git commit -m "..."
git push -u origin claude/fix-intro-production-M687e
```

---

## C# Source Reference

The original C# implementation is the authoritative spec:

```
VDF.Core/
  Chromaprint/          Audio fingerprinting pipeline
  FFmpeg/               FFmpegEngine — frame extraction, media probe
  Models/               FileEntry.proto, Settings.cs, DuplicateItem.cs
  Utils/                TemporalHashUtils.cs (I-frame sliding window)
  PerceptualHash.cs     DCT pHash 32×32
  Scanner.cs            Main scan orchestration (equivalent of scan.rs)
  SurrealDB/            (not present in C# — new for Rust)
VDF.GUI/                Avalonia 11 desktop (reference for feature set only)
VDF.Web/                Blazor Server (reference for web feature set)
VDF.CLI/                CLI reference implementation
```

When porting any file, read the C# source end-to-end first. Do not guess at behavior.
