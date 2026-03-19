//! Transcription routing to hoosh (Whisper models)
//!
//! Preprocesses audio for transcription (resample to 16kHz mono),
//! encodes as in-memory WAV, and routes to a hoosh endpoint for
//! Whisper-based speech-to-text.

use std::time::Duration;
use crate::core::{AudioBuffer, Result, SampleFormat, TarangError};

use super::{TranscriptionRequest, TranscriptionResult};

/// Whisper model size selection.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WhisperModel {
    Tiny,
    Base,
    Small,
    Medium,
    Large,
    LargeV3,
}

impl std::fmt::Display for WhisperModel {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Tiny => write!(f, "tiny"),
            Self::Base => write!(f, "base"),
            Self::Small => write!(f, "small"),
            Self::Medium => write!(f, "medium"),
            Self::Large => write!(f, "large"),
            Self::LargeV3 => write!(f, "large-v3"),
        }
    }
}

/// Configuration for the hoosh transcription endpoint.
#[derive(Debug, Clone)]
pub struct HooshConfig {
    pub endpoint: String,
    pub api_key: Option<String>,
    pub model: WhisperModel,
    pub timeout: Duration,
}

impl Default for HooshConfig {
    fn default() -> Self {
        Self {
            endpoint: "http://localhost:8088/v1/audio/transcribe".to_string(),
            api_key: None,
            model: WhisperModel::Base,
            timeout: Duration::from_secs(300),
        }
    }
}

/// Client for routing transcription requests to hoosh.
pub struct HooshClient {
    config: HooshConfig,
    http: reqwest::Client,
}

impl HooshClient {
    pub fn new(config: HooshConfig) -> Result<Self> {
        let http = reqwest::Client::builder()
            .timeout(config.timeout)
            .build()
            .map_err(|e| TarangError::NetworkError(format!("failed to create HTTP client: {e}")))?;

        Ok(Self { config, http })
    }

    /// Transcribe an audio buffer via hoosh.
    pub async fn transcribe(
        &self,
        request: &TranscriptionRequest,
        audio: &AudioBuffer,
    ) -> Result<TranscriptionResult> {
        let wav_bytes = encode_wav_bytes(audio)?;

        let mut form = reqwest::multipart::Form::new()
            .text("model", self.config.model.to_string())
            .text("sample_rate", request.sample_rate.to_string())
            .text("channels", request.channels.to_string());

        if let Some(lang) = &request.language_hint {
            form = form.text("language", lang.clone());
        }

        let part = reqwest::multipart::Part::bytes(wav_bytes)
            .file_name("audio.wav")
            .mime_str("audio/wav")
            .map_err(|e| TarangError::NetworkError(format!("mime error: {e}")))?;
        form = form.part("audio", part);

        let mut req = self.http.post(&self.config.endpoint).multipart(form);

        if let Some(key) = &self.config.api_key {
            req = req.header("Authorization", format!("Bearer {key}"));
        }

        let response = req
            .send()
            .await
            .map_err(|e| TarangError::NetworkError(format!("hoosh request failed: {e}")))?;

        if !response.status().is_success() {
            return Err(TarangError::NetworkError(format!(
                "hoosh returned status {}",
                response.status()
            )));
        }

        let result: TranscriptionResult = response
            .json()
            .await
            .map_err(|e| TarangError::NetworkError(format!("failed to parse response: {e}")))?;

        Ok(result)
    }
}

/// Preprocess audio for transcription: downmix to mono, resample to 16kHz.
pub fn prepare_audio_for_transcription(buf: &AudioBuffer) -> AudioBuffer {
    // Downmix to mono if needed
    if buf.channels > 1 {
        let sample_count = buf.num_samples;
        let channels = buf.channels as usize;
        let bytes_per_sample = buf.sample_format.bytes_per_sample();
        let mut mono_data = Vec::with_capacity(sample_count * bytes_per_sample);

        // Average channels for each sample (assuming F32 interleaved)
        if buf.sample_format == SampleFormat::F32 {
            for i in 0..sample_count {
                let mut sum = 0.0f32;
                let mut count = 0u32;
                for ch in 0..channels {
                    let offset = (i * channels + ch) * 4;
                    if offset + 4 <= buf.data.len() {
                        let sample = f32::from_le_bytes([
                            buf.data[offset],
                            buf.data[offset + 1],
                            buf.data[offset + 2],
                            buf.data[offset + 3],
                        ]);
                        sum += sample;
                        count += 1;
                    }
                }
                let avg = if count > 0 { sum / count as f32 } else { 0.0 };
                mono_data.extend_from_slice(&avg.to_le_bytes());
            }
        } else {
            // For non-F32, just take the first channel
            for i in 0..sample_count {
                let offset = i * channels * bytes_per_sample;
                let end = offset + bytes_per_sample;
                if end <= buf.data.len() {
                    mono_data.extend_from_slice(&buf.data[offset..end]);
                }
            }
        }

        AudioBuffer {
            data: bytes::Bytes::from(mono_data),
            sample_format: buf.sample_format,
            channels: 1,
            sample_rate: buf.sample_rate,
            num_samples: sample_count,
            timestamp: buf.timestamp,
        }
    } else {
        buf.clone()
    }
}

