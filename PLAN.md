# MediaOrganizer — Full Project Plan

---

## Vision

**MediaOrganizer is a complete media management platform** — not just a duplicate finder.

The end goal covers every operation you would ever want to perform on a media library:

| Domain | Capabilities |
|--------|-------------|
| **Duplicate detection** | Video, audio, image — perceptual hash, I-frame timeline, audio fingerprint, MPEG-7 |
| **Re-encoding & compression** | All formats, GPU-accelerated, HandBrake-equivalent quality presets |
| **Cropping & trimming** | Non-destructive cuts, chapter-aware trim, batch operations |
| **Metadata enrichment** | Read/write container tags; auto-enrich from MusicBrainz, TMDB, TVDB, MusicBrainz Picard-style matching |
| **Format conversion** | Video → any codec/container, audio → any format, image → any format |
| **Library organisation** | Rename by template, move by rule, folder structure enforcement |
| **Analysis & insight** | Graph-based cluster analysis, codec health, bitrate distribution, storage savings estimates |

**Right now:** port the existing C# VDF to Rust with full feature parity.
**The architecture is being built to support everything above from day one** — the SurrealDB graph
schema, the `file` node structure, and the `core` library are all designed to handle video,
audio, and images uniformly, not retrofitted later.

---

## Current Phase: Rust Port

A **complete Rust replacement** for the C# Video Duplicate Finder (VDF) codebase. The goal is
feature parity across every C# project — not a subset, not a prototype. Every feature listed
in the README must ship. Nothing is removed or deferred.

The new binary is called **MediaOrganizer**. It ships as:

- A native GPU desktop app (Linux/Wayland, Windows, macOS) via Dioxus 0.7
- A browser-accessible web UI via Dioxus WASM + Axum (target: WASM64 when ecosystem ready)
- An iOS/Android app via Dioxus mobile
- A headless CLI for scripting and automation

All four outputs come from **one Rust workspace** — no code duplication.

---

## Architectural decisions that serve the vision

These are locked-in now precisely because of where the app is going:

**SurrealDB graph schema is media-type agnostic.** The `file` node stores `is_image: bool` today.
It will gain `is_audio: bool`, `media_type: string` (video/audio/image/document), and codec/format
metadata as the platform expands. Every graph traversal pattern works identically across types.

**`core/` is a library, not a CLI wrapper.** Re-encoding, trimming, and enrichment will be
additional modules in `core/` — same crate, same error types, same DB access pattern.

**`ui/` has no hardcoded media type assumptions.** Views are parameterised over the data they
receive from server functions, not wired to video-specific types.

**FFmpeg is the universal engine.** Every media operation (decode, encode, filter, remux,
metadata) goes through FFmpeg. The `ffmpeg-the-third` binding and the `std::process::Command`
fallback both remain — the former for hot paths (hashing, decoding), the latter for complex
filter graphs (SSIM, MPEG-7, re-encode with quality presets).

---

## Repository Layout

```
videoduplicatefinder-next/           ← git root
├── mediaorganizer/                  ← Rust workspace
│   ├── Cargo.toml                   workspace root
│   ├── .cargo/config.toml           target overrides (WASM64 ready, currently commented)
│   ├── core/                        detection library (no UI, no CLI)
│   ├── cli/                         headless binary (clap)
│   └── ui/                          ONE crate — feature flags select compile target
│       └── src/
│           ├── main.rs              platform entry points (#[cfg(feature)])
│           ├── app.rs               App root + Route enum
│           ├── views/               all UI views
│           ├── state/               reactive app state
│           └── server/              #[server] functions (server feature only)
├── rust-toolchain.toml              nightly pin (required for WASM64 build-std)
├── VDF.Core/                        C# authoritative spec — read before porting anything
├── VDF.GUI/                         Avalonia reference — every view must be ported
├── VDF.Web/                         Blazor Server reference — every page must be ported
├── VDF.CLI/                         CLI reference
└── CLAUDE.md                        session-to-session instructions
```

### Compile targets from the single `ui` crate

```
cargo build -p ui --features desktop   →  native GPU desktop (Wayland/Win/Mac)
cargo build -p ui --features web       →  WASM32 client + Axum server binary
cargo build -p ui --features wasm64    →  WASM64 client + Axum server (no 4 GB limit)
cargo build -p ui --features mobile    →  iOS / Android
```

