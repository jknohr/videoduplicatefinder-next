# Media Organizer

Video Duplicate Finder is a cross-platform tool that finds duplicate and near-duplicate video (and image) files on disk based on visual similarity. Unlike basic duplicate finders, it handles files that differ in resolution, frame rate, watermark, bitrate, encoding, and even videos that are **partial clips** of a longer original — including when the same scene has been re-edited.

---

## Features

- Cross-platform (Windows, Linux, macOS)
- Fast scanning with ultra-fast rescan (content hashed once, cached in database)
- Optional native FFmpeg bindings for maximum throughput
- Standard perceptual hash (pHash) duplicate detection
- **Configurable intro/outro skip** — shift the sampling window away from intros, credits, or branded bumpers
- **Scene-aware auto-skip** — automatically detect where static intros end using FFmpeg `scdet`, no manual configuration needed
- **I-frame timeline fingerprinting** — sample pHashes across the entire video at a fixed interval, then use a sliding-window comparison to find when a shorter video's content appears inside a longer one at any time offset
- **Gap-tolerant sliding window** — tunable tolerance for re-edits (alternate shots, B-roll inserts, rearranged scenes) vs. strict identical-clip detection
- **Temporal average hash** — collapse a time window into a single "blurred" image representing the color palette and motion energy of an entire segment; used as a fast rejection filter
- **MPEG-7 video signature** — ISO/IEC 15938 coarse signatures; resilient to resolution changes, bitrate compression, and mild cropping; returns the time offset of the match directly
- **SSIM second-pass verification** — for borderline matches, confirm or reject using full-frame structural similarity at the matched offset
- **Partial clip detection** — audio fingerprinting pipeline to find when a clip is taken from a longer source, even with no visual overlap
- Desktop GUI (Windows, Linux, macOS)
- Headless CLI for scripting and automation
- Web UI for remote/headless/NAS use, with video playback and live log panel
- Docker image with VA-API / NVIDIA GPU acceleration support
- **In-browser video playback** — watch any file directly in the Results page with full seek support
- **Live log panel** — see scan progress, errors, and FFmpeg messages in the browser, no `docker logs` required
- **Embedded metadata editing** — read and write container tags (title, genre, artist, show, episode, season, etc.) without re-encoding, directly from the Results page

---

## The detection problem — why a single sample is not enough

Most duplicate finders sample one frame per video (typically the midpoint) and compare those frames. This fails for any collection where the same content appears at different time positions in different files — regardless of whether those files have intros.

### Example collection

```
Video A  [41 min]  introA(3m) │ clip1(10m) │ clip2(6m) │ clip3(8m) │ clip4(14m)
Video B  [ 9 min]  introA(3m) │ clip2(6m)
Video C  [14 min]  introB(4m) │ clip1(10m)
Video D  [17 min]  introA(3m) │ clip4(14m)
Video E  [ 8 min]  clip3(8m)                        ← no intro at all
```

### What midpoint sampling sees

| Video | 50% position | Frame from |
|-------|-------------|------------|
| A     | 20.5 min    | clip2      |
| B     |  4.5 min    | clip2      | ← lucky match with A
| C     |  7.0 min    | clip1      | ← no match
| D     |  8.5 min    | clip4      | ← no match
| E     |  4.0 min    | clip3      | ← no match

Only A and B are identified as duplicates. C, D, and E are invisible. The shared source material (clip1 in A and C, clip4 in A and D, clip3 in A and E) is never discovered.

### What I-frame timeline fingerprinting sees

Instead of one frame, VDF samples one pHash every 30 seconds across the entire video. Then it slides the shorter video's hash sequence over the longer one, position by position, looking for the offset where the sequences best align.

```
Video A  (82 frames @ 30s)
  [0-5]  introA   [6-25] clip1   [26-37] clip2   [38-53] clip3   [54-81] clip4

Video E  (16 frames @ 30s)
  [0-15] clip3

Slide E over A:
  offset 0:  E[0] vs A[0]   → introA vs clip3    → no match
  offset 38: E[0] vs A[38]  → clip3 vs clip3     → MATCH  ← all 16 frames match
  → 16/16 = 100% match, 16 consecutive frames, offset confirmed
```

