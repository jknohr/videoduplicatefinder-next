//! FFmpeg video/audio extraction using ffmpeg-the-third.
//!
//! All FFmpeg operations are synchronous (blocking). Call via
//! `tokio::task::spawn_blocking` from async contexts.

use crate::error::{VdfError, VdfResult};
use camino::Utf8Path;
use fast_image_resize::images::Image;
use fast_image_resize::{PixelType, ResizeAlg, ResizeOptions, Resizer};
use ffmpeg_the_third as ffmpeg;
use ffmpeg_the_third::{
    codec, codec::decoder, format, media,
    packet::Ref as PacketRef,
    software::scaling::{context::Context as SwsContext, flag::Flags as SwsFlags},
    util::frame::video::Video as VideoFrame,
};
use std::collections::BTreeMap;
use tracing::{debug, warn};

const GRAY_SIZE: usize = 32 * 32;

/// Decoded 32×32 grayscale frame.
pub type GrayFrame = Box<[u8; GRAY_SIZE]>;

/// Media info extracted from a file.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct MediaInfo {
    pub duration_secs: f64,
    pub width: u32,
    pub height: u32,
    pub video_codec: String,
    pub bit_rate: i64,
    pub frame_rate: f64,
    pub has_audio: bool,
    pub audio_codec: Option<String>,
    pub audio_sample_rate: Option<u32>,
    pub audio_channels: Option<u32>,
    pub audio_bit_rate: Option<i64>,
    pub pixel_format: Option<String>,
    pub size_bytes: u64,
}

/// Extract media information from a file without decoding frames.
pub fn probe_media(path: &Utf8Path) -> VdfResult<MediaInfo> {
    ffmpeg::init().ok();

    let ctx = format::input(&path.as_std_path())
        .map_err(|e| VdfError::FfmpegGeneral { code: -1, msg: e.to_string() })?;

    let video_stream = ctx
        .streams()
        .best(media::Type::Video)
        .ok_or_else(|| VdfError::NoVideoStream { path: path.to_owned() })?;

    let audio_stream = ctx.streams().best(media::Type::Audio);

    let vcodec = video_stream.parameters().id();
    let duration_tb = ctx.duration();
    let av_tb = ffmpeg_the_third::ffi::AV_TIME_BASE as f64;
    let duration = if duration_tb == ffmpeg_the_third::ffi::AV_NOPTS_VALUE {
        // Fall back to stream duration
        let stream_dur = video_stream.duration();
        let time_base = video_stream.time_base();
        stream_dur as f64 * time_base.numerator() as f64 / time_base.denominator() as f64
    } else {
        duration_tb as f64 / av_tb
    };

    let params = video_stream.parameters();
    let w = params.width();
    let h = params.height();

    // Bit rate from container-level, fall back to stream
    let bit_rate = ctx.bit_rate();

    // Frame rate from video stream
    let frame_rate = {
        let tb = video_stream.time_base();
        let avg_fr = video_stream.avg_frame_rate();
        if avg_fr.denominator() != 0 {
            avg_fr.numerator() as f64 / avg_fr.denominator() as f64
        } else if tb.denominator() != 0 {
            tb.denominator() as f64 / tb.numerator() as f64
        } else {
            0.0
        }
    };

    // Pixel format via raw AVCodecParameters.format field (transmute i32 → AVPixelFormat)
    let pixel_format: Option<String> = unsafe {
        let raw = params.as_ptr() as *const ffmpeg_sys_the_third::AVCodecParameters;
        if !raw.is_null() && (*raw).format >= 0 {
            let avfmt: ffmpeg_sys_the_third::AVPixelFormat =
                std::mem::transmute::<i32, ffmpeg_sys_the_third::AVPixelFormat>((*raw).format);
            let pix = ffmpeg_the_third::format::Pixel::from(avfmt);
            Some(format!("{pix:?}"))
        } else {
            None
        }
    };

    let (has_audio, audio_codec, sample_rate, channels, audio_bit_rate) =
        if let Some(a) = audio_stream {
            let p = a.parameters();
            let acodec = format!("{:?}", p.id());
            let sr = p.sample_rate();
            let ch = p.ch_layout().channels();
            let abr = a.parameters().bit_rate();
            (true, Some(acodec), Some(sr), Some(ch), Some(abr))
        } else {
            (false, None, None, None, None)
        };

    let size = std::fs::metadata(path)?.len();

    Ok(MediaInfo {
        duration_secs: duration.max(0.0),
        width: w,
        height: h,
        video_codec: format!("{vcodec:?}"),
        bit_rate,
        frame_rate,
        has_audio,
        audio_codec,
        audio_sample_rate: sample_rate,
        audio_channels: channels,
        audio_bit_rate,
        pixel_format,
        size_bytes: size,
    })
}

