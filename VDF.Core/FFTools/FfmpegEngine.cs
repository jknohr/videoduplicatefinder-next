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

using System;
using System.Collections.Generic;
using System.Diagnostics;
using System.Globalization;
using System.Linq;
using System.Runtime.InteropServices;
using FFmpeg.AutoGen;
using SixLabors.ImageSharp;
using SixLabors.ImageSharp.Processing;
using VDF.Core.FFTools.FFmpegNative;
using VDF.Core.Utils;

namespace VDF.Core.FFTools {
	internal static partial class FfmpegEngine {
		public static readonly string FFmpegPath;
		const int TimeoutDuration = 15_000; //15 seconds
		public static FFHardwareAccelerationMode HardwareAccelerationMode;
		public static string CustomFFArguments = string.Empty;
		public static bool UseNativeBinding;
		private static readonly SixLabors.ImageSharp.Formats.Jpeg.JpegEncoder jpegEncoder = new();
		static FfmpegEngine() => FFmpegPath = FFToolsUtils.GetPath(FFToolsUtils.FFTool.FFmpeg) ?? string.Empty;


		static AVHWDeviceType GetConfiguredHardwareDeviceType() => HardwareAccelerationMode switch {
			FFHardwareAccelerationMode.vdpau => AVHWDeviceType.AV_HWDEVICE_TYPE_VDPAU,
			FFHardwareAccelerationMode.dxva2 => AVHWDeviceType.AV_HWDEVICE_TYPE_DXVA2,
			FFHardwareAccelerationMode.vaapi => AVHWDeviceType.AV_HWDEVICE_TYPE_VAAPI,
			FFHardwareAccelerationMode.qsv => AVHWDeviceType.AV_HWDEVICE_TYPE_QSV,
			FFHardwareAccelerationMode.cuda => AVHWDeviceType.AV_HWDEVICE_TYPE_CUDA,
			FFHardwareAccelerationMode.videotoolbox => AVHWDeviceType.AV_HWDEVICE_TYPE_VIDEOTOOLBOX,
			FFHardwareAccelerationMode.d3d11va => AVHWDeviceType.AV_HWDEVICE_TYPE_D3D11VA,
			FFHardwareAccelerationMode.drm => AVHWDeviceType.AV_HWDEVICE_TYPE_DRM,
			//FFHardwareAccelerationMode.opencl => AVHWDeviceType.AV_HWDEVICE_TYPE_OPENCL, OpenCL support is irrelevant for frame extraction
			FFHardwareAccelerationMode.mediacodec => AVHWDeviceType.AV_HWDEVICE_TYPE_MEDIACODEC,
			FFHardwareAccelerationMode.vulkan => AVHWDeviceType.AV_HWDEVICE_TYPE_VULKAN,
			_ => AVHWDeviceType.AV_HWDEVICE_TYPE_NONE
		};

		/// <summary>
		/// Copies a 32x32 GRAY8 frame produced by <see cref="VideoFrameConverter"/> into a
		/// freshly-allocated 1024-byte buffer. swscale uses an aligned padded destination
		/// (linesize >= width); the common case is linesize == 32 because we asked for
		/// align=0 and 32 is already aligned, in which case a single copy is enough.
		/// </summary>
		static unsafe byte[] ExtractGray32FromFrame(AVFrame convertedFrame) {
			const int N = 32;
			int width = convertedFrame.width;
			int height = convertedFrame.height;
			if (width != N || height != N)
				throw new Exception($"Unexpected size {width}x{height}, expected {N}x{N}.");
			if (convertedFrame.data[0] == null)
				throw new Exception("Converted frame has no data[0] (null).");
			int srcStride = convertedFrame.linesize[0];
			if (srcStride < width)
				throw new Exception($"Invalid linesize ({srcStride}) for width {width}.");

			byte[] outBuf = new byte[width * height];
			fixed (byte* destPtr = outBuf) {
				byte* sourcePtr = convertedFrame.data[0];
				if (srcStride == width) {
					Buffer.MemoryCopy(sourcePtr, destPtr, width * height, width * height);
				}
				else {
					for (int y = 0; y < height; y++)
						Buffer.MemoryCopy(sourcePtr + (y * srcStride), destPtr + (y * width), width, width);
				}
			}
			return outBuf;
		}

		static int CountMissingGrayBytePositions(FileEntry videoFile, List<float> positions, double maxSamplingDurationSeconds) {
			int missing = 0;
			for (int i = 0; i < positions.Count; i++) {
				double position = videoFile.GetGrayBytesIndex(positions[i], maxSamplingDurationSeconds);
				if (!videoFile.grayBytes.ContainsKey(position))
					missing++;
			}
			return missing;
		}

