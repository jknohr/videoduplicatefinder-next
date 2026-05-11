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

using System.CommandLine;
using VDF.Core;
using VDF.Core.FFTools;

namespace VDF.CLI.Commands {
	/// <summary>Reusable option definitions shared across scan/compare commands.</summary>
	internal static class SharedOptions {
		internal static readonly Option<string[]> Include = new("--include", "-i") {
			Description = "Directory to include in the scan. Can be specified multiple times.",
			Arity = ArgumentArity.OneOrMore,
			AllowMultipleArgumentsPerToken = false
		};

		internal static readonly Option<string[]> Exclude = new("--exclude", "-e") {
			Description = "Directory to exclude from the scan. Can be specified multiple times.",
			Arity = ArgumentArity.ZeroOrMore,
			AllowMultipleArgumentsPerToken = false
		};

		internal static readonly Option<byte> Threshold = new("--threshold") {
			Description = "Hash difference threshold (0–10, lower = stricter). Default: 5.",
			DefaultValueFactory = _ => (byte)5
		};

		internal static readonly Option<float> Percent = new("--percent") {
			Description = "Minimum similarity percentage to report as duplicate. Default: 96.",
			DefaultValueFactory = _ => 96f
		};

		internal static readonly Option<int> Parallelism = new("--parallelism") {
			Description = "Maximum degree of parallelism for hashing. Default: 1.",
			DefaultValueFactory = _ => 1
		};

		internal static readonly Option<string?> Database = new("--db") {
			Description = "Custom folder to store the scan database.",
		};

		internal static readonly Option<bool> NoSubdirs = new("--no-subdirs") {
			Description = "Do not scan subdirectories."
		};

		internal static readonly Option<bool> IncludeImages = new("--include-images") {
			Description = "Include image files in the scan."
		};

		internal static readonly Option<bool> UsePhash = new("--use-phash") {
			Description = "Use perceptual hashing instead of frame sampling."
		};

		internal static readonly Option<bool> NativeFfmpeg = new("--native-ffmpeg") {
			Description = "Use native FFmpeg bindings instead of the CLI wrapper."
		};

		internal static readonly Option<FFHardwareAccelerationMode> HardwareAccel = new("--hardware-accel") {
			Description = "FFmpeg hardware acceleration mode (none, auto, cuda, vaapi, etc.). Default: none.",
			DefaultValueFactory = _ => FFHardwareAccelerationMode.none
		};

		internal static readonly Option<string?> CustomFfArgs = new("--ff-args") {
			Description = "Additional custom FFmpeg arguments."
		};

		internal static readonly Option<bool> IncludeNonExistingFiles = new("--include-non-existing") {
			Description = "Compare against database entries whose files no longer exist on disk."
		};

		internal static readonly Option<bool> EnablePartialClipDetection = new("--partial-clip-detection") {
			Description = "Enable partial clip detection via audio fingerprinting."
		};

		internal static readonly Option<double> PartialClipMinRatio = new("--partial-clip-min-ratio") {
			Description = "Minimum clip/source duration ratio (0.0–1.0). Default: 0.10.",
			DefaultValueFactory = _ => 0.10
		};

		internal static readonly Option<double> PartialClipSimilarityThreshold = new("--partial-clip-similarity") {
			Description = "Minimum audio fingerprint similarity threshold (0.0–1.0). Default: 0.80.",
			DefaultValueFactory = _ => 0.80
		};

		internal static readonly Option<bool> PartialClipRequireVisualMatch = new("--partial-clip-require-visual") {
			Description = "Require an on-demand visual frame check on partial-clip matches to filter false positives from videos that share an audio track but differ visually. Default: true.",
			DefaultValueFactory = _ => true
		};

		internal static readonly Option<double> PartialClipVisualThreshold = new("--partial-clip-visual-threshold") {
			Description = "Minimum visual similarity (0.0–1.0) for the partial-clip visual gate. Uses pHash when --use-phash is set, else 32x32 grayscale percent diff. Default: 0.85.",
			DefaultValueFactory = _ => 0.85
		};

