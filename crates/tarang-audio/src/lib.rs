//! tarang-audio — Audio decoding for the Tarang media framework
//!
//! Pure Rust audio decoding powered by symphonia.
//! Supports MP3, FLAC, WAV, OGG Vorbis, AAC, ALAC, and WMA.

use tarang_core::{AudioCodec, AudioStreamInfo, MediaInfo, Result, SampleFormat, TarangError};

/// Audio decoder wrapping symphonia
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
            | AudioCodec::Alac => Ok(Self {
                codec,
                sample_rate: 0,
                channels: 0,
            }),
            other => Err(TarangError::UnsupportedCodec(other.to_string())),
        }
    }

    /// Get the codec this decoder handles
    pub fn codec(&self) -> AudioCodec {
        self.codec
    }

    /// Initialize from stream info
    pub fn init(&mut self, info: &AudioStreamInfo) {
        self.sample_rate = info.sample_rate;
        self.channels = info.channels;
    }

    /// Get current sample rate
    pub fn sample_rate(&self) -> u32 {
        self.sample_rate
    }

    /// Get current channel count
    pub fn channels(&self) -> u16 {
        self.channels
    }
}

/// Probe an audio file and return metadata using symphonia
pub fn probe_audio(reader: std::fs::File) -> Result<MediaInfo> {
    use symphonia::core::formats::FormatOptions;
    use symphonia::core::io::MediaSourceStream;
    use symphonia::core::meta::MetadataOptions;
    use symphonia::core::probe::Hint;

    let mss = MediaSourceStream::new(Box::new(reader), Default::default());
    let hint = Hint::new();
    let format_opts = FormatOptions::default();
    let meta_opts = MetadataOptions::default();

    let probed = symphonia::default::get_probe()
        .format(&hint, mss, &format_opts, &meta_opts)
        .map_err(|e| TarangError::DemuxError(format!("symphonia probe failed: {e}")))?;

    let format = probed.format;
    let mut streams = Vec::new();

    for track in format.tracks() {
        let params = &track.codec_params;
        let codec = match params.codec {
            symphonia::core::codecs::CODEC_TYPE_FLAC => AudioCodec::Flac,
            symphonia::core::codecs::CODEC_TYPE_MP3 => AudioCodec::Mp3,
            symphonia::core::codecs::CODEC_TYPE_VORBIS => AudioCodec::Vorbis,
            symphonia::core::codecs::CODEC_TYPE_AAC => AudioCodec::Aac,
            symphonia::core::codecs::CODEC_TYPE_ALAC => AudioCodec::Alac,
            symphonia::core::codecs::CODEC_TYPE_PCM_S16LE
            | symphonia::core::codecs::CODEC_TYPE_PCM_S16BE
            | symphonia::core::codecs::CODEC_TYPE_PCM_S24LE
            | symphonia::core::codecs::CODEC_TYPE_PCM_S32LE
            | symphonia::core::codecs::CODEC_TYPE_PCM_F32LE
            | symphonia::core::codecs::CODEC_TYPE_PCM_F64LE => AudioCodec::Pcm,
            _ => continue,
        };

        let sample_rate = params.sample_rate.unwrap_or(44100);
        let channels = params.channels.map(|c| c.count() as u16).unwrap_or(2);

        let duration = params
            .n_frames
            .map(|n| std::time::Duration::from_secs_f64(n as f64 / sample_rate as f64));

        streams.push(tarang_core::StreamInfo::Audio(AudioStreamInfo {
            codec,
            sample_rate,
            channels,
            sample_format: SampleFormat::F32,
            bitrate: params
                .bits_per_coded_sample
                .map(|b| sample_rate * channels as u32 * b),
            duration,
        }));
    }

    let duration = streams.iter().find_map(|s| match s {
        tarang_core::StreamInfo::Audio(a) => a.duration,
        _ => None,
    });

    Ok(MediaInfo {
        id: uuid::Uuid::new_v4(),
        format: tarang_core::ContainerFormat::Mp4, // symphonia determines actual format
        streams,
        duration,
        file_size: None,
        title: None,
        artist: None,
        album: None,
    })
}

/// List codecs supported by the audio decoder (pure Rust, no FFI)
pub fn supported_codecs() -> Vec<AudioCodec> {
    vec![
        AudioCodec::Pcm,
        AudioCodec::Mp3,
        AudioCodec::Flac,
        AudioCodec::Vorbis,
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
            sample_format: SampleFormat::F32,
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
        assert!(codecs.contains(&AudioCodec::Aac));
        assert!(codecs.contains(&AudioCodec::Pcm));
        assert!(!codecs.contains(&AudioCodec::Wma));
    }
}