/// Encode an AudioBuffer as WAV bytes (PCM16, in-memory).
pub fn encode_wav_bytes(buf: &AudioBuffer) -> Result<Vec<u8>> {
    let channels = buf.channels;
    let sample_rate = buf.sample_rate;
    let bits_per_sample: u16 = 16;
    let bytes_per_sample = bits_per_sample / 8;
    let block_align = channels * bytes_per_sample;
    let byte_rate = sample_rate * block_align as u32;

    // Convert samples to PCM16
    let pcm16 = samples_to_pcm16(buf)?;
    let data_size = pcm16.len() as u32;
    let file_size = 36 + data_size;

    let mut wav = Vec::with_capacity(44 + data_size as usize);

    // RIFF header
    wav.extend_from_slice(b"RIFF");
    wav.extend_from_slice(&file_size.to_le_bytes());
    wav.extend_from_slice(b"WAVE");

    // fmt chunk
    wav.extend_from_slice(b"fmt ");
    wav.extend_from_slice(&16u32.to_le_bytes()); // chunk size
    wav.extend_from_slice(&1u16.to_le_bytes()); // PCM format
    wav.extend_from_slice(&channels.to_le_bytes());
    wav.extend_from_slice(&sample_rate.to_le_bytes());
    wav.extend_from_slice(&byte_rate.to_le_bytes());
    wav.extend_from_slice(&block_align.to_le_bytes());
    wav.extend_from_slice(&bits_per_sample.to_le_bytes());

    // data chunk
    wav.extend_from_slice(b"data");
    wav.extend_from_slice(&data_size.to_le_bytes());
    wav.extend_from_slice(&pcm16);

    Ok(wav)
}