/// Extract pHash-ready 32×32 grayscale frames at a set of timestamps (seconds).
///
/// Returns a map from timestamp_ms → gray frame.
/// Timestamps that fail to decode are omitted (logged at WARN).
pub fn extract_gray_frames(
    path: &Utf8Path,
    timestamps: &[f64],
    skip_start_secs: f64,
    skip_end_secs: f64,
) -> VdfResult<BTreeMap<u64, GrayFrame>> {
    if timestamps.is_empty() {
        return Ok(BTreeMap::new());
    }

    ffmpeg::init().ok();

    let mut ictx = format::input(&path.as_std_path())
        .map_err(|e| VdfError::FfmpegGeneral { code: -1, msg: e.to_string() })?;

    let video_stream_idx = ictx
        .streams()
        .best(media::Type::Video)
        .map(|s| s.index())
        .ok_or_else(|| VdfError::NoVideoStream { path: path.to_owned() })?;

    // Create decoder and capture time_base within a borrow scope so the
    // borrow of `ictx` ends before we start iterating packets below.
    let (mut decoder, time_base) = {
        let stream = ictx.stream(video_stream_idx).unwrap();
        let dec = codec::Context::from_parameters(stream.parameters())
            .map_err(|e| VdfError::FfmpegGeneral { code: -1, msg: e.to_string() })?
            .decoder()
            .video()
            .map_err(|e| VdfError::FfmpegGeneral { code: -1, msg: e.to_string() })?;
        let tb = stream.time_base();
        (dec, tb)
    };

    let ctx_dur = ictx.duration();
    let av_tb = ffmpeg_the_third::ffi::AV_TIME_BASE as f64;
    let duration_secs = if ctx_dur == ffmpeg_the_third::ffi::AV_NOPTS_VALUE {
        3600.0
    } else {
        ctx_dur as f64 / av_tb
    };
    let effective_end = (duration_secs - skip_end_secs).max(skip_start_secs + 0.5);

    let mut results: BTreeMap<u64, GrayFrame> = BTreeMap::new();

    let mut sorted_ts: Vec<f64> = timestamps
        .iter()
        .map(|&t| t.clamp(skip_start_secs, effective_end))
        .collect();
    sorted_ts.sort_by(|a, b| a.partial_cmp(b).unwrap());
    sorted_ts.dedup_by(|a, b| (*a - *b).abs() < 0.1);

    for ts in sorted_ts {
        let ts_key = (ts * 1000.0) as u64;
        match decode_frame_at(&mut ictx, &mut decoder, video_stream_idx, time_base, ts) {
            Ok(frame) => match frame_to_gray32(&frame) {
                Ok(gray) => {
                    results.insert(ts_key, gray);
                    debug!("decoded frame at {ts:.2}s in {path}");
                }
                Err(e) => warn!("gray convert failed at {ts:.2}s in {path}: {e}"),
            },
            Err(e) => warn!("decode failed at {ts:.2}s in {path}: {e}"),
        }
    }

    Ok(results)
}