For Video B (introA + clip2):

```
Video B  (18 frames @ 30s)
  [0-5] introA   [6-17] clip2

Slide B over A:
  offset 20 (aligns clip2 content):
    B[0-5] (introA) vs A[20-25] (clip1) → 0 matches
    B[6-17] (clip2) vs A[26-37] (clip2) → 12 matches
  → 12/18 = 67% match, 12 consecutive frames
  → DETECTED (above 40% threshold, above 3 consecutive minimum)
```

All five videos are detected as sharing content with A. The match offset is stored and shown as a timeline strip in the Results page.

---

### Re-edits and alternate versions — the editing dimension

The same scenario gets more interesting when the source footage has been re-edited: same raw material, different assembly.

```
Video F  [11 min]  introA(3m) │ clip2-DirectorsCut(8m)
                               ↑ same scene as clip2, but extended with extra shots
                               and one alternate-angle insert halfway through

Video G  [13 min]  introA(3m) │ clip1-AlternateAngle(10m)
                               ↑ same scene as clip1: same actors, same dialogue,
                               same lighting — but filmed from a different camera position
```

**Strict mode (IFrameMaxGap = 0, IFrameHashThreshold = 0.85):**  
Requires consecutive matching frames with no gaps, and each frame must be 85% similar by pHash. clip2-DirectorsCut shares most of its frames with clip2, but the alternate-angle insert breaks the consecutive run. Depending on length of the insert, the run may still pass `IFrameMinConsecutive`. The alternate-angle clip1 in G may score only 70–80% per frame, which is below the 0.85 threshold — G is NOT detected. This is correct behavior when you want only exact duplicates or identical cuts.

**Re-edit mode (IFrameMaxGap = 2, IFrameHashThreshold = 0.80):**  
Up to 2 consecutive non-matching frames are silently bridged and the run continues. A 2-frame alternate-angle insert in clip2-DC is bridged over; the run resumes on both sides. Video F is detected as a re-edit of clip2. Video G is still unlikely to be detected because the per-frame score (different camera = different image) stays below 0.80.

**Shared-source mode (IFrameMaxGap = 5, IFrameHashThreshold = 0.70):**  
Frequent inserts and alternate angles are tolerated. Same-scene frames from a different camera position (similar color palette, similar composition) now score above 0.70. Both F and G are detected. This mode answers the question: *"do these videos share the same underlying footage?"*

The user controls this distinction explicitly through three parameters:

| Parameter | What it controls |
|-----------|------------------|
| `IFrameMinConsecutive` | Minimum run length before declaring a match. Low = more sensitive to short shared segments; high = only flag substantial overlaps. |
| `IFrameMaxGap` | Non-matching frames allowed within a run before the run resets. 0 = identical cut required; 2 = re-edit/alternate shots tolerated; 5+ = shared source material with heavy re-assembly. |
| `IFrameHashThreshold` | Per-frame similarity required to count as a match. 0.85 = same encode (identical or near-identical content); 0.70 = same scene, different camera or mild color grade. |

---

## Advanced fingerprinting features

### Phase 1 — Configurable intro/outro skip

Shifts the sampling window so frames are drawn from a configurable sub-range of each video. Both a flat seconds value and a percentage of duration are supported; whichever is larger takes effect.

```
Settings:
  Skip start: 90 seconds (or 5% of duration, whichever is greater)
  Skip end:   30 seconds (or 2% of duration)
```

No seeking overhead — this is pure arithmetic that remaps fractional sample positions onto the reduced window.

**CLI flags:** `--skip-start-seconds`, `--skip-start-percent`, `--skip-end-seconds`, `--skip-end-percent`

---

### Phase 2 — I-frame timeline fingerprinting

