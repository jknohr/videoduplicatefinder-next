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
using FFmpeg.AutoGen;

namespace VDF.Core.FFTools {
	/// <summary>
	/// Extracts I-frame timestamps evenly distributed across the full video duration
	/// by seeking to target positions rather than reading packets sequentially.
	/// Seeking is critical: sequential reads hit the sample cap near the start of the
	/// file, leaving the rest of a long video completely unsampled.
	/// </summary>
	internal static class IFrameExtractor {

		/// <summary>
		/// Returns up to <paramref name="maxSamples"/> I-frame timestamps (seconds) spread
		/// uniformly across [<paramref name="startSec"/>, <paramref name="endSec"/>].
		/// Each target position is reached via <c>av_seek_frame(AVSEEK_FLAG_BACKWARD)</c>
		/// which snaps to the nearest preceding keyframe — exactly one packet read per
		/// sample, so cost scales with <paramref name="maxSamples"/>, not video duration.
		///
		/// <para>
		/// When <paramref name="intervalSec"/> &gt; 0 the caller requests a fixed time
		/// between samples (e.g. 30 s) and <paramref name="maxSamples"/> acts as a hard
		/// ceiling.  This ensures that a 9-minute clip and a 4-hour source file are sampled
		/// at the same temporal density, which is required for the sliding-window comparison
		/// to produce meaningful consecutive-match counts.
		/// </para>
		/// </summary>
		internal static unsafe List<double> GetKeyframePtsByInterval(
			string path,
			double startSec, double endSec,
			int    maxSamples,
			double intervalSec = 0) {

			double window = endSec - startSec;
			if (window <= 0 || maxSamples <= 0) return new List<double>();

			// Determine how many evenly-spaced target positions to seek to.
			int desiredCount;
			double actualInterval;
			if (intervalSec > 0) {
				desiredCount  = Math.Min(maxSamples, Math.Max(1, (int)Math.Ceiling(window / intervalSec)));
				actualInterval = window / desiredCount;
			}
			else {
				desiredCount   = maxSamples;
				actualInterval = window / Math.Max(1, desiredCount - 1);
			}

			var result = new List<double>(desiredCount);

			// 30-second I/O watchdog
			long deadlineTicks = Stopwatch.GetTimestamp() + (long)(30.0 * Stopwatch.Frequency);
			AVIOInterruptCB_callback interruptCb = _ => Stopwatch.GetTimestamp() > deadlineTicks ? 1 : 0;

			AVFormatContext* fmtCtx = null;
			AVPacket* pkt = null;
			try {
				fmtCtx = ffmpeg.avformat_alloc_context();
				if (fmtCtx == null) return result;
				fmtCtx->interrupt_callback = new AVIOInterruptCB { callback = interruptCb };

				var ctx = fmtCtx;
				if (ffmpeg.avformat_open_input(&ctx, path, null, null) < 0) return result;
				fmtCtx = ctx;
				if (ffmpeg.avformat_find_stream_info(fmtCtx, null) < 0) return result;

				int streamIdx = ffmpeg.av_find_best_stream(
					fmtCtx, AVMediaType.AVMEDIA_TYPE_VIDEO, -1, -1, null, 0);
				if (streamIdx < 0) return result;

				AVStream* stream = fmtCtx->streams[streamIdx];
				AVRational tb   = stream->time_base;

				pkt = ffmpeg.av_packet_alloc();
				if (pkt == null) return result;

				double lastRecorded = double.MinValue;

				for (int i = 0; i < desiredCount; i++) {
					// Target timestamp for this sample slot
					double targetSec = desiredCount == 1
						? startSec + window * 0.5
						: startSec + actualInterval * i;
					if (targetSec > endSec) break;

					// Seek backward to the nearest keyframe at or before targetSec
					long targetPts = Convert.ToInt64(targetSec * tb.den / tb.num);
					int seekRet = ffmpeg.av_seek_frame(
						fmtCtx, streamIdx, targetPts, ffmpeg.AVSEEK_FLAG_BACKWARD);
					if (seekRet < 0) continue;

					// Read the very next keyframe packet on the video stream
					bool found = false;
					for (int attempt = 0; attempt < 64 && !found; attempt++) {
						ffmpeg.av_packet_unref(pkt);
						int ret = ffmpeg.av_read_frame(fmtCtx, pkt);
						if (ret == ffmpeg.AVERROR_EOF) goto done;
						if (ret < 0) break;
						if (pkt->stream_index != streamIdx) continue;
						if ((pkt->flags & ffmpeg.AV_PKT_FLAG_KEY) == 0) continue;

						double pktSec = pkt->pts != ffmpeg.AV_NOPTS_VALUE
							? pkt->pts * (double)tb.num / tb.den
							: pkt->dts != ffmpeg.AV_NOPTS_VALUE
								? pkt->dts * (double)tb.num / tb.den
								: -1;

						if (pktSec < 0 || pktSec < startSec || pktSec > endSec) break;

						// Deduplicate: skip if this seek landed on the same keyframe as the last one
						// (can happen when targets are closer together than the GOP length)
						if (Math.Abs(pktSec - lastRecorded) < 0.1) break;

						result.Add(pktSec);
						lastRecorded = pktSec;
						found = true;
					}
				}
				done:;
			}
			catch {
				// Best-effort: return whatever was collected
			}
			finally {
				if (pkt != null) ffmpeg.av_packet_free(&pkt);
				if (fmtCtx != null) ffmpeg.avformat_close_input(&fmtCtx);
			}
			return result;
		}
	}
}