/// Extract actual I-frame (keyframe) timestamps by seeking to evenly-spaced target
/// positions and recording the PTS of the first keyframe packet found after each seek.
///
/// Faithful port of `IFrameExtractor.GetKeyframePtsByInterval` from C#.
/// Seeking rather than sequential reading is critical: sequential reads hit the sample
/// cap near the start of long videos, leaving the rest completely unsampled.
pub fn get_keyframe_timestamps_by_interval(
    path: &Utf8Path,
    start_secs: f64,
    end_secs: f64,
    max_samples: usize,
    interval_secs: f64,
) -> VdfResult<Vec<f64>> {
    let window = end_secs - start_secs;
    if window <= 0.0 || max_samples == 0 {
        return Ok(vec![]);
    }

    // Determine desired sample count and spacing, matching C# logic exactly.
    let (desired_count, actual_interval) = if interval_secs > 0.0 {
        let count = max_samples
            .min(((window / interval_secs).ceil() as usize).max(1));
        (count, window / count as f64)
    } else {
        let count = max_samples;
        let interval = window / count.saturating_sub(1).max(1) as f64;
        (count, interval)
    };

    ffmpeg::init().ok();

    let mut ictx = format::input(&path.as_std_path())
        .map_err(|e| VdfError::FfmpegGeneral { code: -1, msg: e.to_string() })?;

    let video_stream_idx = ictx
        .streams()
        .best(media::Type::Video)
        .map(|s| s.index())
        .ok_or_else(|| VdfError::NoVideoStream { path: path.to_owned() })?;

    // Capture time_base before any mutable borrow of ictx.
    let time_base = {
        let stream = ictx.stream(video_stream_idx).unwrap();
        stream.time_base()
    };
    let tb_num = time_base.numerator() as f64;
    let tb_den = time_base.denominator() as f64;
    let av_tb = ffmpeg_the_third::ffi::AV_TIME_BASE as f64;

    let mut result = Vec::with_capacity(desired_count);
    let mut last_recorded = f64::MIN;

    for i in 0..desired_count {
        // Target timestamp for this sample slot (same formula as C#)
        let target_sec = if desired_count == 1 {
            start_secs + window * 0.5
        } else {
            start_secs + actual_interval * i as f64
        };
        if target_sec > end_secs {
            break;
        }

        // Seek backward to the nearest keyframe at or before target_sec.
        // Use stream-specific PTS units (tb_den/tb_num), matching av_seek_frame in C#.
        let target_pts_stream = if tb_num > 0.0 {
            (target_sec * tb_den / tb_num) as i64
        } else {
            (target_sec * av_tb) as i64
        };

        // Try stream-level seek first (AVSEEK_FLAG_BACKWARD via raw FFI)
        let seek_ok = unsafe {
            let ret = ffmpeg_the_third::ffi::av_seek_frame(
                ictx.as_mut_ptr(),
                video_stream_idx as i32,
                target_pts_stream,
                ffmpeg_the_third::ffi::AVSEEK_FLAG_BACKWARD as i32,
            );
            ret >= 0
        };

        if !seek_ok {
            // Fallback: container-level seek in AV_TIME_BASE units
            let ts_av = (target_sec * av_tb) as i64;
            if ictx.seek(ts_av, ..=ts_av).is_err() {
                continue;
            }
        }

        // Read up to 64 packets looking for a keyframe on the video stream.
        let keyframe_sec: Option<f64> = 'search: {
            for pkt_result in ictx.packets().take(64) {
                let (stream, packet) = match pkt_result {
                    Ok(p) => p,
                    Err(_) => break 'search None,
                };

                if stream.index() != video_stream_idx {
                    continue;
                }

                // Check AV_PKT_FLAG_KEY via the raw flags integer.
                let is_key = unsafe {
                    (*packet.as_ptr()).flags & ffmpeg_the_third::ffi::AV_PKT_FLAG_KEY as i32 != 0
                };
                if !is_key {
                    continue;
                }

                // Convert PTS (or fallback DTS) to seconds using stream time_base.
                let pts_raw = packet.pts().or_else(|| packet.dts());
                let sec = match pts_raw {
                    Some(p) if p != ffmpeg_the_third::ffi::AV_NOPTS_VALUE => {
                        p as f64 * tb_num / tb_den
                    }
                    _ => break 'search None,
                };

                break 'search Some(sec);
            }
            None
        };

        if let Some(sec) = keyframe_sec {
            // Bounds + dedup check matching C#
            if sec < start_secs || sec > end_secs {
                continue;
            }
            if (sec - last_recorded).abs() < 0.1 {
                continue;
            }
            result.push(sec);
            last_recorded = sec;
        }
    }

    Ok(result)
}

