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
	/// Scans the packet stream of a video file to locate keyframe (I-frame) PTS values
	/// without decoding any pixel data.  Reading only packet headers is extremely fast —
	/// typically one or two orders of magnitude faster than a seek-and-decode operation.
	/// </summary>
	internal static class IFrameExtractor {
		/// <summary>
		/// Returns the presentation timestamps (in seconds from file start) of all keyframe
		/// packets in the primary video stream that fall within [<paramref name="startSec"/>,
		/// <paramref name="endSec"/>], collecting at most <paramref name="maxCount"/> entries.
		/// </summary>
		internal static unsafe List<double> GetKeyframePts(
			string path, double startSec, double endSec, int maxCount = 200) {

			var result = new List<double>(Math.Min(maxCount, 256));
			AVFormatContext* fmtCtx = null;
			AVPacket* pkt = null;

			// Interrupt callback: bail after 30 seconds of blocked I/O
			long deadlineTicks = Stopwatch.GetTimestamp() + (long)(30.0 * Stopwatch.Frequency);
			AVIOInterruptCB_callback interruptCb = _ => Stopwatch.GetTimestamp() > deadlineTicks ? 1 : 0;

			try {
				fmtCtx = ffmpeg.avformat_alloc_context();
				if (fmtCtx == null) return result;
				fmtCtx->interrupt_callback = new AVIOInterruptCB { callback = interruptCb };

				var ctx = fmtCtx;
				if (ffmpeg.avformat_open_input(&ctx, path, null, null) < 0) return result;
				fmtCtx = ctx;
				if (ffmpeg.avformat_find_stream_info(fmtCtx, null) < 0) return result;

				int streamIdx = ffmpeg.av_find_best_stream(fmtCtx,
					AVMediaType.AVMEDIA_TYPE_VIDEO, -1, -1, null, 0);
				if (streamIdx < 0) return result;

				AVStream* stream = fmtCtx->streams[streamIdx];
				AVRational tb = stream->time_base;
				// Convert seconds to stream timebase ticks
				long startPts = startSec <= 0
					? 0
					: Convert.ToInt64(startSec * tb.den / tb.num);

				if (startPts > 0)
					ffmpeg.av_seek_frame(fmtCtx, streamIdx, startPts, ffmpeg.AVSEEK_FLAG_BACKWARD);

				pkt = ffmpeg.av_packet_alloc();
				if (pkt == null) return result;

				while (result.Count < maxCount) {
					ffmpeg.av_packet_unref(pkt);
					int ret = ffmpeg.av_read_frame(fmtCtx, pkt);
					if (ret == ffmpeg.AVERROR_EOF) break;
					if (ret < 0) continue;  // skip bad packets
					if (pkt->stream_index != streamIdx) continue;
					if ((pkt->flags & ffmpeg.AV_PKT_FLAG_KEY) == 0) continue;

					double pktSec = pkt->pts != ffmpeg.AV_NOPTS_VALUE
						? pkt->pts * (double)tb.num / tb.den
						: pkt->dts != ffmpeg.AV_NOPTS_VALUE
							? pkt->dts * (double)tb.num / tb.den
							: -1;
					if (pktSec < 0) continue;
					if (pktSec < startSec) continue;
					if (pktSec > endSec) break;
					result.Add(pktSec);
				}
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

		/// <summary>
		/// Returns a sublist of <paramref name="pts"/> with at most <paramref name="maxCount"/>
		/// entries, chosen by uniform stride so the selection is spread evenly across the range.
		/// </summary>
		internal static List<double> EvenlySubsample(List<double> pts, int maxCount) {
			if (pts.Count <= maxCount) return pts;
			var result = new List<double>(maxCount);
			double step = (double)(pts.Count - 1) / (maxCount - 1);
			for (int i = 0; i < maxCount; i++)
				result.Add(pts[(int)Math.Round(i * step)]);
			return result;
		}
	}
}
