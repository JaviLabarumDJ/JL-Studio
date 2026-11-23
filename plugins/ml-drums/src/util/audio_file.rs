use std::io::Cursor;
use std::sync::atomic::{AtomicU32, Ordering};

static PLAYBACK_POINTS: AtomicU32 = AtomicU32::new(16);

pub fn set_live_resampling_points(points: u32) {
    PLAYBACK_POINTS.store(points, Ordering::Relaxed);
}


use rubato::{Resampler, SincFixedIn, SincInterpolationParameters, SincInterpolationType, WindowFunction};
use symphonia::core::audio::SampleBuffer;
use symphonia::core::codecs::DecoderOptions;
use symphonia::core::formats::FormatOptions;
use symphonia::core::io::MediaSourceStream;
use symphonia::core::meta::MetadataOptions;
use symphonia::core::probe::Hint;

/// Decode an in-memory audio blob (e.g. `include_bytes!`) to mono f32.
/// Used for the bundled default sample kit shipped inside the binary.
/// Resamples to `target_rate` if the file's native rate differs.
pub fn load_wav_mono_from_bytes(bytes: &'static [u8], label: &str, target_rate: f32) -> Result<Vec<f32>, String> {
    let mss = MediaSourceStream::new(Box::new(Cursor::new(bytes)), Default::default());
    let mut hint = Hint::new();
    hint.with_extension("wav");
    let (samples, _, native_rate) = decode(mss, hint, label, false)?;
    resample_if_needed(samples, native_rate, target_rate, label)
}

/// Load an audio file to mono f32 samples, resampled to `target_rate`.
pub fn load_wav_mono(path: &str, target_rate: f32) -> Result<Vec<f32>, String> {
    let file = std::fs::File::open(path).map_err(|e| format!("open {path}: {e}"))?;
    let mss = MediaSourceStream::new(Box::new(file), Default::default());

    let mut hint = Hint::new();
    if path.ends_with(".wav") {
        hint.with_extension("wav");
    }

    let (samples, _, native_rate) = decode(mss, hint, path, false)?;
    resample_if_needed(samples, native_rate, target_rate, path)
}

/// Retains the left and right channels for a pad, downmixing multichannel source files.
pub fn load_sample(path: &str, target_rate: f32) -> Result<(Vec<f32>, Option<Vec<f32>>), String> {
    let file = std::fs::File::open(path).map_err(|e| format!("open {path}: {e}"))?;
    let mss = MediaSourceStream::new(Box::new(file), Default::default());
    let mut hint = Hint::new();
    if let Some(ext) = std::path::Path::new(path).extension().and_then(|ext| ext.to_str()) { hint.with_extension(ext); }
    let (left, right, rate) = decode(mss, hint, path, true)?;
    let left = resample_if_needed(left, rate, target_rate, path)?;
    if left.is_empty() { return Err(format!("No audio in {path}")); }
    let right = right.map(|pcm| resample_if_needed(pcm, rate, target_rate, path)).transpose()?;
    Ok((left,right))
}

/// Use the same decoder and playback interpolation as the Browser.
pub fn load_source_sample(path: &std::path::Path, target_rate: f32) -> Result<(Vec<f32>, Vec<f32>), String> {
    if !target_rate.is_finite() || target_rate <= 0.0 {
        return Err("ML Drums audio is not initialized".into());
    }
    let os_engine::Sample { left, right, rate, .. } = os_engine::sample::load(path).map_err(|error| error.to_string())?;
    if left.is_empty() || right.is_empty() {
        return Err(format!("No audio in {}", path.display()));
    }
    if rate == target_rate as u32 {
        return Ok((left, right));
    }
    let ratio = f64::from(rate) / f64::from(target_rate);
    let count = (left.len().min(right.len()) as f64 / ratio).ceil() as usize;
    let (mut output_left, mut output_right) = (Vec::with_capacity(count), Vec::with_capacity(count));
    let width = PLAYBACK_POINTS.load(Ordering::Relaxed) as usize / 2;
    let quality = if width <= 1 {
        os_dsp::resample::Quality::Linear
    } else {
        os_dsp::resample::Quality::LiveSinc(width)
    };
    for frame in 0..count {
        let (l, r) = os_dsp::resample::read(&left, &right, frame as f64 * ratio, ratio, quality);
        output_left.push(l);
        output_right.push(r);
    }
    Ok((output_left, output_right))
}

