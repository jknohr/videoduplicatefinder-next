//! FFmpeg video/audio extraction using ffmpeg-the-third.
//!
//! All FFmpeg operations are synchronous (blocking). Call via
//! `tokio::task::spawn_blocking` from async contexts.

use crate::config::HardwareAccel;
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

// ─── Hardware acceleration ────────────────────────────────────────────────────

/// Map a `HardwareAccel` variant to the corresponding `AVHWDeviceType`.
///
/// Faithful port of `FfmpegEngine.GetConfiguredHardwareDeviceType()` from C#.
fn hw_device_type(accel: HardwareAccel) -> ffmpeg_sys_the_third::AVHWDeviceType {
    use ffmpeg_sys_the_third::AVHWDeviceType::*;
    match accel {
        HardwareAccel::None => AV_HWDEVICE_TYPE_NONE,
        // "auto" in the process-spawn path is handled by passing the string "auto"
        // to -hwaccel; for the native path we fall back to no hw acceleration since
        // av_hwdevice_ctx_create does not support an "auto" device type.
        HardwareAccel::Auto => AV_HWDEVICE_TYPE_NONE,
        HardwareAccel::Vdpau => AV_HWDEVICE_TYPE_VDPAU,
        HardwareAccel::Dxva2 => AV_HWDEVICE_TYPE_DXVA2,
        HardwareAccel::Vaapi => AV_HWDEVICE_TYPE_VAAPI,
        HardwareAccel::Qsv => AV_HWDEVICE_TYPE_QSV,
        HardwareAccel::Cuda => AV_HWDEVICE_TYPE_CUDA,
        HardwareAccel::VideoToolbox => AV_HWDEVICE_TYPE_VIDEOTOOLBOX,
        HardwareAccel::D3d11va => AV_HWDEVICE_TYPE_D3D11VA,
        HardwareAccel::Drm => AV_HWDEVICE_TYPE_DRM,
        HardwareAccel::OpenCl => AV_HWDEVICE_TYPE_OPENCL,
        HardwareAccel::MediaCodec => AV_HWDEVICE_TYPE_MEDIACODEC,
        HardwareAccel::Vulkan => AV_HWDEVICE_TYPE_VULKAN,
    }
}

/// The string that FFmpeg's `-hwaccel` flag accepts for this mode.
fn hw_accel_flag_str(accel: HardwareAccel) -> Option<&'static str> {
    match accel {
        HardwareAccel::None => None,
        HardwareAccel::Auto => Some("auto"),
        HardwareAccel::Vdpau => Some("vdpau"),
        HardwareAccel::Dxva2 => Some("dxva2"),
        HardwareAccel::Vaapi => Some("vaapi"),
        HardwareAccel::Qsv => Some("qsv"),
        HardwareAccel::Cuda => Some("cuda"),
        HardwareAccel::VideoToolbox => Some("videotoolbox"),
        HardwareAccel::D3d11va => Some("d3d11va"),
        HardwareAccel::Drm => Some("drm"),
        HardwareAccel::OpenCl => Some("opencl"),
        HardwareAccel::MediaCodec => Some("mediacodec"),
        HardwareAccel::Vulkan => Some("vulkan"),
    }
}

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
    hw_accel: HardwareAccel,
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

    // Attach hardware device context when hw_accel is configured.
    // Mirrors VideoStreamDecoder constructor passing AVHWDeviceType in C#.
    let hw_type = hw_device_type(hw_accel);
    if hw_type != ffmpeg_sys_the_third::AVHWDeviceType::AV_HWDEVICE_TYPE_NONE {
        unsafe {
            let mut hw_ctx: *mut ffmpeg_sys_the_third::AVBufferRef = std::ptr::null_mut();
            let ret = ffmpeg_sys_the_third::av_hwdevice_ctx_create(
                &mut hw_ctx,
                hw_type,
                std::ptr::null(),
                std::ptr::null_mut(),
                0,
            );
            if ret >= 0 {
                (*decoder.as_mut_ptr()).hw_device_ctx =
                    ffmpeg_sys_the_third::av_buffer_ref(hw_ctx);
                // Release the create-ref; the codec context holds its own ref now.
                ffmpeg_sys_the_third::av_buffer_unref(&mut hw_ctx);
            } else {
                warn!(
                    "hw_device_ctx_create failed for {:?} (code {}), falling back to software decode",
                    hw_accel, ret
                );
            }
        }
    }

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
                    // Download hw frame to system memory if needed before we
                    // inspect PTS (the pts field is accessible on both hw and sw frames).
                    let sw_frame = match ensure_sw_frame(std::mem::replace(
                        &mut frame,
                        VideoFrame::empty(),
                    )) {
                        Ok(f) => f,
                        Err(e) => {
                            warn!("hw frame download failed: {e}");
                            break;
                        }
                    };
                    let pts = sw_frame.pts().unwrap_or(0);
                    if pts >= target_pts.saturating_sub(2) {
                        best_frame = sw_frame;
                        break 'outer;
                    }
                    // Keep searching; restore empty frame for next receive_frame call.
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

