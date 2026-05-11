// /*
//     Copyright (C) 2025 0x90d
//     This file is part of VideoDuplicateFinder
//     VideoDuplicateFinder is free software: you can redistribute it and/or modify
//     it under the terms of the GNU Affero General Public License as published by
//     the Free Software Foundation, either version 3 of the License, or
//     (at your option) any later version.
//     VideoDuplicateFinder is distributed in the hope that it will be useful,
//     but WITHOUT ANY WARRANTY without even the implied warranty of
//     MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
//     GNU Affero General Public License for more details.
//     You should have received a copy of the GNU Affero General Public License
//     along with VideoDuplicateFinder.  If not, see <http://www.gnu.org/licenses/>.
// */
//


namespace VDF.Core {
	public enum FolderMatchMode { None, SameFolderOnly, DifferentFolderOnly }

	public sealed class Settings {
		// Settable so System.Text.Json can populate these from --settings JSON; without
		// a setter STJ silently leaves them empty even with IncludeFields=true (read-only
		// collection properties aren't repopulated by the default object converter).
		public HashSet<string> IncludeList { get; set; } = new HashSet<string>();
		public HashSet<string> BlackList { get; set; } = new HashSet<string>();

		public bool IgnoreReadOnlyFolders;
		public bool IgnoreReparsePoints;
		public bool ExcludeHardLinks;
		public bool GeneratePreviewThumbnails;
		public bool UseNativeFfmpegBinding;
		public bool IncludeSubDirectories = true;
		public bool IncludeImages = true;
		public bool ExtendedFFToolsLogging;
		public bool LogExcludedFiles;
		public bool AlwaysRetryFailedSampling;
		public bool IgnoreBlackPixels;
		public bool IgnoreWhitePixels;
		public bool CompareHorizontallyFlipped;
		public bool IncludeNonExistingFiles;
		public bool ScanAgainstEntireDatabase;
		public FolderMatchMode FolderMatchMode;
		public int SameFolderDepth = 1;
		public bool UsePHashing;
		public bool UseExifCreationDate;
		public string LanguageCode = "en";

		public FFTools.FFHardwareAccelerationMode HardwareAccelerationMode;

		public byte Threshhold = 5;
		public float Percent = 96f;
		public double PercentDurationDifference = 20d;
		public double DurationDifferenceMinSeconds;
		public double DurationDifferenceMaxSeconds;
		public double MaxSamplingDurationSeconds;

		// ── Intro / outro skip ──────────────────────────────────────────────────
		/// <summary>
		/// Seconds to skip at the start of each video before sampling begins.
		/// Combined with <see cref="SkipStartPercent"/>: the effective skip is
		/// <c>Max(SkipStartSeconds, duration * SkipStartPercent / 100)</c>.
		/// Default 0 (disabled).
		/// </summary>
		public double SkipStartSeconds;
		/// <summary>
		/// Percentage of effective video duration to skip at the start (0–50).
		/// Combined with <see cref="SkipStartSeconds"/>: the larger of the two wins.
		/// </summary>
		public float SkipStartPercent;
		/// <summary>Seconds to skip at the end of each video before the sampling window closes.</summary>
		public double SkipEndSeconds;
		/// <summary>Percentage of effective video duration to skip at the end (0–50).</summary>
		public float SkipEndPercent;

		public int ThumbnailCount = 1;
		/// <summary>Maximum width in pixels for display thumbnails (0 = original resolution).</summary>
		public int ThumbnailMaxWidth = 100;
		public int MaxDegreeOfParallelism = 1;

		public string CustomFFArguments = string.Empty;
		public string CustomDatabaseFolder = string.Empty;

		public bool FilterByFilePathContains;
		public List<string> FilePathContainsTexts = new();
		public bool FilterByFilePathNotContains;
		public List<string> FilePathNotContainsTexts = new();
		public bool FilterByFileSize;
		public int MaximumFileSize;
		public int MinimumFileSize;

		// ── Partial clip detection ──────────────────────────────────────────────
		/// <summary>Enable audio-fingerprint-based partial clip detection.</summary>
		public bool EnablePartialClipDetection;
		/// <summary>
		/// Minimum ratio of clip-duration / source-duration for a pair to be a candidate.
		/// Default 0.10 (clip must be at least 10% of the longer video).
		/// </summary>
		public double PartialClipMinRatio = 0.10;
		/// <summary>
		/// Minimum average Hamming similarity (0–1) for a sliding-window match to be
		/// accepted as a partial clip.  Default 0.80.
		/// </summary>
		public double PartialClipSimilarityThreshold = 0.80;
		/// <summary>
		/// When true, partial clip matches must also pass a visual frame check at the
		/// matched offset. Suppresses false positives from videos sharing the same audio
		/// (e.g. TikToks reusing a song) but with different visual content.
		/// </summary>
		public bool PartialClipRequireVisualMatch = true;
		/// <summary>
		/// Minimum visual similarity (0–1) for the on-demand frame check used by
		/// <see cref="PartialClipRequireVisualMatch"/>.  Default 0.85.
		/// Compared via pHash when <see cref="UsePHashing"/> is enabled, otherwise via
		/// 32×32 grayscale percentage difference.
		/// </summary>
		public double PartialClipVisualThreshold = 0.85;