		/// <summary>
		/// Opens a single <see cref="VideoStreamDecoder"/> and a single <see cref="VideoFrameConverter"/>
		/// for the file, then walks the requested positions reusing both. This avoids the per-position
		/// avformat_open_input + sws_getContext cost of looping <see cref="GetThumbnail"/>.
		///
		/// On any FFmpeg error we abort and return false; the caller falls back to the per-sample
		/// CLI/native path so partial extraction still succeeds. Already-cached positions are skipped.
		/// </summary>
		static unsafe bool TryGetGrayBytesFromVideoNativeBatch(
			FileEntry videoFile,
			List<float> positions,
			double maxSamplingDurationSeconds,
			ref int tooDarkCounter,
			Action<int>? onSampleComplete) {
			const int N = 32;
			try {
				using var vsd = new VideoStreamDecoder(videoFile.Path, GetConfiguredHardwareDeviceType());
				VideoFrameConverter? converter = null;
				Size converterSourceSize = default;
				AVPixelFormat converterSrcFmt = AVPixelFormat.AV_PIX_FMT_NONE;
				try {
					for (int i = 0; i < positions.Count; i++) {
						double position = videoFile.GetGrayBytesIndex(positions[i], maxSamplingDurationSeconds);
						if (videoFile.grayBytes.ContainsKey(position)) {
							onSampleComplete?.Invoke(i + 1);
							continue;
						}

						if (!vsd.TryDecodeFrame(out var srcFrame, TimeSpan.FromSeconds(position)))
							throw new Exception($"TryDecodeFrame failed at pos={position} for '{videoFile.Path}'");

						// HW decode reports the real (downloaded) sw_format on the frame itself,
						// not on the codec context, so we read it post-decode. SW decode keeps it
						// stable on the codec context.
						Size sourceSize = new(
							srcFrame.width > 0 ? srcFrame.width : vsd.FrameSize.Width,
							srcFrame.height > 0 ? srcFrame.height : vsd.FrameSize.Height);
						AVPixelFormat srcPixFmt = vsd.IsHardwareDecode ? (AVPixelFormat)srcFrame.format : vsd.PixelFormat;
						if (srcPixFmt < 0 || srcPixFmt >= AVPixelFormat.AV_PIX_FMT_NB)
							throw new Exception($"Invalid source pixel format {srcPixFmt}");
						if (sourceSize.Width <= 0 || sourceSize.Height <= 0)
							throw new Exception($"Invalid source frame dimensions {sourceSize.Width}x{sourceSize.Height}");

						// Reuse the SwsContext across positions when the source layout is unchanged.
						// In practice this is the common case for the same file; the rebuild branch
						// only fires if HW decode hands us a different sw_format on a later frame.
						if (converter == null || sourceSize != converterSourceSize || srcPixFmt != converterSrcFmt) {
							converter?.Dispose();
							converter = new VideoFrameConverter(
								sourceSize, srcPixFmt,
								new Size(N, N), AVPixelFormat.AV_PIX_FMT_GRAY8,
								VideoFrameConverter.ScaleQuality.Bicubic, bitExact: false);
							converterSourceSize = sourceSize;
							converterSrcFmt = srcPixFmt;
						}

						AVFrame convertedFrame = converter.Convert(srcFrame);
						byte[] data = ExtractGray32FromFrame(convertedFrame);

						if (!GrayBytesUtils.VerifyGrayScaleValues(data))
							tooDarkCounter++;
						videoFile.grayBytes.Add(position, data);
						videoFile.PHashes.Add(position, pHash.PerceptualHash.ComputePHashFromGray32x32(data));
						onSampleComplete?.Invoke(i + 1);
					}
				}
				finally {
					converter?.Dispose();
				}
				return true;
			}
			catch (Exception e) {
				Logger.Instance.Info($"Native batch graybytes failed on '{videoFile.Path}', falling back to per-sample path. Exception: {e}");
				return false;
			}
		}