/// Compute evenly-spaced sample timestamps within [skip_start, end-skip_end].
///
/// This is used for regular pHash thumbnail extraction — NOT I-frame seeking.
/// For I-frame timeline extraction, use `get_keyframe_timestamps_by_interval`.
pub fn extract_iframe_timestamps(
    path: &Utf8Path,
    interval_secs: f64,
    skip_start_secs: f64,
    skip_end_secs: f64,
    max_samples: usize,
) -> VdfResult<Vec<f64>> {
    ffmpeg::init().ok();

    let ictx = format::input(&path.as_std_path())
        .map_err(|e| VdfError::FfmpegGeneral { code: -1, msg: e.to_string() })?;

    let ctx_dur = ictx.duration();
    let av_tb = ffmpeg_the_third::ffi::AV_TIME_BASE as f64;
    let duration_secs = if ctx_dur == ffmpeg_the_third::ffi::AV_NOPTS_VALUE {
        return Ok(vec![]);
    } else {
        ctx_dur as f64 / av_tb
    };
    drop(ictx);

    let effective_end = (duration_secs - skip_end_secs).max(skip_start_secs + 0.5);

    if effective_end <= skip_start_secs {
        return Ok(vec![]);
    }

    get_keyframe_timestamps_by_interval(
        path,
        skip_start_secs,
        effective_end,
        max_samples,
        interval_secs,
    )
}

// ---------------------------------------------------------------------------
// Internal decode helpers
// ---------------------------------------------------------------------------

fn decode_frame_at(
    ictx: &mut format::context::Input,
    decoder: &mut decoder::Video,
    stream_idx: usize,
    time_base: ffmpeg_the_third::Rational,
    ts: f64,
) -> VdfResult<VideoFrame> {
    let tb_num = time_base.numerator() as f64;
    let tb_den = time_base.denominator() as f64;
    let tb_secs = if tb_den > 0.0 { tb_num / tb_den } else { 1.0 / 25.0 };

    let av_tb = ffmpeg_the_third::ffi::AV_TIME_BASE as f64;
    let ts_av = (ts * av_tb) as i64;
    ictx.seek(ts_av, ..=ts_av).map_err(|_| VdfError::SeekFailed {
        path: "".into(),
        seek_secs: ts,
    })?;

    decoder.flush();

    let target_pts = if tb_secs > 0.0 { (ts / tb_secs) as i64 } else { 0 };

    let mut best_frame = VideoFrame::empty();
    let mut found = false;

    'outer: for result in ictx.packets() {
        let (stream, packet) = match result {
            Ok(p) => p,
            Err(_) => continue,
        };
        if stream.index() != stream_idx {
            continue;
        }
        if decoder.send_packet(&packet).is_err() {
            continue;
        }
        let mut frame = VideoFrame::empty();
        loop {
            match decoder.receive_frame(&mut frame) {
                Ok(()) => {
                    found = true;
                    let pts = frame.pts().unwrap_or(0);
                    if pts >= target_pts.saturating_sub(2) {
                        best_frame = frame;
                        break 'outer;
                    }
                }
                Err(ffmpeg_the_third::Error::Other { errno: e })
                    if e == ffmpeg_the_third::ffi::AVERROR(libc::EAGAIN) =>
                {
                    break;
                }
                Err(_) => break 'outer,
            }
        }
    }

    if !found && best_frame.width() == 0 {
        return Err(VdfError::DecodeTimeout { path: "".into() });
    }

    Ok(best_frame)
}