There is no `ui_server/`, `ui_gui/`, `ui_web/`. Those are outputs, not folders.

---

## Phase 1 — Core Detection Library (`core/`) ✅ COMPLETE

Port every algorithm from `VDF.Core/`. No simplification. No stubs.

### 1a. Error types and config ✅ COMPLETE
- [x] `error.rs` — `VdfError` enum, `VdfResult<T>`
- [x] `config.rs` — all Settings fields including: skip_start/end_percent, scene_aware_skip,
  scene_detection_threshold, scene_skip_count, iframe_sample_interval_secs, max_iframe_samples,
  iframe_match_percent, iframe_min_consecutive, iframe_max_gap, iframe_hash_threshold,
  temporal_avg_hash, temporal_avg_start/window_secs, mpeg7_signature, ssim_verification,
  ssim_verify_min/max_sim, ssim_reject_threshold, ssim_window_secs, hardware_accel

### 1b. Perceptual hashing ✅ COMPLETE
- [x] `phash.rs` — DCT pHash 32×32 grayscale, Hamming similarity
- C# ref: `VDF.Core/pHash/PerceptualHash.cs`

### 1c. FFmpeg integration ✅ COMPLETE
- [x] `ffmpeg.rs` — `probe_media()`, `extract_gray_frames()`, `extract_iframe_timestamps()`
- [x] `get_scene_change_timestamps()` — FFmpeg scdet filter, Vec<f64> timestamps
- [x] `extract_temporal_average_hash()` — FFmpeg tblend, single pHash of blended frame
- [x] `compute_ssim_at_offset()` — FFmpeg ssim filter at matched offset
- [x] `read_metadata_tags()` — ffprobe JSON tag reader (mirrors FFProbeEngine.GetMetadataTags)
- [x] `write_metadata_tags()` — ffmpeg -c copy atomic rewrite
- [x] `which_ffmpeg()` / `which_ffprobe()` — binary discovery
- C# ref: `VDF.Core/FFTools/FFmpegEngine.cs`, `FFProbeEngine.cs`

**Hardware accel:** handled via `Settings::hardware_accel` field passed to FFmpeg; the
13 `HardwareAccel` variants map to ffmpeg `-hwaccel` flag values. Low-level `AVHWDeviceContext`
init is not needed — ffmpeg-the-third handles it through the standard hwaccel API.

### 1d. Audio fingerprinting (Chromaprint) ✅ COMPLETE
- [x] `audio.rs` — full Chromaprint pipeline, Vec<u32> output
- C# ref: `VDF.Core/Chromaprint/`

### 1e. I-frame comparison ✅ COMPLETE
- [x] `comparison.rs` — sliding-window I-frame timeline matching
- [x] Gap-tolerant sliding window honouring `iframe_max_gap`
- [x] Per-frame threshold using `iframe_hash_threshold`
- [x] Returns `consecutive_frames` count and `best_offset_idx` in match result

### 1f. Temporal average hash ✅ COMPLETE
- [x] `extract_temporal_average_hash()` in ffmpeg.rs
- [x] Pre-filter wired into `scan_for_timeline_duplicates()` in scan.rs
- [x] `set_temporal_avg_hash()` / `temporal_avg_hash()` on FileRecord

### 1g. Database ✅ COMPLETE
- [x] `db.rs` — SurrealDB 3.0 graph schema + CRUD + RELATE
- [x] `blacklisted` RELATE table with `added_at`, `reason`
- [x] `meta` table with `db_version` singleton
- [x] Migration system — reads stored version, runs incremental migrations, updates version
- [x] `set_temporal_avg_hash()` / `set_mpeg7_sig_path()` setters on FileRecord
- Schema additions (added via migration v1→v2): scene_change_timestamps, mpeg7_signature_path,
  temporal_avg_hash, is_flipped on duplicate_of

### 1h. Scan engine ✅ COMPLETE (including flipped detection + rescan)
- [x] `scan.rs` — 3-phase scan engine
- [x] Phase 2: scene-aware skip via `get_scene_change_timestamps()`
- [x] Phase 2: temporal average hash via `extract_temporal_average_hash()`
- [x] Phase 2: MPEG-7 signature extraction via `mpeg7::extract_signature()`
- [x] Phase 3: temporal avg hash pre-filter before I-frame sliding window (Hamming > 25 → skip)
- [x] Phase 3: MPEG-7 compare via `scan_for_mpeg7_duplicates()`
- [x] Phase 3: SSIM second-pass for borderline matches (`ssim_verify` in scan.rs)
- [x] Blacklist filter — `pair_is_blacklisted()` guards all four `add_duplicate()` call sites