		/// <summary>
		/// Extracts I-frame pHashes from <paramref name="videoFile"/> within the window
		/// [<paramref name="skipStartSec"/> .. duration-<paramref name="skipEndSec"/>].
		/// On success, <see cref="FileEntry.IFrameTimestamps"/> and
		/// <see cref="FileEntry.IFramePHashes"/> are populated.
		/// Returns <c>false</c> only on hard errors (missing file, no video stream).
		/// Empty keyframe list is not an error — it stores empty arrays.
		/// </summary>
		internal static unsafe bool ExtractIFrameTimeline(
			FileEntry videoFile,
			double skipStartSec,
			double skipEndSec,
			int maxFrames = 100) {
			const int N = 32;
			try {
				double dur = videoFile.mediaInfo?.Duration.TotalSeconds ?? 0;
				if (dur <= 0) {
					videoFile.IFrameTimestamps = Array.Empty<double>();
					videoFile.IFramePHashes    = Array.Empty<ulong>();
					return true;
				}
				double endSec = Math.Max(skipStartSec, dur - skipEndSec);
				if (endSec <= skipStartSec) endSec = dur;

				List<double> rawPts = IFrameExtractor.GetKeyframePts(
					videoFile.Path, skipStartSec, endSec, maxFrames * 4);

				if (rawPts.Count == 0) {
					videoFile.IFrameTimestamps = Array.Empty<double>();
					videoFile.IFramePHashes    = Array.Empty<ulong>();
					return true;
				}

				List<double> selectedPts = IFrameExtractor.EvenlySubsample(rawPts, maxFrames);

				var timestamps = new List<double>(selectedPts.Count);
				var hashes     = new List<ulong>(selectedPts.Count);

				using var vsd = new VideoStreamDecoder(videoFile.Path, GetConfiguredHardwareDeviceType());
				VideoFrameConverter? converter = null;
				Size converterSrcSize = default;
				AVPixelFormat converterSrcFmt = AVPixelFormat.AV_PIX_FMT_NONE;
				try {
					foreach (double pts in selectedPts) {
						if (!vsd.TryDecodeFrame(out var srcFrame, TimeSpan.FromSeconds(pts)))
							continue;

						Size srcSize = new(
							srcFrame.width  > 0 ? srcFrame.width  : vsd.FrameSize.Width,
							srcFrame.height > 0 ? srcFrame.height : vsd.FrameSize.Height);
						AVPixelFormat srcFmt = vsd.IsHardwareDecode
							? (AVPixelFormat)srcFrame.format : vsd.PixelFormat;

						if (converter == null || srcSize != converterSrcSize || srcFmt != converterSrcFmt) {
							converter?.Dispose();
							converter = new VideoFrameConverter(srcSize, srcFmt,
								new Size(N, N), AVPixelFormat.AV_PIX_FMT_GRAY8);
							converterSrcSize = srcSize;
							converterSrcFmt  = srcFmt;
						}

						var converted = converter.Convert(srcFrame);
						byte[] data = ExtractGray32FromFrame(converted);

						timestamps.Add(pts);
						hashes.Add(pHash.PerceptualHash.ComputePHashFromGray32x32(data));
					}
				}
				finally {
					converter?.Dispose();
				}

				videoFile.IFrameTimestamps = timestamps.ToArray();
				videoFile.IFramePHashes    = hashes.ToArray();
				return true;
			}
			catch (Exception e) {
				Logger.Instance.Info($"ExtractIFrameTimeline failed on '{videoFile.Path}': {e.Message}");
				// Leave null so it will be retried on next scan
				return false;
			}
		}

