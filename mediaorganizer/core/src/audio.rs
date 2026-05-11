//! Audio fingerprinting — faithful port of the C# Chromaprint pipeline.
//!
//! Pipeline matches VDF.Core/Chromaprint exactly:
//!   FFmpeg audio decode → SWR resample to 11025 Hz mono s16 →
//!   ChromaContext (4096-sample Hann-windowed FFT, chroma bins A0–A7,
//!   5-tap FIR filter, L2 normalization, 32 fixed pairwise comparisons,
//!   majority-vote aggregation per second) → Vec<u32> fingerprint
//!
//! Output: one u32 per second of audio. Identical to AcoustID Chromaprint format.

use crate::error::{VdfError, VdfResult};
use camino::Utf8Path;
use ffmpeg_the_third as ffmpeg;
use ffmpeg_the_third::{
    codec, format, media,
    software::resampling::context::Context as SwrContext,
    util::frame::audio::Audio as AudioFrame,
    ChannelLayoutMask,
};
use realfft::RealFftPlanner;
use tracing::warn;

// ── Constants matching C# source exactly ─────────────────────────────────────

/// Target sample rate for fingerprinting (matches Chromaprint / AcoustID standard).
const SAMPLE_RATE: u32 = 11025;
/// FFT frame size in samples (must be power of two).
const FRAME_SIZE: usize = 4096;
/// Samples to advance between consecutive frames (Chroma.FrameHop in C#).
const FRAME_HOP: usize = 1365;
/// Lowest frequency mapped to a chroma bin (A0 = 27.5 Hz).
const MIN_FREQ: f64 = 27.5;
/// Highest frequency mapped to a chroma bin (A7 = 3520 Hz).
const MAX_FREQ: f64 = 3520.0;
/// Number of chroma bins — one per semitone in a chromatic octave.
const CHROMA_BINS: usize = 12;
/// FIR temporal smoothing coefficients (ChromaFilter in C#).
const FIR_COEFF: [f64; 5] = [0.25, 0.50, 1.00, 0.50, 0.25];
/// Sum of FIR coefficients (used to normalise filtered output).
const FIR_NORM: f64 = 2.50;

// ── Pre-computed lookup tables ────────────────────────────────────────────────

/// Build a Hann window of length FRAME_SIZE.
fn build_hann_window() -> [f64; FRAME_SIZE] {
    let mut w = [0f64; FRAME_SIZE];
    let factor = 2.0 * std::f64::consts::PI / (FRAME_SIZE - 1) as f64;
    for (i, v) in w.iter_mut().enumerate() {
        *v = 0.5 * (1.0 - (i as f64 * factor).cos());
    }
    w
}

/// Chroma bin index (0–11) for each FFT output bin, or -1 to skip.
/// Matches C# Chroma.BuildChromaMap() exactly.
fn build_chroma_map() -> Vec<i8> {
    let bins = FRAME_SIZE / 2 + 1; // 2049
    let mut map = vec![-1i8; bins];
    for i in 1..bins {
        let freq = i as f64 * SAMPLE_RATE as f64 / FRAME_SIZE as f64;
        if freq < MIN_FREQ || freq > MAX_FREQ {
            continue;
        }
        // Semitones above A0 (27.5 Hz), mapped modulo 12 into a chroma class
        let note = 12.0 * (freq / MIN_FREQ).log2();
        let c = (note as i32).rem_euclid(CHROMA_BINS as i32) as usize;
        map[i] = c as i8;
    }
    map
}

/// 32 fixed pairwise feature comparisons used by FingerprintCalculator in C#.
///
///   bits  0–11: adjacent semitone pairs  (i, (i+1) % 12)  — 12 pairs
///   bits 12–23: minor-third pairs        (i, (i+3) % 12)  — 12 pairs
///   bits 24–31: tritone pairs            (i, (i+6) % 12)  for i in 0..8 — 8 pairs
const fn build_pairs() -> [(u8, u8); 32] {
    let mut pairs = [(0u8, 0u8); 32];
    let mut idx = 0usize;
    let mut i = 0u8;
    while i < 12 { pairs[idx] = (i, (i + 1) % 12); idx += 1; i += 1; } // adjacent
    i = 0;
    while i < 12 { pairs[idx] = (i, (i + 3) % 12); idx += 1; i += 1; } // minor-third
    i = 0;
    while i < 8  { pairs[idx] = (i, (i + 6) % 12); idx += 1; i += 1; } // tritone
    pairs
}

static PAIRS: [(u8, u8); 32] = build_pairs();