**Completed:**
- [x] Phase 3: flipped-image detection — `compare_videos_bucketed()` pre-computes `flipped_phash_hashes()` and re-compares when normal match fails; stored as `is_flipped: bool` on edge
- [x] Rescan single file — `ScanEngine::rescan_file()` in `scan.rs`; exposed as `#[server] rescan_file(path)` in `server/api.rs`; Rescan button wired in `database.rs` view

### 1i. Metadata module ✅ COMPLETE
- [x] `read_metadata_tags(path)` in `ffmpeg.rs` — ffprobe JSON tag reader
- [x] `write_metadata_tags(path, tags)` in `ffmpeg.rs` — ffmpeg -c copy atomic rewrite
- [x] Exposed in `lib.rs`, wired in `server/api.rs` (read_tags / write_tags server functions)
- [x] `MetadataEditorInline` component in results.rs — inline tag editor per file row

### 1j. MPEG-7 module ✅ COMPLETE
- [x] `mpeg7.rs` — `sig_folder()`, `extract_signature()`, `compare_signatures()`
  - Wraps FFmpeg video signature filter via `std::process::Command`
  - `extract_signature(path, ffmpeg_path, extended_logging) -> Option<PathBuf>`
  - `compare_signatures(sig_a, sig_b, ffmpeg_path) -> Option<(f64, f64)>` (returns similarity + clip offset)
  - Wired into `scan.rs` Phase 3 via `scan_for_mpeg7_duplicates()`

### 1k. SSIM — ✅ COMPLETE (in `ffmpeg.rs`)
- [x] `compute_ssim_at_offset(path_a, path_b, offset_secs, window_secs, ffmpeg_path) -> f32`
  - Wraps `ffmpeg -lavfi [0][1]ssim=stats_file=-` via `std::process::Command`
  - Returns parsed `All:` SSIM value; -1.0 on failure
  - Wired into `scan.rs` Phase 3 SSIM second-pass verification

---

## Phase 2 — CLI (`cli/`) ✅ MOSTLY COMPLETE

Port `VDF.CLI/` using `clap` derive macros.

### Subcommands implemented
- [x] `scan` — run scan, output progress, full settings flags (cmd_scan)
- [x] `list` — list duplicate clusters from DB (text/json/csv output) (cmd_list)
- [x] `compare` — show evidence for a specific file pair (cmd_compare)
- [x] `mark` — trash/delete files by ID (cmd_mark)
- [x] `relocate` — move files to target directory, update DB paths, name deconfliction (cmd_relocate)
- [x] `rescan <path>` — re-hash a single file, update DB, re-run comparisons (cmd_rescan)
- [x] `blacklist add/remove/list` — manage blacklisted pairs (cmd_blacklist)
- [x] `stats` — show library statistics (cmd_stats)
- [x] `db` — database subcommands: list-files, remove-file, clear (cmd_db)

### Subcommands completed this session
- [x] `delete` — auto-delete duplicates from DB by strategy (cmd_delete)
  - Strategies: `lowest-quality`, `smallest-file`, `shortest-duration`, `worst-resolution`, `100-percent-only`
  - Flags: `--dry-run` (default), `--delete` (XDG trash via trash-put/gio/kioclient fallback chain), `--delete-permanent`
  - Filters: `--min-similarity`, `--method`
- [x] `export` — export to file (cmd_export); `list` now also has `--output` flag
- [x] `list` — updated with `--output <FILE>` option

### All CLI flags to implement (full list from README)