		// ── Intro / outro skip ───────────────────────────────────────────────────
	internal static readonly Option<double> SkipStartSeconds = new("--skip-start-seconds") {
		Description = "Seconds to skip at the start of each video before sampling. Combined with --skip-start-percent: the larger value wins. Default: 0."
	};
	internal static readonly Option<float> SkipStartPercent = new("--skip-start-percent") {
		Description = "Percentage of video duration to skip at the start (0–50). Default: 0."
	};
	internal static readonly Option<double> SkipEndSeconds = new("--skip-end-seconds") {
		Description = "Seconds to skip at the end of each video. Default: 0."
	};
	internal static readonly Option<float> SkipEndPercent = new("--skip-end-percent") {
		Description = "Percentage of video duration to skip at the end (0–50). Default: 0."
	};

	// ── I-frame timeline fingerprint ─────────────────────────────────────────
	internal static readonly Option<bool> IFrameFingerprint = new("--iframe-fingerprint") {
		Description = "Enable I-frame timeline fingerprinting and sliding-window clip detection."
	};
	internal static readonly Option<int> MaxIFrameSamples = new("--max-iframe-samples") {
		Description = "Hard ceiling on I-frames sampled per video. Default: 300.",
		DefaultValueFactory = _ => 300
	};
	internal static readonly Option<double> IFrameSampleInterval = new("--iframe-sample-interval") {
		Description = "Seconds between I-frame samples (0 = divide duration into --max-iframe-samples equal slots). " +
		              "Use a fixed interval so clips and sources share the same temporal density — required for " +
		              "correct sliding-window detection. Default: 30.",
		DefaultValueFactory = _ => 30.0
	};
	internal static readonly Option<float> IFrameMatchPercent = new("--iframe-match-percent") {
		Description = "Minimum % of shorter video's frames that must match (0–100). Default: 40.",
		DefaultValueFactory = _ => 40f
	};
	internal static readonly Option<int> IFrameMinConsecutive = new("--iframe-min-consecutive") {
		Description = "Minimum consecutive matching I-frames. At 30 s/sample, 3 = 90 s of matching content. Default: 3.",
		DefaultValueFactory = _ => 3
	};
	internal static readonly Option<float> IFrameHashThreshold = new("--iframe-hash-threshold") {
		Description = "Per-frame Hamming similarity threshold (0–1) to count as matching. Default: 0.85.",
		DefaultValueFactory = _ => 0.85f
	};

	// ── Temporal average hash ─────────────────────────────────────────────────
	internal static readonly Option<bool> TemporalAvgHash = new("--temporal-avg-hash") {
		Description = "Enable temporal average (tblend) hash fingerprint."
	};
	internal static readonly Option<double> TemporalAvgStartSec = new("--temporal-avg-start-sec") {
		Description = "Start of tblend window in seconds. Default: 120.",
		DefaultValueFactory = _ => 120.0
	};
	internal static readonly Option<double> TemporalAvgWindowSec = new("--temporal-avg-window-sec") {
		Description = "Duration of tblend averaging window in seconds. Default: 60.",
		DefaultValueFactory = _ => 60.0
	};

	// ── Scene-aware skip ──────────────────────────────────────────────────────
	internal static readonly Option<bool> SceneAwareSkip = new("--scene-aware-skip") {
		Description = "Auto-detect first scene transition and use it as skip-start offset."
	};
	internal static readonly Option<float> SceneDetectionThreshold = new("--scene-detection-threshold") {
		Description = "scdet sensitivity (0–100). Higher = less sensitive. Default: 14.",
		DefaultValueFactory = _ => 14f
	};
	internal static readonly Option<int> SceneSkipCount = new("--scene-skip-count") {
		Description = "Number of scene transitions to skip at the start. Default: 1.",
		DefaultValueFactory = _ => 1
	};

	// ── MPEG-7 signature ──────────────────────────────────────────────────────
	internal static readonly Option<bool> Mpeg7Signature = new("--mpeg7-signature") {
		Description = "Enable MPEG-7 Video Signature extraction and comparison."
	};