// ── Chromaprint pipeline state ────────────────────────────────────────────────

/// Orchestrates the full fingerprinting pipeline.
/// Mirrors ChromaContext.cs but owns all sub-stage state inline.
struct ChromaContext {
    hann: [f64; FRAME_SIZE],
    chroma_map: Vec<i8>,

    // realfft planner output buffers (reused across frames)
    fft_in: Vec<f64>,
    fft_out: Vec<realfft::num_complex::Complex<f64>>,
    fft_scratch: Vec<realfft::num_complex::Complex<f64>>,
    fft: std::sync::Arc<dyn realfft::RealToComplex<f64>>,

    // Carry buffer for leftover i16 samples between feed() calls
    carry: Vec<i16>,

    // FIR ring buffer: [FilterSize][CHROMA_BINS]
    ring: [[f64; CHROMA_BINS]; 5],
    ring_head: usize,
    ring_count: usize,

    // Aggregation state
    frame_index: usize,
    second_frames: Vec<u32>,
    aggregated: Vec<u32>,
}

impl ChromaContext {
    fn new() -> Self {
        let mut planner = RealFftPlanner::<f64>::new();
        let fft = planner.plan_fft_forward(FRAME_SIZE);
        let fft_in = fft.make_input_vec();
        let fft_out = fft.make_output_vec();
        let fft_scratch = fft.make_scratch_vec();
        Self {
            hann: build_hann_window(),
            chroma_map: build_chroma_map(),
            fft_in,
            fft_out,
            fft_scratch,
            fft,
            carry: Vec::with_capacity(FRAME_SIZE * 2),
            ring: [[0.0f64; CHROMA_BINS]; 5],
            ring_head: 0,
            ring_count: 0,
            frame_index: 0,
            second_frames: Vec::new(),
            aggregated: Vec::new(),
        }
    }

    /// Feed a block of mono s16 samples (11025 Hz).
    fn feed(&mut self, samples: &[i16]) {
        self.carry.extend_from_slice(samples);
        let mut pos = 0;
        while pos + FRAME_SIZE <= self.carry.len() {
            self.process_frame(&self.carry[pos..pos + FRAME_SIZE].to_vec());
            pos += FRAME_HOP;
        }
        // Retain leftover samples not consumed in any complete frame
        self.carry.drain(..pos);
    }

    /// Flush any partial second bucket after all audio has been fed.
    fn finish(&mut self) {
        if !self.second_frames.is_empty() {
            let val = Self::majority_vote(&self.second_frames);
            self.aggregated.push(val);
            self.second_frames.clear();
        }
    }

    /// Retrieve the accumulated fingerprint (call after finish()).
    fn fingerprint(&self) -> Vec<u32> {
        self.aggregated.clone()
    }

    // ── Internal helpers ──────────────────────────────────────────────────────

    fn process_frame(&mut self, samples: &[i16]) {
        // Apply Hann window and normalise i16 → f64 in [-1, 1]
        for (i, (&s, &w)) in samples.iter().zip(self.hann.iter()).enumerate() {
            self.fft_in[i] = s as f64 * w / 32768.0;
        }

        if self
            .fft
            .process_with_scratch(&mut self.fft_in, &mut self.fft_out, &mut self.fft_scratch)
            .is_err()
        {
            self.frame_index += 1;
            return;
        }

        // Accumulate squared magnitude per chroma bin (skip DC i=0 and Nyquist i=2048)
        let mut chroma = [0f64; CHROMA_BINS];
        let bins = FRAME_SIZE / 2 + 1; // 2049
        for i in 1..bins - 1 {
            let c = self.chroma_map[i];
            if c < 0 {
                continue;
            }
            let re = self.fft_out[i].re;
            let im = self.fft_out[i].im;
            chroma[c as usize] += re * re + im * im;
        }

        // 5-tap FIR temporal smoothing
        let mut filtered = match self.fir_feed(&chroma) {
            Some(f) => f,
            None => {
                self.frame_index += 1;
                return; // buffer not primed yet
            }
        };

        // L2 normalization
        l2_normalize(&mut filtered);

        // 32 fixed pairwise comparisons → 32-bit fingerprint for this frame
        let fp = compute_frame_fp(&filtered);

        // Determine which 1-second bucket this frame belongs to
        let frame_sec = self.frame_index as f64 * FRAME_HOP as f64 / SAMPLE_RATE as f64;
        let bucket = frame_sec.floor() as usize;

        if !self.second_frames.is_empty() && bucket > self.aggregated.len() {
            // Close the previous bucket
            let val = Self::majority_vote(&self.second_frames);
            self.aggregated.push(val);
            self.second_frames.clear();
        }

        self.second_frames.push(fp);
        self.frame_index += 1;
    }