		public static unsafe byte[]? GetThumbnail(FfmpegSettings settings, bool extendedLogging) {

			const int N = 32;
			const int ExpectedBytes = N * N;
			bool isGrayByte = settings.GrayScale == 1;

			try {
				if (UseNativeBinding) {


					AVHWDeviceType HWDevice = GetConfiguredHardwareDeviceType();

					using var vsd = new VideoStreamDecoder(settings.File, HWDevice);

					Size sourceSize = vsd.FrameSize;

					// Decode first so we know the real source pixel format. For HW decode
					// we can't know this up front — the downloaded sw_format depends on
					// the stream's bit depth (NV12 for 8-bit, P010LE for 10-bit HEVC, etc.).
					if (!vsd.TryDecodeFrame(out var srcFrame, settings.Position))
						throw new Exception($"TryDecodeFrame failed at pos={settings.Position} for '{settings.File}'. size={sourceSize.Width}x{sourceSize.Height}");

					AVPixelFormat srcPixFmt = vsd.IsHardwareDecode
						? (AVPixelFormat)srcFrame.format
						: vsd.PixelFormat;
					if (srcPixFmt < 0 || srcPixFmt >= AVPixelFormat.AV_PIX_FMT_NB)
						throw new Exception($"Invalid source pixel format {srcPixFmt}");

					if (sourceSize.Width <= 0 || sourceSize.Height <= 0)
						throw new Exception($"Invalid source frame dimensions {sourceSize.Width}x{sourceSize.Height}.");

					Size destinationSize = isGrayByte ? new Size(N, N) :
						settings.Fullsize == 1 ?
							sourceSize :
							new Size(100, Convert.ToInt32(sourceSize.Height * (100 / (double)sourceSize.Width)));

					AVPixelFormat destinationPixelFrmt = isGrayByte ?
						AVPixelFormat.AV_PIX_FMT_GRAY8 :
						AVPixelFormat.AV_PIX_FMT_BGRA;

					using var vfc = new VideoFrameConverter(
										sourceSize: sourceSize,
										sourcePixelFormat: srcPixFmt,
										destinationSize: destinationSize,
										destinationPixelFormat: destinationPixelFrmt,
										quality: VideoFrameConverter.ScaleQuality.Bicubic,
										bitExact: false);

					AVFrame convertedFrame = vfc.Convert(srcFrame);

					if (convertedFrame.data[0] == null)
						throw new Exception("Converted frame has no data[0] (null).");


					if (isGrayByte) {
						int width = convertedFrame.width; // should be 32
						if (convertedFrame.linesize[0] < width)
							throw new Exception($"Invalid linesize ({convertedFrame.linesize[0]}) for width {width}.");
						int height = convertedFrame.height; // should be 32
						int srcStride = convertedFrame.linesize[0]; // can be >= width (padding)
						IntPtr srcPtr = (IntPtr)convertedFrame.data[0];

						if (width != N || height != N)
							throw new Exception($"Unexpected size {width}x{height}, expected {N}x{N}.");

						byte[] outBuf = new byte[width * height]; // 1024
						fixed (byte* destPtr = outBuf) {
							byte* sourcePtr = (byte*)srcPtr;
							for (int y = 0; y < height; y++) {
								// Source: y*stride bytes offset; Target: y*width bytes
								Buffer.MemoryCopy(sourcePtr + (y * srcStride), destPtr + (y * width), width, width);
							}
						}
						return outBuf;
					}
					else {
						int width = convertedFrame.width;
						int height = convertedFrame.height;
						if (width <= 0 || height <= 0)
							throw new Exception($"Invalid converted frame dimensions {width}x{height}.");
						long totalBytesLong = (long)width * height * 4;
						if (totalBytesLong > 200_000_000) // ~200 MB sanity cap
							throw new Exception($"Frame too large: {width}x{height} ({totalBytesLong} bytes).");
						var totalBytes = (int)totalBytesLong;
						var rgbaBytes = new byte[totalBytes];
						int stride = convertedFrame.linesize[0];
						if (stride < width * 4)
							throw new Exception($"Invalid stride ({stride}) for width {width}.");
						fixed (byte* destPtr = rgbaBytes) {
							byte* sourcePtr = convertedFrame.data[0];
							if (stride == width * 4) {
								Buffer.MemoryCopy(sourcePtr, destPtr, totalBytes, totalBytes);
							}
							else {
								var byteWidth = width * 4;
								for (var y = 0; y < height; y++) {
									Buffer.MemoryCopy(sourcePtr + (y * stride), destPtr + (y * byteWidth), byteWidth, byteWidth);
								}
							}
						}
						var image = Image.LoadPixelData<SixLabors.ImageSharp.PixelFormats.Bgra32>(rgbaBytes, width, height);
						using MemoryStream stream = new();
						image.Save(stream, jpegEncoder);
						return stream.ToArray();
					}
				}
			}
			catch (Exception e) {
				Logger.Instance.Info($"Failed using native FFmpeg binding on '{settings.File}', try switching to process mode. Exception: {e}");
			}

			var psi = new ProcessStartInfo {
				FileName = FFmpegPath,
				CreateNoWindow = true,
				RedirectStandardInput = false,
				RedirectStandardOutput = true,
				WorkingDirectory = Path.GetDirectoryName(FFmpegPath)!,
				RedirectStandardError = extendedLogging,
				WindowStyle = ProcessWindowStyle.Hidden
			};

			psi.ArgumentList.Add("-hide_banner");
			psi.ArgumentList.Add("-loglevel"); psi.ArgumentList.Add((extendedLogging ? "error" : "quiet"));

			psi.ArgumentList.Add("-nostdin");

			if (HardwareAccelerationMode != FFHardwareAccelerationMode.none) {
				psi.ArgumentList.Add("-hwaccel");
				psi.ArgumentList.Add(HardwareAccelerationMode.ToString());
			}

			// -ss before -i (faster seek, may be less accurate; OK for frame sampling)
			psi.ArgumentList.Add("-ss"); psi.ArgumentList.Add(settings.Position.ToString(null, CultureInfo.InvariantCulture));
			psi.ArgumentList.Add("-i"); psi.ArgumentList.Add(FFToolsUtils.LongPathFix(settings.File));

			// Parse CustomFFArguments up front so we can detect a user-supplied -vf and merge it
			// into our own filter chain rather than letting a second -vf silently override the
			// scale filter (last -vf wins in ffmpeg). See: https://github.com/0x90d/videoduplicatefinder/issues/588
			string? userVfFilter = null;
			var remainingCustomArgs = new List<string>();
			if (!string.IsNullOrWhiteSpace(CustomFFArguments)) {
				var tokens = TokenizeArgs(CustomFFArguments);
				for (int ti = 0; ti < tokens.Count; ti++) {
					if ((tokens[ti] == "-vf" || tokens[ti] == "-filter:v") && ti + 1 < tokens.Count)
						userVfFilter = tokens[++ti];
					else
						remainingCustomArgs.Add(tokens[ti]);
				}
			}

			// Filter chain: scale + gray
			if (isGrayByte) {
				string vfChain = $"scale={N}:{N}:flags=bicubic,format=gray";
				if (userVfFilter != null) vfChain = $"{userVfFilter},{vfChain}";
				psi.ArgumentList.Add("-vf"); psi.ArgumentList.Add(vfChain);
				psi.ArgumentList.Add("-f"); psi.ArgumentList.Add("rawvideo");
				psi.ArgumentList.Add("-pix_fmt"); psi.ArgumentList.Add("gray");
			}
			else {
				if (settings.Fullsize != 1) {
					string vfChain = "scale=100:-1";
					if (userVfFilter != null) vfChain = $"{vfChain},{userVfFilter}";
					psi.ArgumentList.Add("-vf"); psi.ArgumentList.Add(vfChain);
				}
				else if (userVfFilter != null) {
					psi.ArgumentList.Add("-vf"); psi.ArgumentList.Add(userVfFilter);
				}
				psi.ArgumentList.Add("-f"); psi.ArgumentList.Add("mjpeg");
			}

			psi.ArgumentList.Add("-frames:v"); psi.ArgumentList.Add("1");

			foreach (var item in remainingCustomArgs)
				psi.ArgumentList.Add(item);
			psi.ArgumentList.Add("pipe:1"); // stdout

			////https://docs.microsoft.com/en-us/dotnet/csharp/how-to/concatenate-multiple-strings#string-literals
			//string ffmpegArguments = $" -hide_banner -loglevel {(extendedLogging ? "error" : "quiet")}" +
			//	$" -y -hwaccel {HardwareAccelerationMode} -ss {settings.Position} -i \"{FFToolsUtils.LongPathFix(settings.File)}\"" +
			//	$" -t 1 -f {(isGrayByte ? "rawvideo -pix_fmt gray" : "mjpeg")} -vframes 1" +
			//	$" {(isGrayByte ? "-s 16x16" : (settings.Fullsize == 1 ? string.Empty : "-vf scale=100:-1"))} {CustomFFArguments} \"-\"";

			using var process = new Process {
				StartInfo = psi
			};
			string errOut = string.Empty;
			// Collapse consecutive identical stderr lines: a single broken HEVC/H.264
			// stream can emit the same decoder error tens of thousands of times per
			// file (e.g. "[hevc] Error constructing the frame RPS"), turning the log
			// into noise. Track the last line and a repeat count, then flush.
			string lastErrLine = string.Empty;
			int repeatCount = 0;
			byte[]? bytes = null;
			try {
				process.EnableRaisingEvents = true;
				process.Start();
				if (extendedLogging) {
					process.ErrorDataReceived += new DataReceivedEventHandler((sender, e) => {
						if (e.Data?.Length > 0) {
							if (e.Data == lastErrLine) {
								repeatCount++;
							}
							else {
								if (repeatCount > 0) {
									errOut += $" (repeated {repeatCount} more time{(repeatCount == 1 ? string.Empty : "s")})";
									repeatCount = 0;
								}
								errOut += Environment.NewLine + e.Data;
								lastErrLine = e.Data;
							}
						}
					});
					process.BeginErrorReadLine();
				}
				using var ms = new MemoryStream();
				process.StandardOutput.BaseStream.CopyTo(ms);

				if (!process.WaitForExit(TimeoutDuration)) {
					throw new TimeoutException($"FFmpeg timed out on file: {settings.File}");
				}
				else if (extendedLogging)
					process.WaitForExit(); // Because of asynchronous event handlers, see: https://github.com/dotnet/runtime/issues/18789

				if (process.ExitCode != 0)
					throw new FFInvalidExitCodeException($"FFmpeg exited with: {process.ExitCode}");

				bytes = ms.ToArray();
				if (bytes.Length == 0)
					bytes = null;   // Makes subsequent checks easier
				else if (isGrayByte && bytes.Length != ExpectedBytes) {
					errOut += $"{Environment.NewLine}graybytes length != {ExpectedBytes} (got {bytes.Length})";
					bytes = null;
				}
			}
			catch (Exception e) {
				errOut += $"{Environment.NewLine}{e.Message}";
				try {
					if (process.HasExited == false)
						process.Kill();
				}
				catch { }
				bytes = null;
			}
			if (repeatCount > 0)
				errOut += $" (repeated {repeatCount} more time{(repeatCount == 1 ? string.Empty : "s")})";
			if (bytes == null || errOut.Length > 0) {
				string message = $"{((bytes == null) ? "ERROR: Failed to retrieve" : "WARNING: Problems while retrieving")} {(isGrayByte ? "graybytes" : "thumbnail")} from: {settings.File}";
				if (extendedLogging) {
					var args = string.Join(" ", psi.ArgumentList);
					message += $":{Environment.NewLine}{FFmpegPath} {args}";
				}
				Logger.Instance.Info($"{message}{errOut}");
			}
			return bytes;
		}
		internal static bool GetGrayBytesFromVideo(FileEntry videoFile, List<float> positions, double maxSamplingDurationSeconds, bool extendedLogging, Action<int>? onSampleComplete = null) {
			// Count missing up front so the TooDark check below compares against samples
			// we actually extracted this run, not the total positions (which may already
			// be partially cached from a prior scan).
			int missingPositions = CountMissingGrayBytePositions(videoFile, positions, maxSamplingDurationSeconds);
			if (missingPositions == 0) {
				for (int i = 0; i < positions.Count; i++)
					onSampleComplete?.Invoke(i + 1);
				return true;
			}

			int tooDarkCounter = 0;

			// Native batch path: open file + decoder + sws context once, walk all positions.
			// The for-loop fallback below recreates them per position, so on a 4-position scan
			// this avoids ~3x of the per-file FFmpeg setup cost.
			if (UseNativeBinding && TryGetGrayBytesFromVideoNativeBatch(videoFile, positions, maxSamplingDurationSeconds, ref tooDarkCounter, onSampleComplete)) {
				if (tooDarkCounter == missingPositions) {
					videoFile.Flags.Set(EntryFlags.TooDark);
					Logger.Instance.Info($"ERROR: Graybytes too dark of: {videoFile.Path}");
					return false;
				}
				return true;
			}

			// Re-count: the batch path may have populated some positions before throwing.
			missingPositions = CountMissingGrayBytePositions(videoFile, positions, maxSamplingDurationSeconds);
			if (missingPositions == 0)
				return true;

			tooDarkCounter = 0;
			for (int i = 0; i < positions.Count; i++) {
				double position = videoFile.GetGrayBytesIndex(positions[i], maxSamplingDurationSeconds);
				if (videoFile.grayBytes.ContainsKey(position)) {
					onSampleComplete?.Invoke(i + 1);
					continue;
				}

				var data = GetThumbnail(new FfmpegSettings {
					File = videoFile.Path,
					Position = TimeSpan.FromSeconds(position),
					GrayScale = 1,
				}, extendedLogging);
				if (data == null) {
					videoFile.Flags.Set(EntryFlags.ThumbnailError);
					return false;
				}
				if (!GrayBytesUtils.VerifyGrayScaleValues(data))
					tooDarkCounter++;
				videoFile.grayBytes.Add(position, data);
				videoFile.PHashes.Add(position, pHash.PerceptualHash.ComputePHashFromGray32x32(data));
				onSampleComplete?.Invoke(i + 1);
			}
			if (tooDarkCounter == missingPositions) {
				videoFile.Flags.Set(EntryFlags.TooDark);
				Logger.Instance.Info($"ERROR: Graybytes too dark of: {videoFile.Path}");
				return false;
			}
			return true;
		}