| Flag | Type | Description |
|------|------|-------------|
| `--include <path>` | repeatable | Directory to scan |
| `--exclude <path>` | repeatable | Directory to exclude |
| `--threshold <n>` | u32 | Hash difference threshold (default 5) |
| `--percent <n>` | f64 | Minimum similarity % to report (default 96) |
| `--parallelism <n>` | usize | Parallel hashing threads |
| `--include-images` | bool | Also scan image files |
| `--use-phash` | bool | Enable perceptual hashing |
| `--native-ffmpeg` | bool | Use native FFmpeg bindings (vs process spawn fallback) |
| `--skip-start-seconds <n>` | f64 | Seconds to skip at start |
| `--skip-start-percent <n>` | f64 | % of duration to skip at start (max with seconds) |
| `--skip-end-seconds <n>` | f64 | Seconds to skip at end |
| `--skip-end-percent <n>` | f64 | % of duration to skip at end |
| `--scene-aware-skip` | bool | Auto-detect intro end via scdet |
| `--scene-detection-threshold <n>` | f64 | scdet sensitivity 0–100 |
| `--scene-skip-count <n>` | u32 | Scene transitions to skip at start |
| `--iframe-fingerprint` | bool | Enable I-frame timeline fingerprinting |
| `--iframe-sample-interval <n>` | f64 | Seconds between I-frame samples |
| `--max-iframe-samples <n>` | u32 | Cap on I-frame samples per video |
| `--iframe-match-percent <n>` | f64 | Required match fraction 0.0–1.0 |
| `--iframe-min-consecutive <n>` | u32 | Minimum consecutive matching frames |
| `--iframe-max-gap <n>` | u32 | Non-matching frames tolerated inside a run |
| `--iframe-hash-threshold <n>` | f64 | Per-frame pHash similarity threshold |
| `--temporal-avg-hash` | bool | Enable temporal average hash rejection filter |
| `--temporal-avg-start-sec <n>` | f64 | Start of averaging window |
| `--temporal-avg-window-sec <n>` | f64 | Duration of averaging window |
| `--mpeg7-signature` | bool | Enable MPEG-7 video signature comparison |
| `--ssim-verification` | bool | Enable SSIM second-pass for borderline matches |
| `--ssim-verify-min-sim <n>` | f64 | Lower bound of grey zone |
| `--ssim-verify-max-sim <n>` | f64 | Upper bound of grey zone |
| `--ssim-reject-threshold <n>` | f64 | SSIM score below this = hard reject |
| `--ssim-window-seconds <n>` | f64 | Duration compared at matched offset |
| `--partial-clip-detection` | bool | Enable audio fingerprint partial clip detection |
| `--partial-clip-min-ratio <n>` | f64 | Min clip/source duration ratio 0.0–1.0 |
| `--partial-clip-similarity <n>` | f64 | Min audio fingerprint similarity 0.0–1.0 |
| `--action <strategy>` | enum | Auto-delete strategy (see below) |
| `--dry-run` | bool | Show what would be deleted, make no changes |
| `--delete` | bool | Move to trash |
| `--delete-permanent` | bool | Delete from disk permanently |
| `--format json\|text\|csv` | enum | Output format |
| `--output <file>` | path | Write results to file instead of stdout |
| `--settings <file>` | path | Load full settings from JSON file |

---

## Phase 3 — UI (`ui/`) ✅ MOSTLY COMPLETE

Single Dioxus 0.7 crate. All views render on desktop, web, and mobile from the same component
tree. Feature flags select the platform runtime — not the components.

### Architecture (Dioxus 0.7 fullstack)
- **Reactive state**: `Signal<ScanState>` and `Signal<AppState>` via `use_context_provider` at root
- **Server functions**: `#[server]` fns in `server/api.rs` — bodies compile only with `server` feature; client gets HTTP stub
- **Routing**: `dioxus-router` with `#[component]` routes matching enum variants
- **Live log**: custom `tracing::Layer` → capped `VecDeque<LogEntry>` behind `RwLock`; polled each render tick
- **Duplicate clusters**: union-find over `duplicate_of` edges → transitive groups
- **Video streaming**: Axum route serving byte ranges for browser `<video>` element

### Routes
```
/                       → ScanView
/results                → ResultsView
/compare/:a/:b          → CompareView (side-by-side)
/settings               → SettingsView
/database               → DatabaseView
/blacklist              → BlacklistView
/expression-builder     → ExpressionBuilderView
/quality-order          → QualityOrderView
/logs                   → LogsView
```

### Views already built
- [x] `views/scan.rs` — folder picker, start/stop/pause controls, progress bar, live log panel
- [x] `views/results.rs` — cluster cards, file actions, sort/filter, search, auto-select,
  blacklist group, move-to-folder inline, metadata editor inline (⋮ button per file),
  QualityOrderPanel (reorderable criteria), CustomSelectionPanel (8 filter criteria + presets),
  SurrealSelectionPanel (SurrealQL WHERE clause editor + presets), thumbnail strip per file