	// ── SSIM second-pass verification ─────────────────────────────────────────
	internal static readonly Option<bool> SsimVerification = new("--ssim-verification") {
		Description = "Enable SSIM second-pass verification for borderline matches."
	};
	internal static readonly Option<float> SsimVerifyMinSim = new("--ssim-verify-min-sim") {
		Description = "Lower bound of similarity gray zone for SSIM. Default: 0.80.",
		DefaultValueFactory = _ => 0.80f
	};
	internal static readonly Option<float> SsimVerifyMaxSim = new("--ssim-verify-max-sim") {
		Description = "Upper bound of similarity gray zone for SSIM. Default: 0.95.",
		DefaultValueFactory = _ => 0.95f
	};
	internal static readonly Option<float> SsimRejectThreshold = new("--ssim-reject-threshold") {
		Description = "SSIM score below which a borderline match is rejected. Default: 0.90.",
		DefaultValueFactory = _ => 0.90f
	};
	internal static readonly Option<double> SsimWindowSeconds = new("--ssim-window-seconds") {
		Description = "Duration (seconds) of video segment compared by SSIM. Default: 10.",
		DefaultValueFactory = _ => 10.0
	};

	internal static readonly Option<int> CheckpointInterval = new("--checkpoint-interval") {
			Description = "Database checkpoint interval in minutes during scanning. 0 = disabled. Default: 5.",
			DefaultValueFactory = _ => 5
		};

		internal static readonly Option<FileInfo?> SettingsFile = new("--settings", "-s") {
			Description = "Path to a VDF settings JSON file. Individual flags override values from this file."
		};

		internal static readonly Option<string> Format = new("--format", "-f") {
			Description = "Output format: text (default), json, csv.",
			DefaultValueFactory = _ => "text"
		};

		internal static readonly Option<FileInfo?> Output = new("--output", "-o") {
			Description = "Write results to a file instead of stdout."
		};

		internal static void ApplyToSettings(Settings s, ParseResult r) {
			var includes = r.GetValue(Include);
			if (includes != null)
				foreach (var p in includes) s.IncludeList.Add(p);

			var excludes = r.GetValue(Exclude);
			if (excludes != null)
				foreach (var p in excludes) s.BlackList.Add(p);

			s.Threshhold = r.GetValue(Threshold);
			s.Percent = r.GetValue(Percent);
			s.MaxDegreeOfParallelism = r.GetValue(Parallelism);
			s.IncludeSubDirectories = !r.GetValue(NoSubdirs);
			s.IncludeImages = r.GetValue(IncludeImages);
			s.UsePHashing = r.GetValue(UsePhash);
			s.UseNativeFfmpegBinding = r.GetValue(NativeFfmpeg);
			s.HardwareAccelerationMode = r.GetValue(HardwareAccel);

			var db = r.GetValue(Database);
			if (db != null) s.CustomDatabaseFolder = db;

			var ffArgs = r.GetValue(CustomFfArgs);
			if (ffArgs != null) s.CustomFFArguments = ffArgs;

			s.SkipStartSeconds = r.GetValue(SkipStartSeconds);
			s.SkipStartPercent = r.GetValue(SkipStartPercent);
			s.SkipEndSeconds   = r.GetValue(SkipEndSeconds);
			s.SkipEndPercent   = r.GetValue(SkipEndPercent);

			// I-frame timeline fingerprint
			if (r.GetValue(IFrameFingerprint)) {
				s.EnableIFrameFingerprint  = true;
				s.MaxIFrameSamples         = r.GetValue(MaxIFrameSamples);
				s.IFrameSampleIntervalSec  = r.GetValue(IFrameSampleInterval);
				s.IFrameMatchPercent       = r.GetValue(IFrameMatchPercent) / 100f;
				s.IFrameMinConsecutive     = r.GetValue(IFrameMinConsecutive);
				s.IFrameHashThreshold      = r.GetValue(IFrameHashThreshold);
			}

			// Temporal average hash
			if (r.GetValue(TemporalAvgHash)) {
				s.EnableTemporalAverageHash    = true;
				s.TemporalAverageHashStartSec  = r.GetValue(TemporalAvgStartSec);
				s.TemporalAverageHashWindowSec = r.GetValue(TemporalAvgWindowSec);
			}

			// Scene-aware skip
			if (r.GetValue(SceneAwareSkip)) {
				s.SceneAwareSkip          = true;
				s.SceneDetectionThreshold = r.GetValue(SceneDetectionThreshold);
				s.SceneSkipCount          = r.GetValue(SceneSkipCount);
			}

			s.EnableMpeg7Signature = r.GetValue(Mpeg7Signature);

			// SSIM verification
			if (r.GetValue(SsimVerification)) {
				s.EnableSsimVerification  = true;
				s.SsimVerificationMinSim  = r.GetValue(SsimVerifyMinSim);
				s.SsimVerificationMaxSim  = r.GetValue(SsimVerifyMaxSim);
				s.SsimRejectThreshold     = r.GetValue(SsimRejectThreshold);
				s.SsimWindowSeconds       = r.GetValue(SsimWindowSeconds);
			}

			s.DatabaseCheckpointIntervalMinutes = r.GetValue(CheckpointInterval);
			s.IncludeNonExistingFiles = r.GetValue(IncludeNonExistingFiles);
			s.EnablePartialClipDetection = r.GetValue(EnablePartialClipDetection);
			s.PartialClipMinRatio = r.GetValue(PartialClipMinRatio);
			s.PartialClipSimilarityThreshold = r.GetValue(PartialClipSimilarityThreshold);
			s.PartialClipRequireVisualMatch = r.GetValue(PartialClipRequireVisualMatch);
			s.PartialClipVisualThreshold = r.GetValue(PartialClipVisualThreshold);
		}