		private static List<string> TokenizeArgs(string args) {
			var tokens = new List<string>();
			var current = new System.Text.StringBuilder();
			bool inQuotes = false;
			foreach (char c in args) {
				if (c == '"') {
					inQuotes = !inQuotes;
				}
				else if (c == ' ' && !inQuotes) {
					if (current.Length > 0) {
						tokens.Add(current.ToString());
						current.Clear();
					}
				}
				else {
					current.Append(c);
				}
			}
			if (current.Length > 0)
				tokens.Add(current.ToString());
			return tokens;
		}

		/// <summary>
		/// Extracts a single JPEG thumbnail from a video file at the given position.
		/// Returns null if extraction fails.
		/// </summary>
		public static byte[]? ExtractThumbnailJpeg(string filePath, TimeSpan position, int maxWidth = 0, bool extendedLogging = false) {
			var settings = new FfmpegSettings {
				File = filePath,
				Position = position,
				GrayScale = 0,
				Fullsize = (byte)(maxWidth == 0 ? 1 : 0),
			};
			var raw = GetThumbnail(maxWidth == 0 ? settings : settings with { Fullsize = 1 }, extendedLogging);
			if (raw == null || raw.Length == 0) return null;

			if (maxWidth > 0) {
				using var ms = new MemoryStream(raw);
				using var image = Image.Load(ms);
				if (image.Width > maxWidth) {
					int h = (int)(image.Height * ((double)maxWidth / image.Width));
					image.Mutate(x => x.Resize(maxWidth, h));
				}
				using var outMs = new MemoryStream();
				image.Save(outMs, new SixLabors.ImageSharp.Formats.Jpeg.JpegEncoder { Quality = 90 });
				return outMs.ToArray();
			}
			return raw;
		}
	}