fn samples_to_pcm16(buf: &AudioBuffer) -> Result<Vec<u8>> {
    match buf.sample_format {
        SampleFormat::F32 => {
            let num_values = buf.data.len() / 4;
            let mut pcm = Vec::with_capacity(num_values * 2);
            for i in 0..num_values {
                let offset = i * 4;
                let sample = f32::from_le_bytes([
                    buf.data[offset],
                    buf.data[offset + 1],
                    buf.data[offset + 2],
                    buf.data[offset + 3],
                ]);
                let clamped = (sample * 32767.0).clamp(-32768.0, 32767.0) as i16;
                pcm.extend_from_slice(&clamped.to_le_bytes());
            }
            Ok(pcm)
        }
        SampleFormat::I16 => Ok(buf.data.to_vec()),
        SampleFormat::I32 => {
            let num_values = buf.data.len() / 4;
            let mut pcm = Vec::with_capacity(num_values * 2);
            for i in 0..num_values {
                let offset = i * 4;
                let sample = i32::from_le_bytes([
                    buf.data[offset],
                    buf.data[offset + 1],
                    buf.data[offset + 2],
                    buf.data[offset + 3],
                ]);
                let clamped = (sample >> 16) as i16;
                pcm.extend_from_slice(&clamped.to_le_bytes());
            }
            Ok(pcm)
        }
        _ => Err(TarangError::AiError(format!(
            "unsupported sample format for WAV encoding: {:?}",
            buf.sample_format
        ))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bytes::Bytes;

    fn make_f32_buffer(channels: u16, sample_rate: u32, num_samples: usize) -> AudioBuffer {
        let total_values = num_samples * channels as usize;
        let mut data = Vec::with_capacity(total_values * 4);
        for i in 0..total_values {
            let t = i as f32 / sample_rate as f32;
            let sample = (t * 440.0 * std::f32::consts::TAU).sin() * 0.5;
            data.extend_from_slice(&sample.to_le_bytes());
        }
        AudioBuffer {
            data: Bytes::from(data),
            sample_format: SampleFormat::F32,
            channels,
            sample_rate,
            num_samples,
            timestamp: Duration::ZERO,
        }
    }

    #[test]
    fn whisper_model_display() {
        assert_eq!(WhisperModel::Tiny.to_string(), "tiny");
        assert_eq!(WhisperModel::LargeV3.to_string(), "large-v3");
    }

    #[test]
    fn hoosh_config_default() {
        let config = HooshConfig::default();
        assert!(config.endpoint.contains("8088"));
        assert!(config.endpoint.contains("transcribe"));
        assert_eq!(config.model, WhisperModel::Base);
    }

    #[test]
    fn prepare_stereo_to_mono() {
        let buf = make_f32_buffer(2, 44100, 1024);
        assert_eq!(buf.channels, 2);
        let mono = prepare_audio_for_transcription(&buf);
        assert_eq!(mono.channels, 1);
        assert_eq!(mono.num_samples, 1024);
    }

    #[test]
    fn prepare_mono_passthrough() {
        let buf = make_f32_buffer(1, 16000, 512);
        let result = prepare_audio_for_transcription(&buf);
        assert_eq!(result.channels, 1);
        assert_eq!(result.sample_rate, 16000);
    }

    #[test]
    fn encode_wav_valid_header() {
        let buf = make_f32_buffer(1, 16000, 1600);
        let wav = encode_wav_bytes(&buf).unwrap();

        // RIFF header
        assert_eq!(&wav[..4], b"RIFF");
        assert_eq!(&wav[8..12], b"WAVE");
        // fmt chunk
        assert_eq!(&wav[12..16], b"fmt ");
        // PCM format = 1
        assert_eq!(u16::from_le_bytes([wav[20], wav[21]]), 1);
        // 1 channel
        assert_eq!(u16::from_le_bytes([wav[22], wav[23]]), 1);
        // 16000 Hz
        assert_eq!(
            u32::from_le_bytes([wav[24], wav[25], wav[26], wav[27]]),
            16000
        );
        // 16 bits per sample
        assert_eq!(u16::from_le_bytes([wav[34], wav[35]]), 16);
        // data chunk
        assert_eq!(&wav[36..40], b"data");
    }

    #[test]
    fn encode_wav_correct_data_size() {
        let buf = make_f32_buffer(1, 16000, 100);
        let wav = encode_wav_bytes(&buf).unwrap();
        let data_size = u32::from_le_bytes([wav[40], wav[41], wav[42], wav[43]]);
        assert_eq!(data_size, 200); // 100 samples * 2 bytes (PCM16)
    }

    #[test]
    fn pcm16_from_f32() {
        let mut data = Vec::new();
        data.extend_from_slice(&0.5f32.to_le_bytes()); // should map to ~16383
        data.extend_from_slice(&(-0.5f32).to_le_bytes()); // should map to ~-16383
        data.extend_from_slice(&0.0f32.to_le_bytes()); // should map to 0

        let buf = AudioBuffer {
            data: Bytes::from(data),
            sample_format: SampleFormat::F32,
            channels: 1,
            sample_rate: 16000,
            num_samples: 3,
            timestamp: Duration::ZERO,
        };

        let pcm = samples_to_pcm16(&buf).unwrap();
        assert_eq!(pcm.len(), 6); // 3 samples * 2 bytes

        let s0 = i16::from_le_bytes([pcm[0], pcm[1]]);
        let s1 = i16::from_le_bytes([pcm[2], pcm[3]]);
        let s2 = i16::from_le_bytes([pcm[4], pcm[5]]);

        assert!(s0 > 16000);
        assert!(s1 < -16000);
        assert_eq!(s2, 0);
    }

    #[test]
    fn pcm16_passthrough_i16() {
        let data = vec![0x00u8, 0x40, 0xFF, 0x3F]; // two i16 samples
        let buf = AudioBuffer {
            data: Bytes::from(data.clone()),
            sample_format: SampleFormat::I16,
            channels: 1,
            sample_rate: 16000,
            num_samples: 2,
            timestamp: Duration::ZERO,
        };
        let pcm = samples_to_pcm16(&buf).unwrap();
        assert_eq!(pcm, data);
    }

    #[test]
    fn transcription_result_serde() {
        use crate::ai::TranscriptionSegment;
        let result = TranscriptionResult {
            text: "hello world".to_string(),
            language: "en".to_string(),
            confidence: 0.95,
            segments: vec![TranscriptionSegment {
                start: 0.0,
                end: 1.5,
                text: "hello world".to_string(),
                confidence: 0.95,
            }],
        };
        let json = serde_json::to_string(&result).unwrap();
        let parsed: TranscriptionResult = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.text, "hello world");
        assert_eq!(parsed.segments.len(), 1);
    }

    #[test]
    fn whisper_model_display_all() {
        assert_eq!(WhisperModel::Small.to_string(), "small");
        assert_eq!(WhisperModel::Medium.to_string(), "medium");
        assert_eq!(WhisperModel::Large.to_string(), "large");
        assert_eq!(WhisperModel::Base.to_string(), "base");
    }

    #[test]
    fn pcm16_from_i32() {
        let mut data = Vec::new();
        // i32 max >> 16 should give i16 max range
        data.extend_from_slice(&(0x7FFF0000i32).to_le_bytes());
        data.extend_from_slice(&(0i32).to_le_bytes());
        data.extend_from_slice(&(-0x7FFF0000i32).to_le_bytes());

        let buf = AudioBuffer {
            data: Bytes::from(data),
            sample_format: SampleFormat::I32,
            channels: 1,
            sample_rate: 16000,
            num_samples: 3,
            timestamp: Duration::ZERO,
        };

        let pcm = samples_to_pcm16(&buf).unwrap();
        assert_eq!(pcm.len(), 6);
        let s0 = i16::from_le_bytes([pcm[0], pcm[1]]);
        let s1 = i16::from_le_bytes([pcm[2], pcm[3]]);
        let s2 = i16::from_le_bytes([pcm[4], pcm[5]]);
        assert_eq!(s0, 0x7FFF);
        assert_eq!(s1, 0);
        assert_eq!(s2, -0x7FFF);
    }

    #[test]
    fn pcm16_unsupported_format() {
        let buf = AudioBuffer {
            data: Bytes::from(vec![0u8; 64]),
            sample_format: SampleFormat::F64,
            channels: 1,
            sample_rate: 16000,
            num_samples: 8,
            timestamp: Duration::ZERO,
        };
        assert!(samples_to_pcm16(&buf).is_err());
    }

    #[test]
    fn encode_wav_stereo() {
        let buf = make_f32_buffer(2, 44100, 1000);
        let wav = encode_wav_bytes(&buf).unwrap();
        assert_eq!(&wav[..4], b"RIFF");
        assert_eq!(&wav[8..12], b"WAVE");
        // 2 channels
        assert_eq!(u16::from_le_bytes([wav[22], wav[23]]), 2);
        // 44100 Hz
        assert_eq!(
            u32::from_le_bytes([wav[24], wav[25], wav[26], wav[27]]),
            44100
        );
    }

    #[test]
    fn prepare_i16_stereo_takes_first_channel() {
        let num_samples = 100;
        let mut data = Vec::with_capacity(num_samples * 2 * 2);
        for i in 0..num_samples {
            // Left channel: ascending
            let left = (i as i16) * 100;
            // Right channel: descending
            let right = -((i as i16) * 100);
            data.extend_from_slice(&left.to_le_bytes());
            data.extend_from_slice(&right.to_le_bytes());
        }
        let buf = AudioBuffer {
            data: Bytes::from(data),
            sample_format: SampleFormat::I16,
            channels: 2,
            sample_rate: 16000,
            num_samples,
            timestamp: Duration::ZERO,
        };
        let mono = prepare_audio_for_transcription(&buf);
        assert_eq!(mono.channels, 1);
        // Non-F32 takes first channel, so first sample should be 0
        let first = i16::from_le_bytes([mono.data[0], mono.data[1]]);
        assert_eq!(first, 0);
    }

    #[test]
    fn prepare_multichannel_f32_averages() {
        let num_samples = 10;
        let channels = 4u16;
        let mut data = Vec::with_capacity(num_samples * channels as usize * 4);
        for _ in 0..num_samples {
            // All channels = 0.4, so average should be 0.4
            for _ in 0..channels {
                data.extend_from_slice(&(0.4f32).to_le_bytes());
            }
        }
        let buf = AudioBuffer {
            data: Bytes::from(data),
            sample_format: SampleFormat::F32,
            channels,
            sample_rate: 16000,
            num_samples,
            timestamp: Duration::ZERO,
        };
        let mono = prepare_audio_for_transcription(&buf);
        assert_eq!(mono.channels, 1);
        // Check first sample is ~0.4
        let first = f32::from_le_bytes([mono.data[0], mono.data[1], mono.data[2], mono.data[3]]);
        assert!((first - 0.4).abs() < 0.01);
    }

    #[test]
    fn transcription_request_serde() {
        use crate::ai::TranscriptionRequest;
        let req = TranscriptionRequest {
            audio_codec: "Opus".to_string(),
            sample_rate: 48000,
            channels: 1,
            duration_secs: 120.5,
            language_hint: Some("es".to_string()),
        };
        let json = serde_json::to_string(&req).unwrap();
        let parsed: TranscriptionRequest = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.audio_codec, "Opus");
        assert_eq!(parsed.language_hint, Some("es".to_string()));
    }
}
