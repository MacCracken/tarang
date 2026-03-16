//! tarang-audio — Audio decoding for the Tarang media framework
//!
//! Pure Rust audio decoding powered by symphonia.
//! Supports MP3, FLAC, WAV, OGG Vorbis, AAC, ALAC, and PCM.

mod decode;
mod encode;
mod mix;
mod output;
mod probe;
mod resample;

pub use decode::FileDecoder;
pub use encode::{AudioEncoder, EncoderConfig, PcmEncoder, create_encoder};
pub use mix::{ChannelLayout, mix_channels};
pub use output::{AudioOutput, NullOutput, OutputConfig};
#[cfg(feature = "pipewire")]
pub use output::PipeWireOutput;
pub use probe::probe_audio;
pub use resample::{resample, resample_sinc};

use tarang_core::{AudioCodec, AudioStreamInfo, Result, TarangError};

/// Audio decoder metadata (lightweight, no symphonia state)
pub struct AudioDecoder {
    codec: AudioCodec,
    sample_rate: u32,
    channels: u16,
}

impl AudioDecoder {
    /// Create a decoder for the given audio codec
    pub fn new(codec: AudioCodec) -> Result<Self> {
        match codec {
            AudioCodec::Pcm
            | AudioCodec::Mp3
            | AudioCodec::Flac
            | AudioCodec::Vorbis
            | AudioCodec::Aac
            | AudioCodec::Alac
            | AudioCodec::Opus => Ok(Self {
                codec,
                sample_rate: 0,
                channels: 0,
            }),
            other => Err(TarangError::UnsupportedCodec(other.to_string())),
        }
    }

    pub fn codec(&self) -> AudioCodec {
        self.codec
    }

    pub fn init(&mut self, info: &AudioStreamInfo) {
        self.sample_rate = info.sample_rate;
        self.channels = info.channels;
    }

    pub fn sample_rate(&self) -> u32 {
        self.sample_rate
    }

    pub fn channels(&self) -> u16 {
        self.channels
    }
}

/// List codecs supported by the audio decoder (pure Rust, no FFI)
pub fn supported_codecs() -> Vec<AudioCodec> {
    vec![
        AudioCodec::Pcm,
        AudioCodec::Mp3,
        AudioCodec::Flac,
        AudioCodec::Vorbis,
        AudioCodec::Opus,
        AudioCodec::Aac,
        AudioCodec::Alac,
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn create_mp3_decoder() {
        let decoder = AudioDecoder::new(AudioCodec::Mp3).unwrap();
        assert_eq!(decoder.codec(), AudioCodec::Mp3);
    }

    #[test]
    fn create_flac_decoder() {
        let decoder = AudioDecoder::new(AudioCodec::Flac).unwrap();
        assert_eq!(decoder.codec(), AudioCodec::Flac);
    }

    #[test]
    fn create_vorbis_decoder() {
        let decoder = AudioDecoder::new(AudioCodec::Vorbis).unwrap();
        assert_eq!(decoder.codec(), AudioCodec::Vorbis);
    }

    #[test]
    fn create_opus_decoder() {
        let decoder = AudioDecoder::new(AudioCodec::Opus).unwrap();
        assert_eq!(decoder.codec(), AudioCodec::Opus);
    }

    #[test]
    fn create_aac_decoder() {
        let decoder = AudioDecoder::new(AudioCodec::Aac).unwrap();
        assert_eq!(decoder.codec(), AudioCodec::Aac);
    }

    #[test]
    fn create_pcm_decoder() {
        let decoder = AudioDecoder::new(AudioCodec::Pcm).unwrap();
        assert_eq!(decoder.codec(), AudioCodec::Pcm);
    }

    #[test]
    fn unsupported_wma() {
        assert!(AudioDecoder::new(AudioCodec::Wma).is_err());
    }

    #[test]
    fn decoder_init() {
        let mut decoder = AudioDecoder::new(AudioCodec::Mp3).unwrap();
        let info = AudioStreamInfo {
            codec: AudioCodec::Mp3,
            sample_rate: 48000,
            channels: 2,
            sample_format: tarang_core::SampleFormat::F32,
            bitrate: Some(320_000),
            duration: None,
        };
        decoder.init(&info);
        assert_eq!(decoder.sample_rate(), 48000);
        assert_eq!(decoder.channels(), 2);
    }

    #[test]
    fn supported_codecs_list() {
        let codecs = supported_codecs();
        assert!(codecs.contains(&AudioCodec::Mp3));
        assert!(codecs.contains(&AudioCodec::Flac));
        assert!(codecs.contains(&AudioCodec::Vorbis));
        assert!(codecs.contains(&AudioCodec::Opus));
        assert!(codecs.contains(&AudioCodec::Aac));
        assert!(codecs.contains(&AudioCodec::Pcm));
        assert!(!codecs.contains(&AudioCodec::Wma));
    }
}