	internal struct FfmpegSettings {
		public byte GrayScale;
		public byte Fullsize;
		public string File;
		public TimeSpan Position;
	}

	// ── Temporal average hash (tblend) ────────────────────────────────────────
	internal static partial class FfmpegEngine {
		/// <summary>
		/// Collapses a <paramref name="windowSec"/>-second segment starting at
		/// <paramref name="startSec"/> into a single 32×32 grayscale "average frame" by
		/// applying FFmpeg's <c>tblend=all_mode=average</c> filter.  The resulting 1024-byte
		/// buffer is stored in <see cref="FileEntry.TemporalAverageGrayBytes"/>.
		/// </summary>
		internal static void ExtractTemporalAverageHash(
			FileEntry videoFile, double startSec, double windowSec,
			bool extendedLogging) {
			const int N = 32;
			const int ExpectedBytes = N * N;

			if (string.IsNullOrEmpty(FFmpegPath)) return;
			if (videoFile.mediaInfo == null) return;
			double dur = videoFile.mediaInfo.Duration.TotalSeconds;
			if (dur <= startSec) return;
			double actualWindow = Math.Min(windowSec, dur - startSec);
			if (actualWindow <= 0) return;

			try {
				string args = $"-hide_banner -loglevel quiet -nostdin " +
					$"-ss {startSec.ToString(System.Globalization.CultureInfo.InvariantCulture)} " +
					$"-t {actualWindow.ToString(System.Globalization.CultureInfo.InvariantCulture)} " +
					$"-i \"{videoFile.Path}\" " +
					$"-vf \"tblend=all_mode=average,framestep=32767,scale={N}:{N}:flags=bicubic,format=gray\" " +
					$"-frames:v 1 -f rawvideo pipe:1";

				using var proc = new Process {
					StartInfo = new ProcessStartInfo {
						FileName = FFmpegPath,
						Arguments = args,
						UseShellExecute = false,
						RedirectStandardOutput = true,
						RedirectStandardError = !extendedLogging,
						CreateNoWindow = true
					}
				};
				proc.Start();
				byte[] buf = new byte[ExpectedBytes];
				int read = 0;
				using (var stdout = proc.StandardOutput.BaseStream) {
					while (read < ExpectedBytes) {
						int n = stdout.Read(buf, read, ExpectedBytes - read);
						if (n == 0) break;
						read += n;
					}
				}
				proc.WaitForExit(10_000);
				if (read == ExpectedBytes)
					videoFile.TemporalAverageGrayBytes = buf;
			}
			catch (Exception e) {
				Logger.Instance.Info($"ExtractTemporalAverageHash failed on '{videoFile.Path}': {e.Message}");
			}
		}
	}

