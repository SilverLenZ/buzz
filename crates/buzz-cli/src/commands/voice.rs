//! `buzz voice` — local speech-to-text for voice notes and audio files.
//!
//! Runs entirely on the local machine: no relay connection, no network, no
//! API key. The speech model is read from a local directory, so this command
//! works offline and is usable as a building block by `buzz-acp`.
//!
//! A Buzz voice note is a `video/mp4` whose video track is a 16x16 black still
//! and whose audio track carries the speech (the relay has no audio allowlist,
//! so clients wrap speech in that envelope). Decoding one therefore means
//! reading the AAC track out of an MP4 container.

use std::path::PathBuf;

use crate::error::CliError;

/// Dispatch `buzz voice ...`.
pub async fn dispatch(cmd: crate::VoiceCmd) -> Result<(), CliError> {
    match cmd {
        crate::VoiceCmd::Transcribe {
            input,
            model_dir,
            model_type,
        } => transcribe_command(&input, &model_dir, model_type),
    }
}

/// Transcribe a local audio or voice-note file to stdout.
///
/// The model directory is resolved from `--model-dir`, or from
/// `BUZZ_VOICE_MODEL_DIR` when the flag is omitted.
fn transcribe_command(
    input: &str,
    model_dir: &Option<PathBuf>,
    model_type: crate::VoiceModelType,
) -> Result<(), CliError> {
    let model_dir = match model_dir {
        Some(dir) => dir.clone(),
        None => std::env::var_os("BUZZ_VOICE_MODEL_DIR")
            .map(PathBuf::from)
            .ok_or_else(|| {
                CliError::Usage(
                    "no speech model directory: pass --model-dir or set BUZZ_VOICE_MODEL_DIR"
                        .to_string(),
                )
            })?,
    };

    let config = buzz_voice::TranscribeConfig {
        model_dir: model_dir.clone(),
        model_type: model_type.as_model_type(),
        lm: None,
    };

    let samples = decode_input(input)?;
    let text = buzz_voice::transcribe(&samples.0, samples.1, &config)
        .map_err(|error| CliError::Other(error.to_string()))?;

    println!("{text}");
    Ok(())
}

/// Decode `input` into mono samples and its sample rate.
///
/// Accepts a WAV file directly, and anything else through `ffmpeg` (which is
/// what reads the AAC track out of a Buzz voice note's MP4 container).
fn decode_input(input: &str) -> Result<(Vec<f32>, u32), CliError> {
    let path = std::path::Path::new(input);
    if !path.exists() {
        return Err(CliError::Usage(format!("file not found: {input}")));
    }

    if is_wav(path) {
        let wave = sherpa_onnx::Wave::read(&path.display().to_string())
            .ok_or_else(|| CliError::Other(format!("could not read WAV file {input}")))?;
        let rate = wave.sample_rate() as u32;
        let samples = wave.samples().to_vec();
        return Ok((samples, rate));
    }

    decode_with_ffmpeg(path)
}

/// True when `path` is a RIFF/WAVE file.
fn is_wav(path: &std::path::Path) -> bool {
    let Ok(bytes) = std::fs::read(path) else {
        return false;
    };
    bytes.len() >= 12 && &bytes[..4] == b"RIFF" && &bytes[8..12] == b"WAVE"
}

/// Decode any media file to mono 16-bit PCM via `ffmpeg`.
///
/// `ffmpeg` is the only tool that reads the AAC track out of a Buzz voice
/// note's MP4 container, and it is already a dependency of the media pipeline.
fn decode_with_ffmpeg(path: &std::path::Path) -> Result<(Vec<f32>, u32), CliError> {
    const SAMPLE_RATE: u32 = 16_000;

    let display = path.display().to_string();
    let output = std::process::Command::new("ffmpeg")
        .args(["-v", "error"])
        .arg("-i")
        .arg(path)
        .args(["-f", "f32le", "-ar"])
        .arg(SAMPLE_RATE.to_string())
        .args(["-ac", "1"])
        .arg("pipe:1")
        .output()
        .map_err(|error| {
            CliError::Other(format!(
                "could not run ffmpeg to decode {display}: {error}. \
                 Install ffmpeg, or pass a WAV file."
            ))
        })?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(CliError::Other(format!(
            "ffmpeg could not decode {display}: {}",
            stderr.trim()
        )));
    }

    let bytes = output.stdout;
    if bytes.len() % 4 != 0 {
        return Err(CliError::Other(
            "ffmpeg returned a truncated sample buffer".to_string(),
        ));
    }
    let samples: Vec<f32> = bytes
        .chunks_exact(4)
        .map(|chunk| f32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]))
        .collect();
    if samples.is_empty() {
        return Err(CliError::Other(format!(
            "{display} decoded to no audio samples"
        )));
    }
    Ok((samples, SAMPLE_RATE))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_missing_input_is_a_usage_error() {
        let error =
            decode_input("/nonexistent/audio/file.wav").expect_err("missing file must fail");
        assert!(
            matches!(error, CliError::Usage(_)),
            "expected a usage error, got {error:?}"
        );
    }

    #[test]
    fn a_non_wav_file_is_routed_through_ffmpeg() {
        // A file that is not RIFF/WAVE must not be read as a WAV.
        let path = std::env::temp_dir().join("buzz-cli-voice-not-a-wav.bin");
        std::fs::write(&path, b"\x00\x01\x02\x03not a wav").expect("write fixture");
        assert!(!is_wav(&path));
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn a_wav_file_is_detected_by_its_magic_bytes() {
        let path = std::env::temp_dir().join("buzz-cli-voice-is-a-wav.wav");
        let mut bytes = b"RIFF".to_vec();
        bytes.extend_from_slice(&0u32.to_le_bytes());
        bytes.extend_from_slice(b"WAVE");
        std::fs::write(&path, bytes).expect("write fixture");
        assert!(is_wav(&path));
        let _ = std::fs::remove_file(&path);
    }
}