fn decode(mss: MediaSourceStream, hint: Hint, path: &str, keep_stereo: bool) -> Result<(Vec<f32>, Option<Vec<f32>>, u32), String> {
    let probed = symphonia::default::get_probe()
        .format(&hint, mss, &FormatOptions::default(), &MetadataOptions::default())
        .map_err(|e| format!("probe {path}: {e}"))?;

    let mut format = probed.format;
    let track = format
        .default_track()
        .ok_or_else(|| format!("no audio track in {path}"))?;

    let channels = track.codec_params.channels.map(|c| c.count()).unwrap_or(1);
    let native_rate = track.codec_params.sample_rate.unwrap_or(44100);
    let track_id = track.id;

    let mut decoder = symphonia::default::get_codecs()
        .make(&track.codec_params, &DecoderOptions::default())
        .map_err(|e| format!("decoder {path}: {e}"))?;

    let mut samples = Vec::new();
    let mut right = (keep_stereo && channels == 2).then(Vec::new);

    loop {
        let packet = match format.next_packet() {
            Ok(p) => p,
            Err(symphonia::core::errors::Error::IoError(ref e))
                if e.kind() == std::io::ErrorKind::UnexpectedEof =>
            {
                break;
            }
            Err(e) => return Err(format!("packet read: {e}")),
        };

        if packet.track_id() != track_id {
            continue;
        }

        let decoded = decoder.decode(&packet).map_err(|e| format!("decode: {e}"))?;
        let spec = *decoded.spec();
        let duration = decoded.capacity();

        let mut buf = SampleBuffer::<f32>::new(duration as u64, spec);
        buf.copy_interleaved_ref(decoded);

        let interleaved = buf.samples();
        if let Some(right) = right.as_mut() {
            for frame in interleaved.chunks_exact(2) { samples.push(frame[0]); right.push(frame[1]); }
        } else if channels == 1 {
            samples.extend_from_slice(interleaved);
        } else {
            // Downmix to mono by averaging channels
            for chunk in interleaved.chunks(channels) {
                let sum: f32 = chunk.iter().sum();
                samples.push(sum / channels as f32);
            }
        }
    }

    Ok((samples, right, native_rate))
}

/// Resample mono f32 audio from `native_rate` to `target_rate` using sinc
/// interpolation. Returns the input unchanged if rates already match.
pub fn resample_if_needed(samples: Vec<f32>, native_rate: u32, target_rate: f32, label: &str) -> Result<Vec<f32>, String> {
    let target = target_rate as u32;
    if native_rate == target || samples.is_empty() {
        return Ok(samples);
    }

    let ratio = target_rate as f64 / native_rate as f64;
    let params = SincInterpolationParameters {
        sinc_len: 256,
        f_cutoff: 0.95,
        interpolation: SincInterpolationType::Linear,
        oversampling_factor: 256,
        window: WindowFunction::BlackmanHarris2,
    };

    let chunk_size = 1024;
    let mut resampler = SincFixedIn::<f64>::new(
        ratio,
        2.0,  // max relative ratio (fixed ratio, so this is just headroom)
        params,
        chunk_size,
        1, // mono
    ).map_err(|e| format!("resampler init for {label}: {e}"))?;

    let mut output = Vec::with_capacity((samples.len() as f64 * ratio * 1.1) as usize);
    let mut pos = 0;
    let expected_len = (samples.len() as f64 * ratio).ceil() as usize;
    let delay = resampler.output_delay();

    while output.len() < expected_len + delay {
        let end = (pos + chunk_size).min(samples.len());
        let mut chunk: Vec<f64> = samples[pos.min(end)..end].iter().map(|&s| s as f64).collect();

        // Pad the last chunk to chunk_size
        if chunk.len() < chunk_size {
            chunk.resize(chunk_size, 0.0);
        }

        let result = resampler.process(&[chunk], None)
            .map_err(|e| format!("resample {label}: {e}"))?;

        for &s in &result[0] {
            output.push(s as f32);
        }
        pos += chunk_size;
    }

    // Trim padding silence from the end (from the zero-padded last chunk)
    output.drain(..delay);
    output.truncate(expected_len);

    tracing::debug!(
        native_rate,
        target_rate = target,
        input_len = samples.len(),
        output_len = output.len(),
        "{label}: resampled"
    );

    Ok(output)
}
