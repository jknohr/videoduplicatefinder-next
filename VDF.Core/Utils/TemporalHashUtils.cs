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
	/// Used to detect when the hash sequence of a shorter video appears as a contiguous
	/// sub-sequence inside a longer video, regardless of where in the longer video it starts.
	/// </summary>
	internal static class TemporalHashUtils {
		/// <summary>
		/// Slides <paramref name="shorter"/> over <paramref name="longer"/>, computing
		/// per-position Hamming similarity at each offset.
		/// </summary>
		/// <param name="shorter">I-frame pHashes for the candidate clip / shorter video.</param>
		/// <param name="longer">I-frame pHashes for the source / longer video.</param>
		/// <param name="hashSimilarityThreshold">
		/// Minimum per-frame Hamming similarity (0–1) to count a frame as "matching".
		/// Default 0.85 (≈ 10 differing bits out of 64).
		/// </param>
		/// <returns>
		/// (bestSimilarity, bestOffsetIndex, longestConsecutiveRun) where:
		/// <list type="bullet">
		///   <item>bestSimilarity — fraction of shorter's frames that matched at bestOffsetIndex</item>
		///   <item>bestOffsetIndex — start index in <paramref name="longer"/> of the best alignment</item>
		///   <item>longestConsecutiveRun — the longest run of consecutive matching frames seen</item>
		/// </list>
		/// </returns>
		internal static (float bestSimilarity, int bestOffsetIndex, int longestConsecutiveRun)
			SlidingWindowTimelineCompare(
				ulong[] shorter, ulong[] longer,
				float hashSimilarityThreshold = 0.85f) {

			if (shorter.Length == 0 || longer.Length == 0)
				return (0f, 0, 0);
			if (shorter.Length > longer.Length)
				return (0f, 0, 0);

			// Pre-compute the Hamming bit threshold: frames with <= maxBits differing bits count as matching
			int maxBits = (int)Math.Floor((1.0 - hashSimilarityThreshold) * 64.0);

			float bestSim = 0f;
			int bestOffset = 0;
			int bestRun = 0;

			int windowCount = longer.Length - shorter.Length + 1;
			// Budget for early-exit: allow up to (1 - minAcceptable) fraction of misses
			// before giving up on an offset. We use 0.70 as the floor — if we couldn't
			// achieve 70% match we skip the rest of this offset.
			const float earlyExitFloor = 0.70f;

			for (int o = 0; o < windowCount; o++) {
				int matchCount = 0;
				int currentRun = 0;
				int maxRun = 0;
				int missesAllowed = (int)Math.Ceiling(shorter.Length * (1.0 - earlyExitFloor));
				int missCount = 0;

				for (int i = 0; i < shorter.Length; i++) {
					int bits = HammingDistance(shorter[i], longer[o + i]);
					if (bits <= maxBits) {
						matchCount++;
						currentRun++;
						if (currentRun > maxRun) maxRun = currentRun;
					}
					else {
						currentRun = 0;
						missCount++;
						if (missCount > missesAllowed) break;
					}
				}

				float sim = (float)matchCount / shorter.Length;
				if (sim > bestSim) {
					bestSim = sim;
					bestOffset = o;
					bestRun = maxRun;
				}
				else if (sim == bestSim && maxRun > bestRun) {
					bestRun = maxRun;
					bestOffset = o;
				}
			}

			return (bestSim, bestOffset, bestRun);
		}

		[MethodImpl(MethodImplOptions.AggressiveInlining)]
		static int HammingDistance(ulong a, ulong b) => BitOperations.PopCount(a ^ b);
	}
}
