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
using System.Numerics;
using System.Runtime.CompilerServices;

namespace VDF.Core.Utils {
	/// <summary>
	/// Sliding-window comparison over sequences of perceptual hashes extracted from I-frames.
	/// Detects when the hash sequence of a shorter video appears as a contiguous sub-sequence
	/// inside a longer video, with optional gap tolerance for re-edit / alternate-cut detection.
	/// </summary>
	internal static class TemporalHashUtils {

		/// <summary>
		/// Slides <paramref name="shorter"/> over <paramref name="longer"/> and finds the best
		/// alignment. Returns the fraction of clip frames that matched, the start index in
		/// <paramref name="longer"/>, and the longest gap-tolerant consecutive run.
		/// </summary>
		/// <param name="shorter">I-frame pHashes for the clip / shorter video.</param>
		/// <param name="longer">I-frame pHashes for the source / longer video.</param>
		/// <param name="hashSimilarityThreshold">
		/// Minimum per-frame Hamming similarity (0–1) to count a frame as "matching".
		/// Default 0.85 (≈ 10 differing bits out of 64).
		/// </param>
		/// <param name="maxGap">
		/// Maximum non-matching frames allowed within a single matching segment before the
		/// run resets.  0 = strict consecutive (identical edit).  Higher values bridge across
		/// alternate shots or brief inserts — useful for detecting re-edits and alternate cuts
		/// of the same source material without requiring frame-perfect order.
		/// A gap of N means up to N consecutive non-matching frames can appear inside a
		/// matching segment without breaking it.
		/// </param>
		/// <returns>
		/// (bestSimilarity, bestOffsetIndex, longestConsecutiveRun):
		/// <list type="bullet">
		///   <item>bestSimilarity — fraction of clip frames that matched at the best offset</item>
		///   <item>bestOffsetIndex — start position in <paramref name="longer"/> of best alignment</item>
		///   <item>longestConsecutiveRun — longest gap-bridged matching segment (in frame count)</item>
		/// </list>
		/// </returns>
		internal static (float bestSimilarity, int bestOffsetIndex, int longestConsecutiveRun)
			SlidingWindowTimelineCompare(
				ulong[] shorter, ulong[] longer,
				float hashSimilarityThreshold = 0.85f,
				int   maxGap                  = 0) {

			if (shorter.Length == 0 || longer.Length == 0)
				return (0f, 0, 0);
			if (shorter.Length > longer.Length)
				return (0f, 0, 0);

			int maxBits = (int)Math.Floor((1.0 - hashSimilarityThreshold) * 64.0);

			float bestSim    = 0f;
			int   bestOffset = 0;
			int   bestRun    = 0;

			int windowCount = longer.Length - shorter.Length + 1;

			// Early-exit: if we accumulate enough misses that even a perfect remaining run
			// couldn't reach earlyExitFloor, skip the rest of this offset.
			const float earlyExitFloor = 0.70f;

			for (int o = 0; o < windowCount; o++) {
				int matchCount   = 0;
				int currentRun   = 0;  // matching frames in the current gap-tolerant segment
				int gapCount     = 0;  // consecutive non-matching frames since last match
				int maxRun       = 0;
				int missesAllowed = (int)Math.Ceiling(shorter.Length * (1.0 - earlyExitFloor));
				int hardMissCount = 0; // total non-matching frames (for early exit)

				for (int i = 0; i < shorter.Length; i++) {
					int bits = HammingDistance(shorter[i], longer[o + i]);

					if (bits <= maxBits) {
						// Matching frame — extend (or continue) the current segment.
						matchCount++;
						currentRun++;
						gapCount = 0;
						if (currentRun > maxRun) maxRun = currentRun;
					}
					else {
						hardMissCount++;
						if (gapCount < maxGap) {
							// Non-matching frame within gap budget — bridge it.
							// The segment continues but this frame doesn't count as a match.
							gapCount++;
							currentRun++;    // segment grows in span but not in match count
						}
						else {
							// Gap budget exhausted — reset the segment.
							// Subtract any gap frames that were included in currentRun.
							currentRun = 0;
							gapCount   = 0;
						}
						if (hardMissCount > missesAllowed) break;
					}
				}

				float sim = (float)matchCount / shorter.Length;
				if (sim > bestSim || (sim == bestSim && maxRun > bestRun)) {
					bestSim    = sim;
					bestOffset = o;
					bestRun    = maxRun;
				}
			}

			return (bestSim, bestOffset, bestRun);
		}

		[MethodImpl(MethodImplOptions.AggressiveInlining)]
		static int HammingDistance(ulong a, ulong b) => BitOperations.PopCount(a ^ b);
	}
}