- [x] `views/compare.rs` — side-by-side file cards with evidence display, ThumbnailStrip (5 frames)
- [x] `views/settings.rs` — all settings fields: similarity, fingerprinting, scan scope,
  MPEG-7, SSIM, hardware acceleration, skip start/end (seconds + %)
- [x] `views/stats.rs` — group count, dup storage, reclaimable space, method breakdown
- [x] `views/blacklist.rs` — list blacklisted pairs, un-mark, clear, prune missing
- [x] `views/database.rs` — paginated sortable file browser, db entry removal
- [x] `views/logs.rs` — live log panel with level filter, auto-scroll, clear
- [x] `views/relocate.rs` — two-mode file relocator (prefix replace + filesystem rescan with size/mtime/duration matching)

### Views — what still needs to be built or completed

#### `views/results.rs` — remaining additions
- [ ] **Detection badges** per duplicate group header:
  - `I-frame timeline` (blue) — I-frame sliding window match
  - `MPEG-7` (purple) — MPEG-7 signature match
  - `Audio fingerprint` (green) — partial clip via Chromaprint
  - `Frame similarity` (orange) — standard pHash
  - `Flipped` (red) — horizontally mirrored content
  - Data source: `method` field on `duplicate_of` edge
- [ ] **Match explanation line** per duplicate pair:
  - *"I-frame timeline · clip found at 1:23:45 in source · 67% match"*
  - *"Frame similarity · 94% match"*
  - Data source: `clip_offset_secs`, `consecutive_frames`, `similarity` on `duplicate_of` edge

#### `views/compare.rs` — additions
- [ ] Full thumbnail scrub timeline (frame-by-frame stepping with arrow keys)
- [ ] Overlay diff mode — show pixel difference image between corresponding frames

### State

- [x] `state/scan_state.rs` — ScanState: progress, log entries, pause/stop flags
- [x] `state/app_state.rs` — AppState: clusters, selected pair, sort, method_filter,
  selected_for_action, criteria_order

### Server functions (`server/api.rs`) — implementation status

| Function | Status | Description |
|----------|--------|-------------|
| `trigger_scan(settings)` | ✅ | Trigger scan via ScanState |
| `cancel_scan()` | ✅ | Cancel in-progress scan |
| `set_scan_paused(paused)` | ✅ | Pause/resume scan |
| `read_tags(path)` | ✅ | Read container metadata via ffprobe |
| `write_tags(path, tags)` | ✅ | Write container metadata via ffmpeg -c copy |
| `load_duplicates()` | ✅ | All duplicate clusters from DB |
| `delete_file(file_id, from_disk)` | ✅ | Remove from DB; optionally trash from disk |
| `remove_duplicate_pair(a, b)` | ✅ | Remove duplicate_of edge |
| `video_stream_handler` | ✅ | Axum range-request video endpoint (HTTP 206) |
| `thumbnail_handler` | ✅ | FFmpeg frame extraction to JPEG at position |
| `get_blacklist()` | ✅ | All blacklisted pairs |
| `add_to_blacklist(a, b, reason)` | ✅ | Add pair to blacklist |
| `remove_from_blacklist(a, b)` | ✅ | Remove blacklist entry |
| `rescan_file(path)` | ✅ | Re-hash single file, update DB |
| `get_ffmpeg_status()` | ✅ | FFmpeg binary status check |
| `export_results(format)` | ❌ | JSON / CSV export (available in CLI via `export` subcommand) |

### Web-only features (`#[cfg(feature = "web")]`)

#### HTTP range request video endpoint ✅ COMPLETE
- [x] Axum route `GET /api/video` with `Range:` header support (HTTP 206 Partial Content)
- [x] Security: paths validated (path traversal rejected)

#### Authentication ✅ COMPLETE
- [x] `server/auth.rs` — password state, token store, cookie middleware
- [x] Axum middleware `auth_middleware` — checks `vdf_auth` cookie on all routes
- [x] `/login` GET → HTML login form; `/auth/login` POST → validate + set cookie
- [x] On first launch: generate random 10-char alphanumeric password, print to stdout
- [x] Cookie: HttpOnly, SameSite=Strict, 30-day Max-Age
- [x] Env: `VDF_WEB_PASSWORD` override, `VDF_WEB_AUTH=false` to disable
- [x] API routes (`/api/*`) return 401 JSON; browser routes → 302 /login