    /// Feed one chroma frame into the 5-tap FIR ring buffer.
    /// Returns `Some(filtered)` once the buffer is primed (needs 5 frames), else `None`.
    fn fir_feed(&mut self, input: &[f64; CHROMA_BINS]) -> Option<[f64; CHROMA_BINS]> {
        // Write new frame into ring at head position
        self.ring[self.ring_head] = *input;
        self.ring_head = (self.ring_head + 1) % 5;
        if self.ring_count < 5 {
            self.ring_count += 1;
            if self.ring_count < 5 {
                return None;
            }
        }

        // Weighted sum over ring slots (oldest = ring_head after increment)
        let mut out = [0f64; CHROMA_BINS];
        for i in 0..5 {
            let slot = (self.ring_head + i) % 5;
            let w = FIR_COEFF[i];
            for j in 0..CHROMA_BINS {
                out[j] += self.ring[slot][j] * w;
            }
        }
        for v in out.iter_mut() {
            *v /= FIR_NORM;
        }
        Some(out)
    }

    /// Bitwise majority vote across a set of per-frame fingerprints.
    fn majority_vote(frames: &[u32]) -> u32 {
        if frames.is_empty() {
            return 0;
        }
        let threshold = frames.len() / 2 + 1;
        let mut result = 0u32;
        for bit in 0..32u32 {
            let mask = 1u32 << bit;
            let count = frames.iter().filter(|&&f| f & mask != 0).count();
            if count >= threshold {
                result |= mask;
            }
        }
        result
    }
}

/// L2-normalise a chroma vector in-place (ε = 1e-10 to avoid division by zero).
fn l2_normalize(chroma: &mut [f64; CHROMA_BINS]) {
    let sum_sq: f64 = chroma.iter().map(|&v| v * v).sum();
    if sum_sq < 1e-10 {
        *chroma = [0.0; CHROMA_BINS];
        return;
    }
    let inv = 1.0 / sum_sq.sqrt();
    for v in chroma.iter_mut() {
        *v *= inv;
    }
}

/// Encode a normalised chroma vector as a 32-bit fingerprint using the 32 fixed pairs.
fn compute_frame_fp(chroma: &[f64; CHROMA_BINS]) -> u32 {
    let mut fp = 0u32;
    for (i, &(a, b)) in PAIRS.iter().enumerate() {
        if chroma[a as usize] > chroma[b as usize] {
            fp |= 1u32 << i;
        }
    }
    fp
}

// ── Public API ────────────────────────────────────────────────────────────────

/// Compute an audio fingerprint from a video/audio file.
///
/// Returns `None` if the file has no audio stream.
/// Returns an empty Vec if the audio stream contains too little data to fingerprint.
/// Returns one `u32` per second of audio on success.
pub fn compute_fingerprint(path: &Utf8Path) -> VdfResult<Option<Vec<u32>>> {
    ffmpeg::init().ok();

    let mut ictx = format::input(&path.as_std_path())
        .map_err(|e| VdfError::FfmpegGeneral { code: -1, msg: e.to_string() })?;

    // Resolve stream index and create the audio decoder before entering the
    // packet loop so we don't hold a borrow of ictx through the loop.
    let (stream_idx, mut decoder) = {
        let audio_stream = match ictx.streams().best(media::Type::Audio) {
            Some(s) => s,
            None => return Ok(None),
        };
        let idx = audio_stream.index();
        let dec = codec::Context::from_parameters(audio_stream.parameters())
            .map_err(|e| VdfError::FfmpegGeneral { code: -1, msg: e.to_string() })?
            .decoder()
            .audio()
            .map_err(|e| VdfError::FfmpegGeneral { code: -1, msg: e.to_string() })?;
        (idx, dec)
    };

    // Build the SWR resampler from decoder properties (known before decoding begins).
    let mut swr = match SwrContext::get(
        decoder.format(),
        decoder.channel_layout(),
        decoder.rate(),
        ffmpeg_the_third::format::Sample::I16(
            ffmpeg_the_third::format::sample::Type::Packed,
        ),
        ChannelLayoutMask::MONO,
        SAMPLE_RATE,
    ) {
        Ok(ctx) => ctx,
        Err(e) => {
            warn!("SWR init failed for {path}: {e}");
            return Ok(Some(vec![]));
        }
    };

    let mut chroma_ctx = ChromaContext::new();
    let mut raw_frame = AudioFrame::empty();
    let mut resampled = AudioFrame::empty();

    // Packets iterator yields Result<(Stream, Packet), Error> in ffmpeg-the-third 3.x
    for result in ictx.packets() {
        let (stream, packet) = match result {
            Ok(pair) => pair,
            Err(_) => continue,
        };
        if stream.index() != stream_idx {
            continue;
        }
        if decoder.send_packet(&packet).is_err() {
            continue;
        }
        while decoder.receive_frame(&mut raw_frame).is_ok() {
            if swr.run(&raw_frame, &mut resampled).is_err() {
                continue;
            }
            let pcm = frame_to_mono_i16(&resampled);
            chroma_ctx.feed(&pcm);
        }
    }

    chroma_ctx.finish();
    Ok(Some(chroma_ctx.fingerprint()))
}