/// Download a hardware-decoded frame to system memory, if needed.
///
/// When hw acceleration is active, decoded frames reside in GPU memory with a
/// pixel format such as `AV_PIX_FMT_CUDA` or `AV_PIX_FMT_VAAPI`. libswscale
/// cannot process these directly; `av_hwframe_transfer_data` copies to a CPU
/// frame using the surface's software pixel format (e.g. NV12, P010LE).
///
/// If the frame is already a software frame, returns it as-is.
pub fn ensure_sw_frame(frame: VideoFrame) -> VdfResult<VideoFrame> {
    // AVHWFramesContext is set on frames that live in GPU memory.
    let is_hw = unsafe { !(*frame.as_ptr()).hw_frames_ctx.is_null() };
    if !is_hw {
        return Ok(frame);
    }

    let mut sw = VideoFrame::empty();
    let ret = unsafe {
        ffmpeg_sys_the_third::av_hwframe_transfer_data(
            sw.as_mut_ptr(),
            frame.as_ptr(),
            0,
        )
    };
    if ret < 0 {
        return Err(VdfError::FfmpegGeneral {
            code: ret,
            msg: format!("av_hwframe_transfer_data failed (code {})", ret),
        });
    }
    // Copy presentation metadata that av_hwframe_transfer_data does not copy.
    unsafe {
        (*sw.as_mut_ptr()).pts = (*frame.as_ptr()).pts;
        (*sw.as_mut_ptr()).pkt_dts = (*frame.as_ptr()).pkt_dts;
        (*sw.as_mut_ptr()).best_effort_timestamp = (*frame.as_ptr()).best_effort_timestamp;
    }
    Ok(sw)
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

// ─── Temporal average hash (tblend) ──────────────────────────────────────────

/// Collapses a `window_secs`-second segment starting at `start_secs` into a
/// single 32×32 grayscale "average frame" by applying FFmpeg's
/// `tblend=all_mode=average` filter.
///
/// Mirrors `FfmpegEngine.ExtractTemporalAverageHash` from C#.
pub fn extract_temporal_average_hash(
    video_path: &Utf8Path,
    start_secs: f64,
    window_secs: f64,
    duration_secs: f64,
    hw_accel: HardwareAccel,
) -> Option<Box<[u8; GRAY_SIZE]>> {
    const N: usize = 32;

    if start_secs >= duration_secs {
        return None;
    }
    let actual_window = (window_secs).min(duration_secs - start_secs);
    if actual_window <= 0.0 {
        return None;
    }

    let ffmpeg = which_ffmpeg()?;

    let hw_flag = hw_accel_flag_str(hw_accel)
        .map(|s| format!("-hwaccel {s} "))
        .unwrap_or_default();

    let args = format!(
        "-hide_banner -loglevel quiet -nostdin \
         {hw_flag}-ss {start:.6} -t {win:.6} -i \"{path}\" \
         -vf \"tblend=all_mode=average,framestep=32767,scale={N}:{N}:flags=bicubic,format=gray\" \
         -frames:v 1 -f rawvideo pipe:1",
        hw_flag = hw_flag,
        start = start_secs,
        win = actual_window,
        path = video_path,
        N = N,
    );

    let output = std::process::Command::new(&ffmpeg)
        .args(shell_split(&args))
        .output()
        .ok()?;

    if output.stdout.len() == GRAY_SIZE {
        let mut buf = Box::new([0u8; GRAY_SIZE]);
        buf.copy_from_slice(&output.stdout);
        Some(buf)
    } else {
        warn!(
            "temporal_average_hash: expected {} bytes, got {} for {:?}",
            GRAY_SIZE,
            output.stdout.len(),
            video_path
        );
        None
    }
}

// ─── Thumbnail JPEG extraction ────────────────────────────────────────────────

/// Extract a single JPEG thumbnail from `video_path` at `position_secs`.
/// If `max_width > 0`, the image is resized to at most that width.
///
/// Mirrors `FfmpegEngine.ExtractThumbnailJpeg` from C#.
pub fn extract_thumbnail_jpeg(
    video_path: &Utf8Path,
    position_secs: f64,
    max_width: u32,
    hw_accel: HardwareAccel,
) -> Option<Vec<u8>> {
    let ffmpeg = which_ffmpeg()?;

    let hw_flag = hw_accel_flag_str(hw_accel)
        .map(|s| format!("-hwaccel {s} "))
        .unwrap_or_default();

    let scale_filter = if max_width > 0 {
        format!("-vf \"scale={}:-1:flags=bicubic\" ", max_width)
    } else {
        String::new()
    };

    let args = format!(
        "-hide_banner -loglevel quiet -nostdin \
         {hw_flag}-ss {pos:.6} -i \"{path}\" \
         {scale}-frames:v 1 -f image2pipe -vcodec mjpeg -q:v 2 pipe:1",
        hw_flag = hw_flag,
        pos = position_secs,
        path = video_path,
        scale = scale_filter,
    );

    let output = std::process::Command::new(&ffmpeg)
        .args(shell_split(&args))
        .output()
        .ok()?;

    if output.stdout.is_empty() {
        warn!("extract_thumbnail_jpeg: empty output for {:?}", video_path);
        None
    } else {
        Some(output.stdout)
    }
}

// ─── SSIM second-pass verification ───────────────────────────────────────────

/// Compute SSIM score between two video segments at given offsets using
/// `ffmpeg -lavfi [0][1]ssim=stats_file=-`.
/// Returns a value in `[0.0, 1.0]`, or `-1.0` on failure.
///
/// Mirrors `FfmpegEngine.ComputeSsimAtOffset` from C#.
pub fn compute_ssim_at_offset(
    path_a: &Utf8Path,
    offset_a: f64,
    path_b: &Utf8Path,
    offset_b: f64,
    window_secs: f64,
    hw_accel: HardwareAccel,
) -> f32 {
    let ffmpeg = match which_ffmpeg() {
        Some(f) => f,
        None => return -1.0,
    };

    let hw_flag = hw_accel_flag_str(hw_accel)
        .map(|s| format!("-hwaccel {s} "))
        .unwrap_or_default();

    let args = format!(
        "-hide_banner -loglevel info -nostdin \
         {hw_flag}-ss {oa:.3} -t {ws:.3} -i \"{pa}\" \
         {hw_flag}-ss {ob:.3} -t {ws:.3} -i \"{pb}\" \
         -lavfi \"[0][1]ssim=stats_file=-\" -f null -",
        hw_flag = hw_flag,
        oa = offset_a,
        ob = offset_b,
        ws = window_secs,
        pa = path_a,
        pb = path_b,
    );

    let output = match std::process::Command::new(&ffmpeg)
        .args(shell_split(&args))
        .output()
    {
        Ok(o) => o,
        Err(e) => {
            warn!("compute_ssim_at_offset failed: {}", e);
            return -1.0;
        }
    };

    // SSIM stats are written to stdout (stats_file=-); "All:X.XXXXXX" appears in each line.
    let combined = String::from_utf8_lossy(&output.stderr);
    let mut ssim = -1.0f32;
    for line in combined.lines() {
        if let Some(idx) = line.find("All:") {
            let start = idx + 4;
            let val_str = &line[start..];
            let end = val_str.find(' ').unwrap_or(val_str.len());
            if let Ok(v) = val_str[..end].parse::<f32>() {
                ssim = v;
            }
        }
    }
    ssim
}

// ─── Scene-change detection ───────────────────────────────────────────────────

/// Detect the first `max_count` scene transitions whose score exceeds `threshold`.
/// Returns timestamps in seconds.
///
/// Mirrors `FfmpegEngine.GetSceneChangeTimestamps` from C#.
pub fn get_scene_change_timestamps(
    video_path: &Utf8Path,
    threshold: f32,
    max_count: usize,
    hw_accel: HardwareAccel,
) -> Vec<f64> {
    let ffmpeg = match which_ffmpeg() {
        Some(f) => f,
        None => return vec![],
    };

    let hw_flag = hw_accel_flag_str(hw_accel)
        .map(|s| format!("-hwaccel {s} "))
        .unwrap_or_default();

    let args = format!(
        "-hide_banner -loglevel info -nostdin {hw_flag}-i \"{path}\" \
         -vf \"select='gt(scene,{thresh:.2})',showinfo\" \
         -vsync vfr -frames:v {max} -f null -",
        hw_flag = hw_flag,
        path = video_path,
        thresh = threshold,
        max = max_count,
    );

    let output = match std::process::Command::new(&ffmpeg)
        .args(shell_split(&args))
        .output()
    {
        Ok(o) => o,
        Err(e) => {
            warn!("get_scene_change_timestamps failed: {}", e);
            return vec![];
        }
    };

    let mut result = Vec::with_capacity(max_count);
    let stderr = String::from_utf8_lossy(&output.stderr);
    for line in stderr.lines() {
        if let Some(idx) = line.find("pts_time:") {
            let start = idx + 9;
            let rest = &line[start..];
            let end = rest.find(' ').unwrap_or(rest.len());
            if let Ok(t) = rest[..end].parse::<f64>() {
                result.push(t);
                if result.len() >= max_count {
                    break;
                }
            }
        }
    }
    result
}

// ─── Metadata tag writing ─────────────────────────────────────────────────────

/// Write metadata tags to a video file by remuxing with `-c copy`.
/// Uses a temp file + rename to avoid data loss on error.
///
/// Returns `(true, None)` on success, `(false, Some(reason))` on failure.
///
/// Mirrors `FfmpegEngine.WriteMetadataTags` from C#.
/// Read container-level metadata tags from a media file using `ffprobe`.
///
/// Mirrors `FFProbeEngine.GetMetadataTags` from C#.
/// Excludes noise keys (`encoder`, `handler_name`, `vendor_id`).
/// Returns an empty map if ffprobe is not found or the file has no tags.
pub fn read_metadata_tags(path: &Utf8Path) -> std::collections::HashMap<String, String> {
    const EXCLUDED: &[&str] = &["encoder", "handler_name", "vendor_id"];
    let mut result = std::collections::HashMap::new();

    let ffprobe = match which_ffprobe() {
        Some(p) => p,
        None => return result,
    };
    if !path.exists() {
        return result;
    }

    let args = [
        "-v", "quiet",
        "-print_format", "json",
        "-show_entries", "format_tags",
        &long_path_fix(path.as_str()),
    ];

    let output = match std::process::Command::new(&ffprobe).args(args).output() {
        Ok(o) => o,
        Err(_) => return result,
    };
    if output.stdout.is_empty() {
        return result;
    }

    if let Ok(doc) = serde_json::from_slice::<serde_json::Value>(&output.stdout) {
        if let Some(tags) = doc.get("format").and_then(|f| f.get("tags")).and_then(|t| t.as_object()) {
            for (k, v) in tags {
                let key = k.to_lowercase();
                if EXCLUDED.iter().any(|ex| *ex == key) {
                    continue;
                }
                if let Some(s) = v.as_str() {
                    result.insert(key, s.to_string());
                }
            }
        }
    }
    result
}

pub fn write_metadata_tags(
    path: &Utf8Path,
    tags: &std::collections::HashMap<String, String>,
) -> (bool, Option<String>) {
    let ffmpeg = match which_ffmpeg() {
        Some(f) => f,
        None => return (false, Some("ffmpeg not found".to_string())),
    };

    if !path.exists() {
        return (false, Some(format!("file not found: {}", path)));
    }

    let ext = path.extension().unwrap_or("");
    let tmp = path.with_extension(format!("vdf_meta_tmp.{}", ext));

    let mut args: Vec<String> = vec![
        "-hide_banner".into(),
        "-loglevel".into(), "quiet".into(),
        "-y".into(),
        "-i".into(), path.to_string(),
        "-c".into(), "copy".into(),
        "-map_metadata".into(), "0".into(),
    ];
    for (k, v) in tags {
        args.push("-metadata".into());
        args.push(format!("{}={}", k, v));
    }
    args.push(tmp.to_string());

    match std::process::Command::new(&ffmpeg).args(&args).output() {
        Ok(out) if out.status.success() => {
            match std::fs::rename(tmp.as_std_path(), path.as_std_path()) {
                Ok(_) => (true, None),
                Err(e) => {
                    let _ = std::fs::remove_file(tmp.as_std_path());
                    (false, Some(e.to_string()))
                }
            }
        }
        Ok(out) => {
            let _ = std::fs::remove_file(tmp.as_std_path());
            let stderr = String::from_utf8_lossy(&out.stderr).trim().to_string();
            (false, Some(if stderr.is_empty() {
                format!("ffmpeg exited with code {:?}", out.status.code())
            } else {
                stderr
            }))
        }
        Err(e) => {
            let _ = std::fs::remove_file(tmp.as_std_path());
            (false, Some(e.to_string()))
        }
    }
}

// ─── Windows long-path fix ────────────────────────────────────────────────────

/// Prefix a path with `\\?\` (or `\\?\UNC\` for UNC paths) on Windows to bypass
/// the 260-character MAX_PATH limit.  No-op on other platforms.
///
/// Mirrors `FFToolsUtils.LongPathFix` from C#.
pub fn long_path_fix(path: &str) -> String {
    #[cfg(target_os = "windows")]
    {
        if path.starts_with('\\') {
            return format!("\\\\?\\UNC\\{}", path.trim_start_matches('\\'));
        }
        return format!("\\\\?\\{}", path);
    }
    #[cfg(not(target_os = "windows"))]
    path.to_string()
}

// ─── Shell-split helper ───────────────────────────────────────────────────────

/// Naïve shell argument split for the simple single-quoted / space-separated
/// argument strings we build for FFmpeg subprocesses.
fn shell_split(args: &str) -> Vec<String> {
    let mut result = Vec::new();
    let mut current = String::new();
    let mut in_quotes = false;
    let mut quote_char = ' ';

    for c in args.chars() {
        match c {
            '"' | '\'' if !in_quotes => {
                in_quotes = true;
                quote_char = c;
            }
            c if in_quotes && c == quote_char => {
                in_quotes = false;
            }
            ' ' if !in_quotes => {
                if !current.is_empty() {
                    result.push(current.clone());
                    current.clear();
                }
            }
            _ => current.push(c),
        }
    }
    if !current.is_empty() {
        result.push(current);
    }
    result
}

/// Find the `ffmpeg` binary. Checks:
/// 1. `bin/` subfolder next to the executable (portable installs)
/// 2. Same folder as the executable
/// 3. PATH
pub fn which_ffmpeg() -> Option<std::path::PathBuf> {
    find_tool("ffmpeg")
}

/// Find the `ffprobe` binary using the same search order as `which_ffmpeg`.
pub fn which_ffprobe() -> Option<std::path::PathBuf> {
    find_tool("ffprobe")
}

fn find_tool(name: &str) -> Option<std::path::PathBuf> {
    let exe_name = if cfg!(windows) { format!("{}.exe", name) } else { name.to_string() };

    // 1. bin/ next to the current executable
    if let Some(exe_dir) = std::env::current_exe().ok().and_then(|p| p.parent().map(|d| d.to_path_buf())) {
        let candidate = exe_dir.join("bin").join(&exe_name);
        if candidate.is_file() {
            return Some(candidate);
        }
        // 2. Same directory as the executable
        let candidate = exe_dir.join(&exe_name);
        if candidate.is_file() {
            return Some(candidate);
        }
    }

    // 3. PATH
    std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default())
        .map(|dir| dir.join(&exe_name))
        .find(|p| p.is_file())
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

    #[test]
    fn shell_split_basic() {
        let parts = shell_split("-i \"my file.mp4\" -f null -");
        assert_eq!(parts, ["-i", "my file.mp4", "-f", "null", "-"]);
    }
}