		// ── I-Frame timeline fingerprint ────────────────────────────────────
		/// <summary>Enable I-frame-based timeline fingerprinting and sliding-window comparison.</summary>
		public bool EnableIFrameFingerprint;
		/// <summary>
		/// Hard ceiling on the number of I-frames sampled per video. Default 300.
		/// When <see cref="IFrameSampleIntervalSec"/> &gt; 0 the actual count is
		/// <c>Min(MaxIFrameSamples, ceil(duration / IFrameSampleIntervalSec))</c>.
		/// </summary>
		public int MaxIFrameSamples = 300;
		/// <summary>
		/// Seconds between I-frame samples. Default 30.
		/// This is the key parameter for correct clip-in-movie detection: both the clip
		/// and the source are sampled at the same temporal density, so array indices
		/// correspond to the same time intervals and the sliding-window comparison works.
		/// A 9-minute clip at 30 s/sample → 18 samples.
		/// A 4-hour source at 30 s/sample → 480 samples, capped at MaxIFrameSamples.
		/// Set to 0 to divide the window into exactly MaxIFrameSamples equal slots.
		/// </summary>
		public double IFrameSampleIntervalSec = 30.0;
		/// <summary>
		/// Minimum fraction of the shorter video's I-frames that must match for a timeline duplicate.
		/// With interval-based sampling a 9-minute clip in a 4-hour source can match 100% of
		/// the clip's 18 frames, so even 0.30 (30%) is conservative. Default 0.40.
		/// </summary>
		public float IFrameMatchPercent = 0.40f;
		/// <summary>
		/// Minimum consecutive matching I-frames required. With 30-second intervals each
		/// consecutive frame represents 30 seconds of matching content. Default 3 (= 90 s).
		/// </summary>
		public int IFrameMinConsecutive = 3;
		/// <summary>
		/// Minimum per-frame Hamming similarity (0–1) to count an I-frame as matching.
		/// Default 0.85 (~10 differing bits out of 64).
		/// </summary>
		public float IFrameHashThreshold = 0.85f;

		// ── Temporal average hash (tblend) ──────────────────────────────────
		/// <summary>Enable the temporal average hash (tblend collapse) fingerprint.</summary>
		public bool EnableTemporalAverageHash;
		/// <summary>Start of the tblend window in seconds from the beginning of the video.</summary>
		public double TemporalAverageHashStartSec = 120.0;
		/// <summary>Duration (seconds) of the tblend averaging window.</summary>
		public double TemporalAverageHashWindowSec = 60.0;

		// ── Scene-aware skip ────────────────────────────────────────────────
		/// <summary>Automatically detect the first scene transition and use it as the skip-start offset.</summary>
		public bool SceneAwareSkip;
		/// <summary>scdet sensitivity (0–100). Higher values = less sensitive. Default 14.</summary>
		public float SceneDetectionThreshold = 14f;
		/// <summary>Number of scene transitions to skip at the start. Default 1.</summary>
		public int SceneSkipCount = 1;

		// ── MPEG-7 signature ────────────────────────────────────────────────
		/// <summary>Enable MPEG-7 Video Signature extraction and comparison.</summary>
		public bool EnableMpeg7Signature;

		// ── SSIM second-pass verification ───────────────────────────────────
		/// <summary>Enable SSIM second-pass verification for borderline matches.</summary>
		public bool EnableSsimVerification;
		/// <summary>Minimum initial similarity for SSIM to be run (lower bound of gray zone). Default 0.80.</summary>
		public float SsimVerificationMinSim = 0.80f;
		/// <summary>Maximum initial similarity for SSIM to be run (upper bound of gray zone). Default 0.95.</summary>
		public float SsimVerificationMaxSim = 0.95f;
		/// <summary>SSIM score below which a borderline match is rejected. Default 0.90.</summary>
		public float SsimRejectThreshold = 0.90f;
		/// <summary>Duration (seconds) of the video segment compared by SSIM. Default 10.</summary>
		public double SsimWindowSeconds = 10.0;

		// ── Database checkpoints ────────────────────────────────────────────
		/// <summary>
		/// Interval in minutes between automatic database saves during scanning.
		/// 0 = disabled (only save at phase boundaries). Default 5.
		/// </summary>
		public int DatabaseCheckpointIntervalMinutes = 5;

		/// <summary>
		/// Returns the allowed duration tolerance in seconds for a video of the given duration,
		/// based on <see cref="PercentDurationDifference"/>, <see cref="DurationDifferenceMinSeconds"/>,
		/// and <see cref="DurationDifferenceMaxSeconds"/>. When the percent rule is disabled (0%),
		/// the seconds bounds act as a flat tolerance so users can run a seconds-only comparison.
		/// </summary>
		internal double GetDurationToleranceSeconds(double durationSeconds) {
			if (PercentDurationDifference > 0) {
				double toleranceSeconds = durationSeconds * PercentDurationDifference / 100d;
				if (DurationDifferenceMinSeconds > 0)
					toleranceSeconds = Math.Max(toleranceSeconds, DurationDifferenceMinSeconds);
				if (DurationDifferenceMaxSeconds > 0)
					toleranceSeconds = Math.Min(toleranceSeconds, DurationDifferenceMaxSeconds);
				return Math.Max(0d, toleranceSeconds);
			}
			// Percent rule disabled: tolerance comes solely from the seconds bounds. Without a
			// percent term, Max would otherwise pin the tolerance to 0; instead take the largest
			// enabled bound so a seconds-only setup behaves like a flat tolerance.
			return Math.Max(0d, Math.Max(DurationDifferenceMinSeconds, DurationDifferenceMaxSeconds));
		}
	}
}