#### FFmpeg setup service ✅ COMPLETE
- [x] `server/ffmpeg_setup.rs` — `check_ffmpeg()` called at startup in `register_axum_routes()`
- [x] `FfmpegStatus` enum: Ready / MissingFfprobe / MissingFfmpeg / Missing
- [x] `FfmpegBanner` component in scan.rs — shows inline warning if FFmpeg not found
- [x] Platform-specific install instructions per OS

---

## Phase 4 — Docker ✅ MOSTLY COMPLETE

- [x] `Dockerfile` — multi-stage build (rust:1.87-bookworm builder + debian:bookworm-slim runtime)
  - FFmpeg dev headers in builder; FFmpeg runtime + VA-API drivers in final image
  - Builds both `mediaorganizer-ui` and `mediaorganizer-cli` binaries
  - EXPOSE 8080, ENV VDF_DB_PATH, VDF_CONFIG_PATH
- [x] `docker-compose.yml`
  - Default port 8080
  - Named volumes: mediaorganizer-db (/data), mediaorganizer-config (/config)
  - VA-API device passthrough (commented, opt-in: `devices: /dev/dri`)
  - NVIDIA GPU deploy block (commented, opt-in)
  - LIBVA_DRIVER_NAME env var examples
- [ ] Multi-arch build: `linux/amd64` + `linux/arm64` (Raspberry Pi / NAS)
- [ ] GitHub Actions workflow: build + push to GHCR on every commit to main

---

## Phase 5 — Integration and Quality

- [ ] Integration tests: scan testdata/ (small MP4s), assert pHash output matches C# reference vectors
- [ ] Property tests: sliding-window invariants via `proptest`
  - If A ⊂ B (content), sliding window always finds the match
  - Gap tolerance: inserting N frames never breaks a run if gap ≤ iframe_max_gap
- [ ] Scene-aware skip regression: known intro-heavy test files; assert skip offset is correct
- [ ] Chromaprint regression: same audio fingerprint output as C# AcoustID.NET pipeline
- [ ] SSIM regression: known borderline pairs at known offsets; assert accept/reject decisions
- [ ] MPEG-7 regression: signature files from C# VDF; assert detectmode=full returns correct offset
- [ ] Binary size audit: desktop binary < 15 MB, web WASM < 5 MB
- [ ] Cross-compile: Windows x86_64 via `cargo-zigbuild`; ARM64 via `cross`

---

## Technology Decisions (locked)

| Layer | Choice | Why |
|-------|--------|-----|
| UI framework | Dioxus 0.7 | Native Wayland, no WebView, one codebase for all targets |
| Database | SurrealDB 3.0 (kv-rocksdb) | Graph traversal for cluster analysis, embedded, async |
| FFmpeg bindings | ffmpeg-the-third 3.0.2 | Only actively maintained Rust FFmpeg wrapper |
| Image resize | fast_image_resize 5 | SIMD-accelerated |
| Audio FFT | realfft | Real-valued FFT for Chromaprint |
| Parallelism | Rayon 1.8+ | Work-stealing for O(n²) comparison |
| Paths | camino | Utf8PathBuf — fail-fast on non-UTF-8 filenames |
| CLI | clap 4 (derive) | Declarative, auto-generates --help |
| Errors | thiserror (core) + anyhow (binaries) | Structured in lib, ergonomic in binaries |
| WASM target | wasm32 now → wasm64 when ecosystem ready | No 4 GB limit by design; .cargo/config.toml prepared |

---

---

## Future Phases (after port is complete)

These phases are planned but not started. Architecture decisions made during the port are
chosen specifically to avoid blocking any of these.

### Phase 6 — Audio duplicate detection

Extend the scan engine and UI to treat audio files as first-class citizens, not just
an optional scan target alongside video.

- [ ] Audio-specific pHash: spectral fingerprint for music (not Chromaprint — Chromaprint is
  for clip matching; this is for perceptually similar music with different mastering/encoding)
- [ ] BPM / key detection as additional comparison dimension
- [ ] Waveform visualisation in compare view
- [ ] Audio-specific result card: duration bar shows waveform amplitude envelope
- [ ] Support all common audio formats: MP3, FLAC, AAC, OGG, WAV, AIFF, M4A, OPUS

