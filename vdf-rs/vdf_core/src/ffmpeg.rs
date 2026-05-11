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
    pub has_audio: bool,
    pub audio_sample_rate: Option<u32>,
    pub audio_channels: Option<u32>,
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
    let tb = ffmpeg_the_third::ffi::AV_TIME_BASE as f64;
    let duration = if duration_tb == ffmpeg_the_third::ffi::AV_NOPTS_VALUE {
        // Fall back to stream duration
        let stream_dur = video_stream.duration();
        let time_base = video_stream.time_base();
        stream_dur as f64 * time_base.numerator() as f64 / time_base.denominator() as f64
    } else {
        duration_tb as f64 / tb
    };

    let params = video_stream.parameters();
    let w = params.width();
    let h = params.height();

    let (has_audio, sample_rate, channels) = if let Some(a) = audio_stream {
        let p = a.parameters();
        let sr = p.sample_rate();
        let ch = p.ch_layout().channels();
        (true, Some(sr), Some(ch))
    } else {
        (false, None, None)
    };

    let size = std::fs::metadata(path)?.len();

    Ok(MediaInfo {
        duration_secs: duration.max(0.0),
        width: w,
        height: h,
        video_codec: format!("{:?}", vcodec),
        has_audio,
        audio_sample_rate: sample_rate,
        audio_channels: channels,
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

    // Get effective end
    let ctx_dur = ictx.duration();
    let tb_base = ffmpeg_the_third::ffi::AV_TIME_BASE as f64;
    let duration_secs = if ctx_dur == ffmpeg_the_third::ffi::AV_NOPTS_VALUE {
        3600.0 // fallback
    } else {
        ctx_dur as f64 / tb_base
    };
    let effective_end = (duration_secs - skip_end_secs).max(skip_start_secs + 0.5);

    let mut results: BTreeMap<u64, GrayFrame> = BTreeMap::new();

    // Sort & clamp timestamps
    let mut sorted_ts: Vec<f64> = timestamps
        .iter()
        .map(|&t| t.clamp(skip_start_secs, effective_end))
        .collect();
    sorted_ts.sort_by(|a, b| a.partial_cmp(b).unwrap());
    sorted_ts.dedup_by(|a, b| (*a - *b).abs() < 0.1);

    for ts in sorted_ts {
        let ts_key = (ts * 1000.0) as u64;
        match decode_frame_at(&mut ictx, &mut decoder, video_stream_idx, time_base, ts) {
            Ok(frame) => {
                match frame_to_gray32(&frame) {
                    Ok(gray) => {
                        results.insert(ts_key, gray);
                        debug!("decoded frame at {ts:.2}s in {path}");
                    }
                    Err(e) => warn!("gray convert failed at {ts:.2}s in {path}: {e}"),
                }
            }
            Err(e) => warn!("decode failed at {ts:.2}s in {path}: {e}"),
        }
    }

    Ok(results)
}

/// Compute evenly-spaced sample timestamps within [skip_start, end-skip_end].
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
    let tb_base = ffmpeg_the_third::ffi::AV_TIME_BASE as f64;
    let duration_secs = if ctx_dur == ffmpeg_the_third::ffi::AV_NOPTS_VALUE {
        return Ok(vec![]);
    } else {
        ctx_dur as f64 / tb_base
    };

    let effective_end = (duration_secs - skip_end_secs).max(skip_start_secs + 0.5);
    drop(ictx);

    if effective_end <= skip_start_secs || interval_secs <= 0.0 {
        return Ok(vec![]);
    }

    let span = effective_end - skip_start_secs;
    let n_samples = ((span / interval_secs).ceil() as usize + 1).min(max_samples);
    let actual_interval = span / (n_samples.max(1) as f64);

    let mut timestamps: Vec<f64> = (0..n_samples)
        .map(|i| {
            let t = skip_start_secs + i as f64 * actual_interval;
            (t * 100.0).round() / 100.0
        })
        .collect();

    timestamps.retain(|&t| t >= skip_start_secs && t <= effective_end);
    timestamps.dedup_by(|a, b| (*a - *b).abs() < 0.05);

    Ok(timestamps)
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

    // Seek to nearest keyframe at or before ts
    let ts_av = (ts * ffmpeg_the_third::ffi::AV_TIME_BASE as f64) as i64;
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
                    // Keep decoding to get closer to target
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

    // Convert 32×32 RGB to grayscale (BT.601)
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gray_size_is_1024() {
        assert_eq!(GRAY_SIZE, 1024);
    }
}