		internal static void AddScanOptions(Command cmd) {
			cmd.Options.Add(Include);
			cmd.Options.Add(Exclude);
			cmd.Options.Add(Threshold);
			cmd.Options.Add(Percent);
			cmd.Options.Add(Parallelism);
			cmd.Options.Add(Database);
			cmd.Options.Add(NoSubdirs);
			cmd.Options.Add(IncludeImages);
			cmd.Options.Add(UsePhash);
			cmd.Options.Add(NativeFfmpeg);
			cmd.Options.Add(HardwareAccel);
			cmd.Options.Add(CustomFfArgs);
			cmd.Options.Add(CheckpointInterval);
			cmd.Options.Add(IncludeNonExistingFiles);
			cmd.Options.Add(SkipStartSeconds);
		cmd.Options.Add(SkipStartPercent);
		cmd.Options.Add(SkipEndSeconds);
		cmd.Options.Add(SkipEndPercent);
		cmd.Options.Add(IFrameFingerprint);
		cmd.Options.Add(MaxIFrameSamples);
		cmd.Options.Add(IFrameSampleInterval);
		cmd.Options.Add(IFrameMatchPercent);
		cmd.Options.Add(IFrameMinConsecutive);
		cmd.Options.Add(IFrameHashThreshold);
		cmd.Options.Add(TemporalAvgHash);
		cmd.Options.Add(TemporalAvgStartSec);
		cmd.Options.Add(TemporalAvgWindowSec);
		cmd.Options.Add(SceneAwareSkip);
		cmd.Options.Add(SceneDetectionThreshold);
		cmd.Options.Add(SceneSkipCount);
		cmd.Options.Add(Mpeg7Signature);
		cmd.Options.Add(SsimVerification);
		cmd.Options.Add(SsimVerifyMinSim);
		cmd.Options.Add(SsimVerifyMaxSim);
		cmd.Options.Add(SsimRejectThreshold);
		cmd.Options.Add(SsimWindowSeconds);
		cmd.Options.Add(EnablePartialClipDetection);
			cmd.Options.Add(PartialClipMinRatio);
			cmd.Options.Add(PartialClipSimilarityThreshold);
			cmd.Options.Add(PartialClipRequireVisualMatch);
			cmd.Options.Add(PartialClipVisualThreshold);
			cmd.Options.Add(SettingsFile);
			cmd.Options.Add(Format);
			cmd.Options.Add(Output);
		}
	}
}