### Phase 7 — Image duplicate detection

Extend to standalone image files (beyond thumbnails extracted from video).

- [ ] Full image pHash pipeline (already partially there via `is_image` flag)
- [ ] EXIF / XMP metadata read and write
- [ ] GPS location clustering — find images from the same location
- [ ] Face detection grouping (optional, requires ML model — TBD)
- [ ] RAW format support (CR2, NEF, ARW, DNG) via FFmpeg or rawler crate
- [ ] Image-specific compare view: side-by-side with zoom, pixel diff overlay

### Phase 8 — Re-encoding and compression

HandBrake-equivalent functionality for all media types, GPU-accelerated.

**Video:**
- [ ] Codec presets: H.264, H.265/HEVC, AV1, VP9 — with quality (CRF) and bitrate modes
- [ ] Hardware encode: VA-API (Intel/AMD), NVENC (NVIDIA), VideoToolbox (macOS)
- [ ] Resolution downscale with aspect ratio preservation
- [ ] Deinterlace, denoise, deblock filters
- [ ] Chapter-aware batch encode: re-encode only selected chapters
- [ ] Quality preview: encode 30-second sample before committing to full encode

**Audio:**
- [ ] Codec presets: AAC, MP3, FLAC, OPUS, AC3 — with quality and bitrate modes
- [ ] Sample rate and channel conversion (stereo downmix, surround upmix)
- [ ] Normalisation: EBU R128 loudness normalisation, peak normalisation
- [ ] Batch re-encode: apply preset to entire library or selected files

**Image:**
- [ ] Format conversion with quality control: JPEG, WebP, AVIF, PNG, HEIC
- [ ] Batch resize with aspect ratio preservation and multiple output sizes
- [ ] Lossless optimisation: oxipng / mozjpeg equivalent via FFmpeg or dedicated crates

**Shared:**
- [ ] Encode queue with Rayon-parallel execution (N simultaneous encodes, configurable)
- [ ] Storage savings estimate before encode: show projected size reduction
- [ ] Encode history in SurrealDB: input path, output path, settings, duration, saved bytes
- [ ] Non-destructive: always write to new file, never overwrite original unless explicitly confirmed

### Phase 9 — Cropping and trimming

Non-destructive edit operations stored as instructions in SurrealDB, applied on export.

- [ ] Video trim: set in/out points, export as new file via `ffmpeg -ss -to -c copy`
- [ ] Chapter split: split a long video at chapter boundaries into separate files
- [ ] Image crop: define crop rectangle, export via FFmpeg `crop` filter
- [ ] Batch trim: apply same trim rule to all files matching a pattern
- [ ] Preview trim without encoding: seek to in/out points in the browser player

### Phase 10 — Metadata enrichment

Auto-identify media and populate container tags from online databases.

**Video:**
- [ ] TMDB integration — match movie/TV episode by title + year, populate title/description/genre/poster
- [ ] TVDB integration — TV series episode matching by show name + season + episode number
- [ ] NFO file generation (Kodi/Jellyfin/Plex compatible)
- [ ] Artwork download and embedding (cover art, episode thumbnails)

**Audio:**
- [ ] MusicBrainz AcoustID matching — identify songs by audio fingerprint, no filename required
- [ ] MusicBrainz Picard-style tag population: artist, album, track, year, genre, ISRC
- [ ] Album art embedding and scaling
- [ ] ReplayGain calculation and embedding

**Shared:**
- [ ] Enrichment queue: scan library → match candidates → show confirmation UI → write on approve
- [ ] Confidence scoring: show match confidence, let user confirm or reject each suggestion
- [ ] Bulk approve/reject for high-confidence matches

---

## What We Are NOT Doing

- No separate `ui_server/`, `ui_gui/`, `ui_web/` crates — those are compile outputs, not folders
- No SQLite, flat files, or optional DB backend — SurrealDB kv-rocksdb is mandatory
- No simplified/dummy implementations — every port must pass the same test cases as C# original
- No CubeCL — CPU path (fast_image_resize + Rayon + POPCNT) is sufficient
- No Avalonia fixes — C# codebase is a read-only spec reference, not a live product

---

## Active Branch

`claude/fix-intro-production-M687e`
