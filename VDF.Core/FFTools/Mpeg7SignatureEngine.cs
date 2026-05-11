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
using System.Diagnostics;
using System.IO;
using System.Security.Cryptography;
using System.Text;
using VDF.Core.Utils;

namespace VDF.Core.FFTools {
	/// <summary>
	/// Wraps FFmpeg's built-in MPEG-7 Video Signature filter (ISO/IEC 15938).
	/// Extraction produces compact binary .mpeg7sig files (~2.5 MB/hr of video) that
	/// describe the temporal structure of a video in a resolution- and bitrate-agnostic way.
	/// Comparison uses FFmpeg's <c>detectmode=full</c> to find sub-segment matches
	/// (i.e. a clip appearing somewhere inside a longer video).
	/// </summary>
	internal static class Mpeg7SignatureEngine {
		static string SigFolder => Utils.DatabaseUtils.GetSignatureFolder();

		/// <summary>
		/// Extracts the MPEG-7 binary signature for <paramref name="videoPath"/>.
		/// The signature file is written to the VDF state folder.
		/// Returns <c>true</c> on success and sets <paramref name="sigPath"/>.
		/// </summary>
		internal static bool ExtractSignature(
			string videoPath, out string sigPath, bool extendedLogging) {
			sigPath = string.Empty;
			string ffmpegPath = FfmpegEngine.FFmpegPath;
			if (string.IsNullOrEmpty(ffmpegPath)) return false;

			string folder = SigFolder;
			Directory.CreateDirectory(folder);
			// Name signature file by hash of the full path so it survives renames of VDF db
			string pathHash = ComputePathHash(videoPath);
			sigPath = Path.Combine(folder, $"{pathHash}.mpeg7sig");

			if (File.Exists(sigPath) && new FileInfo(sigPath).Length > 0)
				return true;  // already extracted

			// FFmpeg signature filter writes the binary file directly
			string args = $"-hide_banner -loglevel {(extendedLogging ? "info" : "quiet")} -nostdin " +
				$"-i \"{videoPath}\" " +
				$"-vf \"signature=format=binary:filename={EscapePath(sigPath)}\" " +
				$"-f null -";
			try {
				using var proc = new Process {
					StartInfo = new ProcessStartInfo {
						FileName = ffmpegPath,
						Arguments = args,
						UseShellExecute = false,
						RedirectStandardOutput = false,
						RedirectStandardError = !extendedLogging,
						CreateNoWindow = true
					}
				};
				proc.Start();
				if (!extendedLogging) proc.StandardError.ReadToEnd();
				proc.WaitForExit(120_000);
				return File.Exists(sigPath) && new FileInfo(sigPath).Length > 0;
			}
			catch (Exception e) {
				Logger.Instance.Info($"Mpeg7SignatureEngine.ExtractSignature failed on '{videoPath}': {e.Message}");
				sigPath = string.Empty;
				return false;
			}
		}

		/// <summary>
		/// Compares two MPEG-7 signature files.
		/// Uses <c>detectmode=full</c> which finds sub-segment matches (clip-inside-movie).
		/// Returns (isMatch, offsetSeconds, 1.0).
		/// </summary>
		internal static (bool isMatch, double offsetSec, double confidence)
			CompareSignatures(string sigPath1, string sigPath2, bool extendedLogging) {
			string ffmpegPath = FfmpegEngine.FFmpegPath;
			if (string.IsNullOrEmpty(ffmpegPath)) return (false, 0, 0);
			if (!File.Exists(sigPath1) || !File.Exists(sigPath2)) return (false, 0, 0);

			// Dummy inputs — the signature filter reads the sig files directly, not the videos.
			// We use lavfi nullsrc so no media decoding happens.
			string args = $"-hide_banner -loglevel info -nostdin " +
				$"-f lavfi -i nullsrc=size=1x1:duration=1 " +
				$"-f lavfi -i nullsrc=size=1x1:duration=1 " +
				$"-lavfi \"[0][1]signature=detectmode=full:nb_inputs=2" +
					$":filename={EscapePath(sigPath1)}|{EscapePath(sigPath2)}\" " +
				$"-f null -";
			try {
				using var proc = new Process {
					StartInfo = new ProcessStartInfo {
						FileName = ffmpegPath,
						Arguments = args,
						UseShellExecute = false,
						RedirectStandardOutput = false,
						RedirectStandardError = true,
						CreateNoWindow = true
					}
				};
				proc.Start();
				bool matched = false;
				double offset = 0;
				while (!proc.StandardError.EndOfStream) {
					string? line = proc.StandardError.ReadLine();
					if (line == null) break;
					// FFmpeg prints "match at offset NNN.NNN" or "no match"
					if (line.Contains("match at offset", StringComparison.OrdinalIgnoreCase)) {
						matched = true;
						int idx = line.IndexOf("offset", StringComparison.OrdinalIgnoreCase) + 7;
						if (idx > 7) {
							int end = line.IndexOf(' ', idx);
							string offStr = end < 0 ? line[idx..] : line[idx..end];
							double.TryParse(offStr,
								System.Globalization.NumberStyles.Float,
								System.Globalization.CultureInfo.InvariantCulture,
								out offset);
						}
					}
				}
				proc.WaitForExit(30_000);
				return (matched, offset, matched ? 1.0 : 0.0);
			}
			catch (Exception e) {
				Logger.Instance.Info($"Mpeg7SignatureEngine.CompareSignatures failed: {e.Message}");
				return (false, 0, 0);
			}
		}

		static string ComputePathHash(string path) {
			byte[] bytes = SHA256.HashData(Encoding.UTF8.GetBytes(path));
			return Convert.ToHexString(bytes)[..16];
		}

		static string EscapePath(string path) =>
			path.Replace("\\", "/").Replace("'", "'\\''");
	}
}