For each video, VDF scans the container packet stream (no decoding) to locate keyframe (I-frame) PTS timestamps, then seeks to N evenly-spaced positions within that set and decodes only those frames. The result is a compact array of pHashes — one per sample — stored in the scan database.

At comparison time, the shorter array is slid over the longer one. At each offset, VDF walks the arrays and counts matches, tracking the longest consecutive run and gap count. Early exit per offset when accumulated mismatches exceed a budget proportional to the gap tolerance.

**Why interval-based sampling matters:** Both a 9-minute clip and a 4-hour source are sampled at the same time interval (default 30 seconds). The clip gets ~18 samples; the source gets ~300 (capped). Sliding 18 frames over 300 gives 282 offsets to try; at the matching offset, array index i in the clip corresponds to the same 30-second interval as array index (offset + i) in the source. Without this, a fixed sample count on both videos produces arrays of the same length with zero sliding room, or arrays where indices represent completely different time intervals.

**Settings:**

| Setting | Default | Description |
|---------|---------|-------------|
| Enable I-frame fingerprint | off | Enable this detection phase |
| Sample interval (seconds) | 30 | One sample every N seconds across the video |
| Max I-frame samples | 300 | Cap on total samples per video (prevents huge arrays for very long content) |
| Match percent | 40% | Fraction of the shorter video's frames that must match at the best offset |
| Min consecutive frames | 3 | Minimum unbroken (or gap-bridged) run to declare a match |
| Max gap | 0 | Non-matching frames tolerated inside a run (0 = strict; 5 = heavy re-edit) |
| Hash threshold | 0.85 | Per-frame pHash similarity to count as a match |

**CLI flags:** `--iframe-fingerprint`, `--iframe-sample-interval`, `--max-iframe-samples`, `--iframe-match-percent`, `--iframe-min-consecutive`, `--iframe-max-gap`, `--iframe-hash-threshold`

---

### Phase 3 — Temporal average hash

Collapses a configurable time window of video into a single 32×32 grayscale image using FFmpeg's `tblend=all_mode=average` filter. The result represents the color palette and motion energy of that entire segment — it is impossible to replicate with different content.

Used as a fast rejection filter before the more expensive I-frame sliding window: if two videos' temporal average hashes are very different, the pair is skipped entirely.

**CLI flags:** `--temporal-avg-hash`, `--temporal-avg-start-sec`, `--temporal-avg-window-sec`

---

### Phase 4 — Scene-aware auto-skip

Automatically detects where the first significant visual transition occurs using FFmpeg's `scdet` filter. Does not assume anything about intro structure — for a video with no intro, the first scene change is at or near second 0 and the effective skip is near-zero. For a video with a 3-minute static logo, the first large spike is at ~180 seconds.

Scene change timestamps are cached in the scan database. The cost is one decode pass per file, paid only on the first scan; rescans are instant.

**Settings:**

| Setting | Default | Description |
|---------|---------|-------------|
| Scene-aware skip | off | Enable auto-detection of skip point |
| Detection threshold | 14 | Scene change sensitivity (0–100; higher = less sensitive to gradual transitions) |
| Skip count | 1 | Number of scene transitions to skip at the start |

**CLI flags:** `--scene-aware-skip`, `--scene-detection-threshold`, `--scene-skip-count`

---

### Phase 5 — MPEG-7 video signature

Uses FFmpeg's `signature` filter (ISO/IEC 15938) to extract a compact binary fingerprint (~2.5 MB/hour of video). The coarse signature is a temporal histogram across 90-frame windows — essentially a video DNA. It is resilient to resolution differences, bitrate compression, and mild cropping.

Comparison runs FFmpeg's `detectmode=full` across two signature files. For clip-inside-movie scenarios, the output includes the time offset of the match directly. Signature files are stored in the VDF database folder and reused across scans.

**CLI flag:** `--mpeg7-signature`

---

### Phase 6 — SSIM second-pass verification

For borderline matches (similarity in the configurable grey zone), computes the Structural Similarity Index (SSIM) at the matched time offset using FFmpeg's `ssim` filter. An SSIM score below the reject threshold causes the match to be discarded — preventing false positives from visually similar but distinct content (for example, different episodes of the same TV series with identical color grading).