	// ── Scene-change detection (scdet) ────────────────────────────────────────
	internal static partial class FfmpegEngine {
		/// <summary>
		/// Runs the FFmpeg <c>select</c>+<c>showinfo</c> filter chain to detect the first
		/// <paramref name="maxCount"/> scene transitions whose scene score exceeds
		/// <paramref name="threshold"/>.  Returns timestamps in seconds.
		/// Sequential decode — no seek required, terminates early after finding enough scenes.
		/// </summary>
		internal static List<double> GetSceneChangeTimestamps(
			string videoPath, float threshold, int maxCount,
			bool extendedLogging) {
			var result = new List<double>(maxCount);
			if (string.IsNullOrEmpty(FFmpegPath)) return result;

			string threshStr = threshold.ToString("F2", System.Globalization.CultureInfo.InvariantCulture);
			// Use showinfo to emit pts_time for every selected frame
			string args = $"-hide_banner -loglevel info -nostdin -i \"{videoPath}\" " +
				$"-vf \"select='gt(scene,{threshStr})',showinfo\" " +
				$"-vsync vfr -frames:v {maxCount} -f null -";
			try {
				using var proc = new Process {
					StartInfo = new ProcessStartInfo {
						FileName = FFmpegPath,
						Arguments = args,
						UseShellExecute = false,
						RedirectStandardOutput = false,
						RedirectStandardError = true,
						CreateNoWindow = true
					}
				};
				proc.Start();
				// showinfo writes to stderr; parse "pts_time:NNN.NNN" tokens
				while (!proc.StandardError.EndOfStream) {
					string? line = proc.StandardError.ReadLine();
					if (line == null) break;
					int idx = line.IndexOf("pts_time:", StringComparison.Ordinal);
					if (idx < 0) continue;
					int start = idx + 9;
					int end = line.IndexOf(' ', start);
					string timeStr = end < 0 ? line[start..] : line[start..end];
					if (double.TryParse(timeStr,
						System.Globalization.NumberStyles.Float,
						System.Globalization.CultureInfo.InvariantCulture,
						out double t)) {
						result.Add(t);
						if (result.Count >= maxCount) break;
					}
				}
				proc.WaitForExit(30_000);
			}
			catch (Exception e) {
				Logger.Instance.Info($"GetSceneChangeTimestamps failed on '{videoPath}': {e.Message}");
			}
			return result;
		}
	}