/// Convert a decoded VideoFrame to a 32×32 grayscale image.
pub fn frame_to_gray32(frame: &VideoFrame) -> VdfResult<GrayFrame> {
    let src_w = frame.width();
    let src_h = frame.height();

    if src_w == 0 || src_h == 0 {
        return Err(VdfError::FfmpegGeneral { code: -1, msg: "zero-size frame".into() });
    }

    // Convert to RGB24 via libswscale
    let mut sws = SwsContext::get(
        frame.format(),
        src_w,
        src_h,
        ffmpeg_the_third::format::Pixel::RGB24,
        src_w,
        src_h,
        SwsFlags::BILINEAR,
    )
    .map_err(|e| VdfError::FfmpegGeneral { code: -1, msg: e.to_string() })?;

    let mut rgb_frame = VideoFrame::new(ffmpeg_the_third::format::Pixel::RGB24, src_w, src_h);
    sws.run(frame, &mut rgb_frame)
        .map_err(|e| VdfError::FfmpegGeneral { code: -1, msg: e.to_string() })?;

    // Build contiguous RGB buffer (strip stride padding)
    let rgb_data = rgb_frame.data(0);
    let stride = rgb_frame.stride(0);
    let row_bytes = src_w as usize * 3;
    let mut rgb_flat = Vec::with_capacity(src_w as usize * src_h as usize * 3);
    for row in 0..src_h as usize {
        let start = row * stride;
        rgb_flat.extend_from_slice(&rgb_data[start..start + row_bytes]);
    }

    // Resize RGB to 32×32 via fast_image_resize (SIMD)
    let src_image = Image::from_vec_u8(src_w, src_h, rgb_flat, PixelType::U8x3)
        .map_err(|e| VdfError::FfmpegGeneral { code: -1, msg: e.to_string() })?;
    let mut dst_image = Image::new(32, 32, PixelType::U8x3);
    let mut resizer = Resizer::new();
    resizer
        .resize(&src_image, &mut dst_image, &ResizeOptions::new().resize_alg(ResizeAlg::Nearest))
        .map_err(|e| VdfError::FfmpegGeneral { code: -1, msg: e.to_string() })?;

    // Convert 32×32 RGB to grayscale (BT.601 luma coefficients)
    let rgb32 = dst_image.buffer();
    let mut gray = Box::new([0u8; GRAY_SIZE]);
    for i in 0..GRAY_SIZE {
        let r = rgb32[i * 3] as u32;
        let g = rgb32[i * 3 + 1] as u32;
        let b = rgb32[i * 3 + 2] as u32;
        gray[i] = ((r * 299 + g * 587 + b * 114) / 1000) as u8;
    }

    Ok(gray)
}

/// Produce a horizontally-flipped copy of a 32×32 gray frame.
///
/// Used by the `compare_horizontally_flipped` scan option to catch mirror-image duplicates.
pub fn flip_gray_horizontal(gray: &[u8; 1024]) -> Box<[u8; 1024]> {
    let mut flipped = Box::new([0u8; 1024]);
    for row in 0..32 {
        for col in 0..32 {
            flipped[row * 32 + col] = gray[row * 32 + (31 - col)];
        }
    }
    flipped
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gray_size_is_1024() {
        assert_eq!(GRAY_SIZE, 1024);
    }

    #[test]
    fn flip_gray_is_involution() {
        let mut src = Box::new([0u8; 1024]);
        for i in 0..1024usize {
            src[i] = (i % 256) as u8;
        }
        let flipped = flip_gray_horizontal(&src);
        let restored = flip_gray_horizontal(&flipped);
        assert_eq!(&*restored, &*src);
    }
}