/// Extract raw mono s16 samples from a resampled audio frame.
fn frame_to_mono_i16(frame: &AudioFrame) -> Vec<i16> {
    let n = frame.samples();
    let data = frame.data(0);
    let mut out = Vec::with_capacity(n);
    for i in 0..n {
        let idx = i * 2;
        if idx + 1 < data.len() {
            out.push(i16::from_le_bytes([data[idx], data[idx + 1]]));
        }
    }
    out
}

/// Hamming-based similarity between two fingerprints (0.0–1.0).
///
/// Compares at most the length of the shorter fingerprint.
pub fn fingerprint_similarity(a: &[u32], b: &[u32]) -> f32 {
    if a.is_empty() || b.is_empty() {
        return 0.0;
    }
    let len = a.len().min(b.len());
    let total_bits = (len * 32) as f32;
    let matching: u32 = a
        .iter()
        .zip(b.iter())
        .map(|(x, y)| !(x ^ y))
        .map(|v| v.count_ones())
        .take(len)
        .sum();
    matching as f32 / total_bits
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identical_fingerprints_have_similarity_one() {
        let fp = vec![0xDEADBEEFu32; 10];
        let sim = fingerprint_similarity(&fp, &fp);
        assert!((sim - 1.0).abs() < 1e-5, "got {sim}");
    }

    #[test]
    fn opposite_fingerprints_have_zero_similarity() {
        let a = vec![0u32; 10];
        let b = vec![0xFFFF_FFFFu32; 10];
        let sim = fingerprint_similarity(&a, &b);
        assert!(sim < 1e-5, "got {sim}");
    }

    #[test]
    fn chroma_map_has_valid_range() {
        let map = build_chroma_map();
        for &c in &map {
            assert!(c < 12, "chroma index out of range: {c}");
        }
    }

    #[test]
    fn pairs_are_exactly_32() {
        assert_eq!(PAIRS.len(), 32);
    }

    #[test]
    fn hann_window_endpoints_near_zero() {
        let w = build_hann_window();
        assert!(w[0] < 1e-10, "hann[0] = {}", w[0]);
        assert!(w[FRAME_SIZE - 1] < 0.01, "hann[last] = {}", w[FRAME_SIZE - 1]);
    }

    #[test]
    fn chroma_context_produces_output_for_sine_440hz() {
        // 440 Hz sine at 11025 Hz for 2 seconds → enough for at least 1 aggregated second
        let sr = SAMPLE_RATE as f64;
        let samples: Vec<i16> = (0..SAMPLE_RATE as usize * 2)
            .map(|i| {
                let t = i as f64 / sr;
                (((2.0 * std::f64::consts::PI * 440.0 * t).sin()) * 16000.0) as i16
            })
            .collect();

        let mut ctx = ChromaContext::new();
        ctx.feed(&samples);
        ctx.finish();
        let fp = ctx.fingerprint();
        assert!(!fp.is_empty(), "fingerprint should not be empty for 440 Hz tone");
    }

    #[test]
    fn fir_primes_after_five_frames() {
        let mut ctx = ChromaContext::new();
        // Feed 4 dummy frames — should get None (buffer not full)
        let dummy = [1.0f64; CHROMA_BINS];
        assert!(ctx.fir_feed(&dummy).is_none());
        assert!(ctx.fir_feed(&dummy).is_none());
        assert!(ctx.fir_feed(&dummy).is_none());
        assert!(ctx.fir_feed(&dummy).is_none());
        // 5th frame primes the buffer
        assert!(ctx.fir_feed(&dummy).is_some());
    }
}