	// ── Metadata tag writing ──────────────────────────────────────────────────
	internal static partial class FfmpegEngine {
		public static (bool success, string? error) WriteMetadataTags(
			string path, Dictionary<string, string> tags, bool extendedLogging) {
			if (string.IsNullOrEmpty(FFmpegPath))
				return (false, "FFmpeg path is not configured.");
			if (!File.Exists(path))
				return (false, $"File not found: {path}");

			string ext = Path.GetExtension(path);
			string tempPath = path + ".vdf_meta_tmp" + ext;

			var psi = new ProcessStartInfo {
				FileName = FFmpegPath,
				CreateNoWindow = true,
				RedirectStandardOutput = false,
				RedirectStandardError = true,
				WindowStyle = ProcessWindowStyle.Hidden,
				WorkingDirectory = Path.GetDirectoryName(FFmpegPath)!
			};
			psi.ArgumentList.Add("-hide_banner");
			psi.ArgumentList.Add("-loglevel"); psi.ArgumentList.Add("quiet");
			psi.ArgumentList.Add("-y");
			psi.ArgumentList.Add("-i"); psi.ArgumentList.Add(FFToolsUtils.LongPathFix(path));
			psi.ArgumentList.Add("-c"); psi.ArgumentList.Add("copy");
			psi.ArgumentList.Add("-map_metadata"); psi.ArgumentList.Add("0");
			foreach (var kvp in tags) {
				psi.ArgumentList.Add("-metadata");
				psi.ArgumentList.Add($"{kvp.Key}={kvp.Value}");
			}
			psi.ArgumentList.Add(FFToolsUtils.LongPathFix(tempPath));

			string stderrOutput = string.Empty;
			try {
				using var process = new Process { StartInfo = psi };
				process.Start();
				stderrOutput = process.StandardError.ReadToEnd();
				bool exited = process.WaitForExit(60_000);
				if (!exited) {
					try { process.Kill(); } catch { }
					if (File.Exists(tempPath)) File.Delete(tempPath);
					return (false, "FFmpeg timed out.");
				}
				if (process.ExitCode != 0) {
					if (File.Exists(tempPath)) File.Delete(tempPath);
					return (false, string.IsNullOrWhiteSpace(stderrOutput)
						? $"FFmpeg exited with code {process.ExitCode}."
						: stderrOutput.Trim());
				}
				File.Move(tempPath, path, overwrite: true);
				return (true, null);
			}
			catch (Exception ex) {
				try { if (File.Exists(tempPath)) File.Delete(tempPath); } catch { }
				return (false, ex.Message);
			}
		}
	}

	// ── SSIM second-pass verification ─────────────────────────────────────────
	internal static partial class FfmpegEngine {
		/// <summary>
		/// Computes the SSIM score between two video segments at given offsets.
		/// Uses <c>ffmpeg -lavfi [0][1]ssim</c>. Returns a score in [0, 1], or -1 on error.
		/// </summary>
		internal static float ComputeSsimAtOffset(
			string pathA, double offsetA,
			string pathB, double offsetB,
			double windowSec,
			bool extendedLogging) {
			if (string.IsNullOrEmpty(FFmpegPath)) return -1f;
			string ic = System.Globalization.CultureInfo.InvariantCulture.ToString();
			string oa = offsetA.ToString("F3", System.Globalization.CultureInfo.InvariantCulture);
			string ob = offsetB.ToString("F3", System.Globalization.CultureInfo.InvariantCulture);
			string ws = windowSec.ToString("F3", System.Globalization.CultureInfo.InvariantCulture);
			string args = $"-hide_banner -loglevel info -nostdin " +
				$"-ss {oa} -t {ws} -i \"{pathA}\" " +
				$"-ss {ob} -t {ws} -i \"{pathB}\" " +
				$"-lavfi \"[0][1]ssim=stats_file=-\" -f null -";
			try {
				using var proc = new Process {
					StartInfo = new ProcessStartInfo {
						FileName = FFmpegPath,
						Arguments = args,
						UseShellExecute = false,
						RedirectStandardOutput = false,
						RedirectStandardError = true,
						CreateNoWindow = true
					}
				};
				proc.Start();
				float ssim = -1f;
				while (!proc.StandardError.EndOfStream) {
					string? line = proc.StandardError.ReadLine();
					if (line == null) break;
					// Look for "All:X.XXXXXX" in stats output
					int idx = line.IndexOf("All:", StringComparison.Ordinal);
					if (idx < 0) continue;
					int start = idx + 4;
					int end = line.IndexOf(' ', start);
					string val = end < 0 ? line[start..] : line[start..end];
					if (float.TryParse(val,
						System.Globalization.NumberStyles.Float,
						System.Globalization.CultureInfo.InvariantCulture,
						out float v)) {
						ssim = v;
					}
				}
				proc.WaitForExit(60_000);
				return ssim;
			}
			catch (Exception e) {
				Logger.Instance.Info($"ComputeSsimAtOffset failed: {e.Message}");
				return -1f;
			}
		}
	}
}