**Settings:**

| Setting | Default | Description |
|---------|---------|-------------|
| Enable SSIM verification | off | |
| Verify min similarity | 80% | Lower bound of the "grey zone" |
| Verify max similarity | 95% | Upper bound of the "grey zone" |
| Reject threshold | 0.90 | SSIM score below this = hard reject |
| Window seconds | 10 | Duration of video compared at the matched offset |

**CLI flags:** `--ssim-verification`, `--ssim-verify-min-sim`, `--ssim-verify-max-sim`, `--ssim-reject-threshold`, `--ssim-window-seconds`

---

## Partial clip detection

VDF can detect when a shorter video is a partial clip of a longer one — for example, a scene ripped from a movie, or a clip saved from a longer recording. This works even when there is no visual overlap between the two files.

It runs as an optional second phase after the normal visual duplicate scan, using an audio fingerprinting pipeline (Chromaprint-style chroma extraction + sliding-window Hamming similarity matching). Matched pairs appear in the duplicate list with a **Clip Offset** column showing where in the source the clip starts.

### Enabling it

In **Settings → Partial Clip Detection**, check **Enable Partial Clip Detection** and adjust:

| Setting | Default | Description |
|---------|---------|-------------|
| Min clip / source ratio (%) | 10 | Minimum clip duration as a percentage of the source duration. Clips shorter than this are ignored. |
| Min audio similarity (%) | 80 | Minimum average Hamming similarity for the sliding-window fingerprint match to be accepted. |

> **Note:** Partial clip detection requires audio tracks in both files. Videos without audio are skipped.

---

## Results page visualization

Each duplicate group in the Results page now displays:

**Detection badges** — color-coded labels on the group header showing which method(s) found the match:
- `I-frame timeline` — I-frame sliding window
- `MPEG-7` — MPEG-7 video signature
- `Audio fingerprint` — partial clip via audio
- `Frame similarity` — standard pHash comparison
- `Flipped` — horizontally mirrored content

**Timeline strips** — each video card shows a bar representing the full duration of that file:
- Source videos show colored segments indicating where each matching clip was found (derived from the stored clip offset and clip duration)
- Clip videos show their entire bar highlighted to indicate they are the matched sub-segment
- Standard frame-match pairs show evenly-spaced sample markers

**Match explanation** — a one-line summary below each timeline:
- *"I-frame timeline · clip found at 1:23:45 in source · 67% match"*
- *"Source video · 3 clip(s) mapped to it · 48% of duration covered"*
- *"Frame similarity · 94% match"*

---

## Embedded metadata editing

Container tags (title, genre, artist, description, show, episode ID, season number, track, composer, etc.) can be read and written directly from the Results page without re-encoding the video.

Click the **⋮** context menu on any result card and select **Edit metadata…**. A modal opens showing all tags read from the file via `ffprobe`. Edit any field and click **Save** — VDF rewrites the container metadata using `ffmpeg -c copy` (copy all streams, metadata only), then replaces the original file atomically.

Supported containers: MP4, MKV, AVI, MOV, WebM, and any other format FFmpeg can remux.

---

## Live log panel

The Web UI includes a **Logs** page (navigation link in the sidebar) showing the last 500 log lines from the current VDF session in real time. New lines appear automatically without a page refresh. A **Clear** button empties the view; an **Auto-scroll** toggle keeps the panel pinned to the latest entry.

All log output is also written to Docker's stdout, so `docker logs <container>` shows the same VDF-level messages alongside the ASP.NET framework logs.

---

## In-browser video playback

Any video file in the Results page can be played directly in the browser. Click the play icon on a result card to open a full-size video player modal. The player supports seeking, because the `/video` endpoint implements HTTP 206 Partial Content range requests — required for the browser `<video>` element's seek bar to function.

Security: only files under the configured scan directories are served. Paths outside those directories return 403.

---

## Downloads

[Daily build](https://github.com/0x90d/videoduplicatefinder/releases/tag/3.0.x) — attachments are automatically rebuilt and replaced on every commit.

Available packages per platform:
- `GUI-<platform>` — desktop application
- `CLI-<platform>` — command-line tool
- `Web-<platform>` — self-contained web server

---

## Desktop GUI

### Requirements

FFmpeg and FFprobe are required. On first launch VDF attempts to download them automatically.
Native FFmpeg binding requires FFmpeg 8.x shared libraries (not the master branch).

#### Windows
Download the latest FFmpeg GPL shared package from https://ffmpeg.org/download.html
Extract `ffmpeg.exe` and `ffprobe.exe` into the same folder as `VDF.GUI.exe`, a subfolder named `bin`, or ensure they are on your `PATH`.

#### Linux
```bash
sudo apt-get update && sudo apt-get install ffmpeg
```
Then run:
```bash
chmod +x VDF.GUI
./VDF.GUI
```

**Optional: add to your application menu**

The Linux archive includes `videoduplicatefinder.desktop` and `icon.png`. To register the app with your desktop environment (GNOME, KDE, XFCE, etc.):

```bash
# Edit the Exec= and Icon= paths to match where you extracted the archive, e.g.:
sed -i "s|/opt/videoduplicatefinder|$(pwd)|g" videoduplicatefinder.desktop

# Install for the current user
mkdir -p ~/.local/share/applications
cp videoduplicatefinder.desktop ~/.local/share/applications/
```

The app will then appear in your application launcher with its icon.

#### macOS
```bash
brew install ffmpeg
```
Extract the archive — it contains `Video Duplicate Finder.app`. Double-click it to launch.

If macOS blocks the app with "cannot be opened because the developer cannot be verified", right-click the `.app` and choose **Open**, then confirm. You only need to do this once.

If macOS still refuses to launch the bundle (e.g. "library load disallowed by system policy" on macOS 14+ / Tahoe), clear the quarantine flag and re-sign every binary in the bundle ad-hoc:
```bash
xattr -cr "Video Duplicate Finder.app"
codesign --force --deep --sign - "Video Duplicate Finder.app"
```

---

## CLI (Command-line Interface)

The CLI is useful for scripting, scheduled tasks, and headless servers where no display is available.

### Requirements

Same as the GUI: FFmpeg and FFprobe must be on your `PATH` or in the same directory as the `vdf-cli` binary.

### Installation

Download `CLI-<platform>` from the [releases page](https://github.com/0x90d/videoduplicatefinder/releases/tag/3.0.x) and extract it.

On Linux/macOS, make the binary executable:
```bash
chmod +x vdf-cli
```

### Usage

#### Scan and compare in one step
```bash
vdf-cli scan-and-compare --include /path/to/media
```

#### Scan multiple directories, save results as JSON
```bash
vdf-cli scan-and-compare \
  --include /mnt/movies \
  --include /mnt/series \
  --exclude /mnt/movies/extras \
  --format json \
  --output results.json
```

#### Find clips-inside-movies in a large collection

This example is tuned for a collection where the same source content appears at different offsets across many files (re-uploads, clips, re-edits):

```bash
vdf-cli scan-and-compare \
  --include /videos \
  --native-ffmpeg \
  --use-phash \
  --iframe-fingerprint \
  --iframe-sample-interval 30 \
  --max-iframe-samples 300 \
  --iframe-match-percent 35 \
  --iframe-min-consecutive 4 \
  --iframe-max-gap 0 \
  --percent 94 \
  --parallelism 4 \
  --format json --output results.json
```

`--iframe-max-gap 0` means only exact consecutive runs count — identical clips, same cut. Increase to `2` to also catch re-edits with occasional alternate shots, or to `5` to catch heavily rearranged content from the same source material.

#### Common options

| Flag | Description | Default |
|------|-------------|----------|
| `--include <path>` | Directory to scan (repeatable) | required |
| `--exclude <path>` | Directory to exclude (repeatable) | — |
| `--threshold <n>` | Hash difference threshold | 5 |
| `--percent <n>` | Minimum similarity % to report | 96 |
| `--parallelism <n>` | Parallel hashing threads | 1 |
| `--include-images` | Also scan image files | off |
| `--use-phash` | Use perceptual hashing | off |
| `--partial-clip-detection` | Enable partial clip detection (audio fingerprinting) | off |
| `--partial-clip-min-ratio <n>` | Min clip/source duration ratio (0.0–1.0) | 0.10 |
| `--partial-clip-similarity <n>` | Min audio fingerprint similarity (0.0–1.0) | 0.80 |
| `--skip-start-seconds <n>` | Seconds to skip at the start of each video | 0 |
| `--skip-start-percent <n>` | Percentage of duration to skip at start (takes max with seconds) | 0 |
| `--skip-end-seconds <n>` | Seconds to skip at the end | 0 |
| `--skip-end-percent <n>` | Percentage of duration to skip at end | 0 |
| `--scene-aware-skip` | Auto-detect intro end using scene change detection | off |
| `--scene-detection-threshold <n>` | Scene change sensitivity (0–100) | 14 |
| `--scene-skip-count <n>` | Number of scene transitions to skip at start | 1 |
| `--iframe-fingerprint` | Enable I-frame timeline fingerprinting | off |
| `--iframe-sample-interval <n>` | Seconds between I-frame samples | 30.0 |
| `--max-iframe-samples <n>` | Maximum I-frame samples per video | 300 |
| `--iframe-match-percent <n>` | Required match fraction (0.0–1.0) | 0.40 |
| `--iframe-min-consecutive <n>` | Minimum consecutive matching frames | 3 |
| `--iframe-max-gap <n>` | Non-matching frames tolerated inside a run | 0 |
| `--iframe-hash-threshold <n>` | Per-frame pHash similarity to count as match | 0.85 |
| `--temporal-avg-hash` | Enable temporal average hash filter | off |
| `--temporal-avg-start-sec <n>` | Start of the averaging window (seconds) | 120.0 |
| `--temporal-avg-window-sec <n>` | Duration of the averaging window (seconds) | 60.0 |
| `--mpeg7-signature` | Enable MPEG-7 video signature comparison | off |
| `--ssim-verification` | Enable SSIM second-pass for borderline matches | off |
| `--ssim-verify-min-sim <n>` | Lower bound of grey zone for SSIM check | 0.80 |
| `--ssim-verify-max-sim <n>` | Upper bound of grey zone for SSIM check | 0.95 |
| `--ssim-reject-threshold <n>` | SSIM score below this = reject match | 0.90 |
| `--ssim-window-seconds <n>` | Duration compared at matched offset | 10.0 |
| `--format json\|text\|csv` | Output format | text |
| `--output <file>` | Write results to file instead of stdout | stdout |
| `--settings <file>` | Load full settings from a JSON file | — |

#### Auto-mark and delete duplicates
```bash
# Dry run — shows what would be deleted, no changes made (default)
vdf-cli scan-and-compare --include /mnt/media --action lowest-quality --dry-run

# Move duplicates to trash (safer)
vdf-cli scan-and-compare --include /mnt/media --action lowest-quality --delete

# Permanently delete (use with care)
vdf-cli scan-and-compare --include /mnt/media --action lowest-quality --delete-permanent
```

Available `--action` strategies:

| Strategy | Keeps |
|----------|-------|
| `lowest-quality` | Highest bitrate/resolution per group |
| `smallest-file` | Largest file per group |
| `shortest-duration` | Longest duration per group |
| `worst-resolution` | Highest resolution per group |
| `100-percent-only` | Only acts on 100% identical groups |

> **Note:** Automatic deletion is not recommended. Always review results with `--dry-run` first.

---

## Web UI

The Web UI runs as a local web server and is accessed from your browser. It is designed for headless machines, NAS devices, and remote management.

> **Security note:** The Web UI is password-protected but intended for local/Docker use only. Do not expose it to the internet.

### Authentication

On first launch, a random password is generated and printed to the console:

```
============================================
  Web UI password:  aB3xK9mQ7p
============================================
```

Enter this password in your browser to log in. A "Remember me" cookie keeps you logged in for 30 days.

**Docker users:** Run `docker logs vdf-web` to see the password.

| Environment variable | Description |
|---------------------|-------------|
| `VDF_WEB_PASSWORD` | Set your own password instead of the auto-generated one |
| `VDF_WEB_AUTH=false` | Disable authentication entirely |

### Requirements

FFmpeg and FFprobe are required. When running outside Docker, VDF.Web will attempt to download them automatically on first launch. You can also install them manually via your system package manager or place them on your `PATH`.

### Installation (self-contained archive)

Download `Web-<platform>` from the [releases page](https://github.com/0x90d/videoduplicatefinder/releases/tag/3.0.x) and extract it.

On Linux/macOS:
```bash
chmod +x VDF.Web
./VDF.Web
```

On Windows:
```
VDF.Web.exe
```

Then open **http://localhost:5000** in your browser and enter the password shown in the console.

To change the port:
```bash
ASPNETCORE_URLS=http://+:8080 ./VDF.Web
```

Settings and the scan database are saved to:
- Windows: `%APPDATA%\VDF\`
- Linux: `~/.config/VDF/`
- macOS: `~/Library/Preferences/VDF/`

---

## Docker (Web UI)

Docker is the easiest way to run the Web UI on a NAS, home server, or any Linux machine. FFmpeg is included in the image — no separate installation needed.

### Requirements

- [Docker](https://docs.docker.com/get-docker/) installed

### Quick start

```bash
docker run -d \
  --name vdf-web \
  -p 8080:8080 \
  -v vdf-db:/root/.config/VDF \
  -v vdf-state:/root/.local/state/VDF \
  -v /path/to/your/media:/media:ro \
  ghcr.io/0x90d/vdf-web:latest
```

Then open **http://localhost:8080** in your browser.
Check the password with `docker logs vdf-web` and enter it to log in.
Inside the Web UI, add `/media` (or whatever path you mounted) as a scan directory.

To set your own password:
```bash
docker run -d \
  --name vdf-web \
  -p 8080:8080 \
  -e VDF_WEB_PASSWORD=mysecretpassword \
  -v vdf-db:/root/.config/VDF \
  -v vdf-state:/root/.local/state/VDF \
  -v /path/to/your/media:/media:ro \
  ghcr.io/0x90d/vdf-web:latest
```

### docker compose (recommended for permanent installs)

1. Download [`docker-compose.yml`](docker-compose.yml) from this repository.

2. Edit the file and add your media volume mounts:
```yaml
environment:
  - VDF_WEB_PASSWORD=mysecretpassword    # optional — otherwise check docker logs
volumes:
  - /mnt/nas/movies:/mnt/nas/movies:ro
  - /mnt/nas/series:/mnt/nas/series:ro
```

3. Start the service:
```bash
docker compose up -d
```

4. Open **http://localhost:8080** in your browser.

5. To update to the latest image:
```bash
docker compose pull && docker compose up -d
```

### Hardware GPU acceleration

The Docker image includes VA-API libraries for Intel and AMD GPU-accelerated decoding. To enable hardware acceleration, pass the GPU device and set the driver name:

```yaml
services:
  vdf-web:
    image: ghcr.io/0x90d/vdf-web:latest
    devices:
      - /dev/dri:/dev/dri        # Intel / AMD VA-API and DRM access
    environment:
      - LIBVA_DRIVER_NAME=iHD    # Intel Gen 8+; use i965 for older Intel
      # - LIBVA_DRIVER_NAME=radeonsi   # AMD
    group_add:
      - video                    # required on some distros for /dev/dri access
```

For NVIDIA, install the [NVIDIA Container Toolkit](https://docs.nvidia.com/datacenter/cloud-native/container-toolkit/install-guide.html) on the host, then uncomment the `deploy` block in `docker-compose.yml`:

```yaml
    deploy:
      resources:
        reservations:
          devices:
            - driver: nvidia
              count: 1
              capabilities: [gpu, video, compute]
```

Verify hardware access inside the container:
```bash
docker exec vdf-web vainfo                # VA-API (Intel/AMD)
docker exec vdf-web ffmpeg -hwaccels      # list all available hardware accelerators
docker exec vdf-web nvidia-smi            # NVIDIA
```

### Volume reference

| Volume | Purpose |
|--------|----------|
| `/root/.config/VDF` | Settings (`web-settings.json`) and login credentials |
| `/root/.local/state/VDF` | Scan database (`ScannedFiles.db`) — mount a named volume here so hashed data persists across container updates |
| Your media paths | Mount each media directory you want to scan. Read-only (`:ro`) is recommended. |

### Folder permissions and SELinux

Before VDF can read or write your media files, the host directories must be accessible by the container.

#### Quick checklist

```bash
# 1. Check that the directories are readable
ls -la /path/to/your/videos

# 2. Grant read access to everyone (scan-only / :ro mounts)
chmod -R a+rX /path/to/your/videos

# 3. Grant write access to the group (needed for delete / move / rename — :rw mounts)
chmod -R g+rwX /path/to/your/videos
chown -R $USER:$USER /path/to/your/videos   # or replace $USER:$USER with user:video
```

#### Fedora / RHEL / CentOS / Rocky Linux — SELinux `:Z` label required

On SELinux-enforcing systems, Docker bind-mounts are **denied by default**, even when the host permissions look correct. You must append a relabel suffix to every volume path:

| Suffix | Meaning |
|--------|----------|
| `:Z` | Relabel for this container only (private). Use for single-container setups. |
| `:z` | Relabel as shared (multiple containers may access the same directory). |

Without `:Z`, SELinux blocks the container from reading the directory and VDF will report "no files found" or permission errors with no other explanation.

```yaml
# Fedora / RHEL — always add :Z (or :z) to every bind-mount:
volumes:
  - /home/user/Videos/movies:/movies:rw,Z
  - /home/user/Videos/series:/series:ro,Z
  - vdf-db:/root/.config/VDF          # named volumes do NOT need :Z
  - vdf-state:/root/.local/state/VDF
```

Ubuntu, Debian, Arch, and other non-SELinux systems: `:Z` is silently ignored, so it is safe to include it in a shared `docker-compose.yml` that runs on mixed hosts.

#### Verify access from inside the container

```bash
# Can the container read your media?
docker exec vdf-web ls /your/mounted/path

# Check SELinux context if on Fedora/RHEL
ls -Z /path/to/your/videos
```

### Notes

- The container image is built for `linux/amd64` and `linux/arm64` (Raspberry Pi / NAS ARM boards).
- The image is published to [GitHub Container Registry](https://github.com/0x90d/videoduplicatefinder/pkgs/container/vdf-web) and updated automatically on every commit.

---

## License
Video Duplicate Finder is licensed under AGPLv3

## Credits / Third Party
- [Avalonia](https://github.com/AvaloniaUI/Avalonia)
- [ActiPro Avalonia Controls (Free Edition)](https://github.com/Actipro/Avalonia-Controls)
- [FFmpeg.AutoGen](https://github.com/Ruslan-B/FFmpeg.AutoGen)
- [protobuf-net](https://github.com/protobuf-net/protobuf-net)
- [SixLabors.ImageSharp](https://github.com/SixLabors/ImageSharp)
- [AcoustID.NET by wo80](https://github.com/wo80/AcoustID.NET) — the audio fingerprinting pipeline (Chromaprint-style chroma extraction, FIR smoothing, and fingerprint encoding) used for partial clip detection is derived from this library, licensed under LGPL 2.1

## Building
- .NET 9.x
- Visual Studio 2022 or later is recommended

## Contributing
- Create a pull request for each addition or fix — do not merge them into one PR
- Unless it refers to an existing issue, write into your pull request what it does
- For larger PRs, open an issue for discussion first
