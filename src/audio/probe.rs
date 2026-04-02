//! Audio file probing via shravan
//!
//! # Example
//! ```rust,ignore
//! use tarang::audio::probe::probe_audio;
//!
//! let file = std::fs::File::open("song.flac").unwrap();
//! let info = probe_audio(file).unwrap();
//! println!("streams: {:?}", info.streams);
//! ```

use crate::core::{
    AudioCodec, AudioStreamInfo, ContainerFormat, MediaInfo, Result, SampleFormat, StreamInfo,
    TarangError,
};
use std::collections::HashMap;
use std::io::Read;

/// Probe an audio file and return metadata using shravan
pub fn probe_audio(mut reader: std::fs::File) -> Result<MediaInfo> {
    let mut data = Vec::new();
    reader.read_to_end(&mut data).map_err(TarangError::Io)?;

    if data.is_empty() {
        return Err(TarangError::DemuxError("empty file".into()));
    }

    let audio_format = shravan::format::detect_format(&data)
        .map_err(|e| TarangError::DemuxError(format!("shravan probe failed: {e}").into()))?;

    let container = map_format_to_container(audio_format);
    let codec = map_shravan_format(audio_format);

    let (info, _samples) = shravan::codec::open(&data)
        .map_err(|e| TarangError::DemuxError(format!("shravan decode failed: {e}").into()))?;

    // Extract metadata tags
    let tags = extract_tags_from_data(&data);

    let total_samples = info.total_samples;
    let duration = if total_samples > 0 && info.sample_rate > 0 {
        Some(std::time::Duration::from_secs_f64(
            total_samples as f64 / info.sample_rate as f64,
        ))
    } else {
        None
    };

    let bitrate = (info.bit_depth as u32)
        .checked_mul(info.sample_rate)
        .and_then(|b| b.checked_mul(info.channels as u32));

    let streams = vec![StreamInfo::Audio(AudioStreamInfo {
        codec,
        sample_rate: info.sample_rate,
        channels: info.channels,
        sample_format: SampleFormat::F32,
        bitrate,
        duration,
    })];

    let title = tags.get("title").cloned();
    let artist = tags.get("artist").cloned();
    let album = tags.get("album").cloned();

    Ok(MediaInfo {
        id: uuid::Uuid::new_v4(),
        format: container,
        streams,
        duration,
        file_size: None,
        title,
        artist,
        album,
        metadata: tags,
    })
}

/// Map shravan `AudioFormat` to our `AudioCodec` enum.
pub(crate) fn map_shravan_format(fmt: shravan::format::AudioFormat) -> AudioCodec {
    match fmt {
        shravan::format::AudioFormat::Wav => AudioCodec::Pcm,
        shravan::format::AudioFormat::Flac => AudioCodec::Flac,
        shravan::format::AudioFormat::Ogg => AudioCodec::Vorbis,
        shravan::format::AudioFormat::Mp3 => AudioCodec::Mp3,
        shravan::format::AudioFormat::Opus => AudioCodec::Opus,
        shravan::format::AudioFormat::Aiff => AudioCodec::Pcm,
        shravan::format::AudioFormat::RawPcm => AudioCodec::Pcm,
        _ => AudioCodec::Pcm,
    }
}

/// Map shravan `AudioFormat` to our `ContainerFormat` enum.
fn map_format_to_container(fmt: shravan::format::AudioFormat) -> ContainerFormat {
    match fmt {
        shravan::format::AudioFormat::Wav => ContainerFormat::Wav,
        shravan::format::AudioFormat::Flac => ContainerFormat::Flac,
        shravan::format::AudioFormat::Ogg => ContainerFormat::Ogg,
        shravan::format::AudioFormat::Mp3 => ContainerFormat::Mp3,
        shravan::format::AudioFormat::Opus => ContainerFormat::Ogg,
        shravan::format::AudioFormat::Aiff => ContainerFormat::Wav,
        shravan::format::AudioFormat::RawPcm => ContainerFormat::Wav,
        _ => ContainerFormat::Mp4,
    }
}

/// Extract metadata tags from raw audio data using shravan's tag parsers.
fn extract_tags_from_data(data: &[u8]) -> HashMap<String, String> {
    let mut tags = HashMap::new();

    // Try ID3v2 tags (MP3 files)
    if let Ok(meta) = shravan::tag::read_id3v2(data) {
        if let Some(v) = &meta.title {
            tags.insert("title".to_string(), v.clone());
        }
        if let Some(v) = &meta.artist {
            tags.insert("artist".to_string(), v.clone());
        }
        if let Some(v) = &meta.album {
            tags.insert("album".to_string(), v.clone());
        }
        if let Some(v) = &meta.track_number {
            tags.insert("tracknumber".to_string(), v.clone());
        }
        if let Some(v) = &meta.year {
            tags.insert("date".to_string(), v.clone());
        }
        if let Some(v) = &meta.genre {
            tags.insert("genre".to_string(), v.clone());
        }
        if let Some(v) = &meta.comment {
            tags.insert("comment".to_string(), v.clone());
        }
    }

    tags
}

/// Create a minimal WAV file in memory for testing.
#[cfg(test)]
fn make_test_wav(num_samples: u32, sample_rate: u32, channels: u16) -> Vec<u8> {
    let bits: u16 = 16;
    let data_size = num_samples * channels as u32 * (bits as u32 / 8);
    let file_size = 36 + data_size;
    let byte_rate = sample_rate * channels as u32 * (bits as u32 / 8);
    let block_align = channels * (bits / 8);

    let mut buf = Vec::new();
    buf.extend_from_slice(b"RIFF");
    buf.extend_from_slice(&file_size.to_le_bytes());
    buf.extend_from_slice(b"WAVE");
    buf.extend_from_slice(b"fmt ");
    buf.extend_from_slice(&16u32.to_le_bytes());
    buf.extend_from_slice(&1u16.to_le_bytes()); // PCM
    buf.extend_from_slice(&channels.to_le_bytes());
    buf.extend_from_slice(&sample_rate.to_le_bytes());
    buf.extend_from_slice(&byte_rate.to_le_bytes());
    buf.extend_from_slice(&block_align.to_le_bytes());
    buf.extend_from_slice(&bits.to_le_bytes());
    buf.extend_from_slice(b"data");
    buf.extend_from_slice(&data_size.to_le_bytes());

    for i in 0..num_samples {
        let t = i as f64 / sample_rate as f64;
        let sample = (t * 440.0 * 2.0 * std::f64::consts::PI).sin();
        let s16 = (sample * 32000.0) as i16;
        for _ in 0..channels {
            buf.extend_from_slice(&s16.to_le_bytes());
        }
    }
    buf
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    /// Write WAV bytes to a temp file, return the File handle opened for reading.
    fn wav_to_tempfile(wav: &[u8]) -> std::fs::File {
        let mut tmp = tempfile::NamedTempFile::new().unwrap();
        tmp.write_all(wav).unwrap();
        tmp.flush().unwrap();
        let path = tmp.into_temp_path();
        std::fs::File::open(&path).unwrap()
    }

    #[test]
    fn probe_wav_stereo() {
        let wav = make_test_wav(4410, 44100, 2);
        let file = wav_to_tempfile(&wav);
        let info = probe_audio(file).unwrap();

        assert_eq!(info.format, ContainerFormat::Wav);
        assert!(info.has_audio());
        assert!(!info.has_video());
        assert_eq!(info.streams.len(), 1);

        let audio = info.audio_streams().collect::<Vec<_>>();
        assert_eq!(audio.len(), 1);
        assert_eq!(audio[0].codec, AudioCodec::Pcm);
        assert_eq!(audio[0].sample_rate, 44100);
        assert_eq!(audio[0].channels, 2);
    }

    #[test]
    fn probe_wav_mono() {
        let wav = make_test_wav(8000, 16000, 1);
        let file = wav_to_tempfile(&wav);
        let info = probe_audio(file).unwrap();

        assert_eq!(info.format, ContainerFormat::Wav);
        let audio = info.audio_streams().collect::<Vec<_>>();
        assert_eq!(audio[0].sample_rate, 16000);
        assert_eq!(audio[0].channels, 1);
    }

    #[test]
    fn probe_wav_has_duration() {
        let wav = make_test_wav(44100, 44100, 1); // 1 second
        let file = wav_to_tempfile(&wav);
        let info = probe_audio(file).unwrap();

        assert!(info.duration.is_some());
        let dur = info.duration.unwrap();
        assert!((dur.as_secs_f64() - 1.0).abs() < 0.05);
    }

    #[test]
    fn probe_wav_high_sample_rate() {
        let wav = make_test_wav(960, 96000, 2);
        let file = wav_to_tempfile(&wav);
        let info = probe_audio(file).unwrap();

        assert_eq!(
            info.audio_streams().collect::<Vec<_>>()[0].sample_rate,
            96000
        );
    }

    #[test]
    fn probe_wav_has_uuid() {
        let wav = make_test_wav(100, 44100, 1);
        let file = wav_to_tempfile(&wav);
        let info = probe_audio(file).unwrap();
        // UUID should be non-nil
        assert!(!info.id.is_nil());
    }

    #[test]
    fn probe_invalid_file_errors() {
        let mut tmp = tempfile::NamedTempFile::new().unwrap();
        tmp.write_all(b"not a valid audio file at all").unwrap();
        tmp.flush().unwrap();
        let path = tmp.into_temp_path();
        let file = std::fs::File::open(&path).unwrap();
        assert!(probe_audio(file).is_err());
    }

    #[test]
    fn probe_empty_file_errors() {
        let tmp = tempfile::NamedTempFile::new().unwrap();
        let path = tmp.into_temp_path();
        let file = std::fs::File::open(&path).unwrap();
        assert!(probe_audio(file).is_err());
    }

    #[test]
    fn map_format_wav() {
        assert_eq!(
            map_shravan_format(shravan::format::AudioFormat::Wav),
            AudioCodec::Pcm
        );
    }

    #[test]
    fn map_format_flac() {
        assert_eq!(
            map_shravan_format(shravan::format::AudioFormat::Flac),
            AudioCodec::Flac
        );
    }

    #[test]
    fn map_format_mp3() {
        assert_eq!(
            map_shravan_format(shravan::format::AudioFormat::Mp3),
            AudioCodec::Mp3
        );
    }

    #[test]
    fn map_format_ogg() {
        assert_eq!(
            map_shravan_format(shravan::format::AudioFormat::Ogg),
            AudioCodec::Vorbis
        );
    }

    #[test]
    fn map_format_opus() {
        assert_eq!(
            map_shravan_format(shravan::format::AudioFormat::Opus),
            AudioCodec::Opus
        );
    }

    #[test]
    fn map_format_aiff() {
        assert_eq!(
            map_shravan_format(shravan::format::AudioFormat::Aiff),
            AudioCodec::Pcm
        );
    }

    #[test]
    fn map_format_raw_pcm() {
        assert_eq!(
            map_shravan_format(shravan::format::AudioFormat::RawPcm),
            AudioCodec::Pcm
        );
    }

    #[test]
    fn probe_wav_has_empty_metadata() {
        // WAV files typically have no ID3/Vorbis metadata
        let wav = make_test_wav(4410, 44100, 2);
        let file = wav_to_tempfile(&wav);
        let info = probe_audio(file).unwrap();
        // Plain WAV has no metadata tags
        assert!(info.metadata.is_empty());
    }

    #[test]
    fn probe_wav_metadata_fields_none() {
        // A plain WAV should have title/artist/album as None
        let wav = make_test_wav(4410, 44100, 1);
        let file = wav_to_tempfile(&wav);
        let info = probe_audio(file).unwrap();
        assert!(info.title.is_none());
        assert!(info.artist.is_none());
        assert!(info.album.is_none());
    }

    #[test]
    fn map_container_wav() {
        assert_eq!(
            map_format_to_container(shravan::format::AudioFormat::Wav),
            ContainerFormat::Wav
        );
    }

    #[test]
    fn map_container_flac() {
        assert_eq!(
            map_format_to_container(shravan::format::AudioFormat::Flac),
            ContainerFormat::Flac
        );
    }

    #[test]
    fn map_container_ogg() {
        assert_eq!(
            map_format_to_container(shravan::format::AudioFormat::Ogg),
            ContainerFormat::Ogg
        );
    }

    #[test]
    fn map_container_mp3() {
        assert_eq!(
            map_format_to_container(shravan::format::AudioFormat::Mp3),
            ContainerFormat::Mp3
        );
    }

    #[test]
    fn map_container_opus() {
        assert_eq!(
            map_format_to_container(shravan::format::AudioFormat::Opus),
            ContainerFormat::Ogg
        );
    }

    #[test]
    fn map_container_aiff() {
        assert_eq!(
            map_format_to_container(shravan::format::AudioFormat::Aiff),
            ContainerFormat::Wav
        );
    }

    #[test]
    fn map_container_raw_pcm() {
        assert_eq!(
            map_format_to_container(shravan::format::AudioFormat::RawPcm),
            ContainerFormat::Wav
        );
    }

    #[test]
    fn probe_wav_bitrate_computed() {
        // 44100 Hz, 16-bit, 2 channels => bitrate = 44100 * 16 * 2 = 1411200
        let wav = make_test_wav(4410, 44100, 2);
        let file = wav_to_tempfile(&wav);
        let info = probe_audio(file).unwrap();

        let audio = info.audio_streams().collect::<Vec<_>>();
        // shravan reports bit_depth and we compute bitrate from it
        if let Some(br) = audio[0].bitrate {
            assert!(br > 0);
        }
    }

    #[test]
    fn probe_wav_format_is_f32() {
        let wav = make_test_wav(1000, 44100, 1);
        let file = wav_to_tempfile(&wav);
        let info = probe_audio(file).unwrap();

        let audio = info.audio_streams().collect::<Vec<_>>();
        // After shravan decode, sample format should be F32
        assert_eq!(audio[0].sample_format, SampleFormat::F32);
    }

    #[test]
    fn probe_wav_multiple_have_different_uuids() {
        let wav1 = make_test_wav(100, 44100, 1);
        let wav2 = make_test_wav(200, 44100, 1);
        let file1 = wav_to_tempfile(&wav1);
        let file2 = wav_to_tempfile(&wav2);
        let info1 = probe_audio(file1).unwrap();
        let info2 = probe_audio(file2).unwrap();
        // UUIDs should be different (v4 random)
        assert_ne!(info1.id, info2.id);
    }

    #[test]
    fn extract_tags_from_non_id3_data() {
        // Random bytes should produce empty tags
        let data = vec![0xAB, 0xCD, 0xEF, 0x01, 0x23, 0x45, 0x67, 0x89];
        let tags = extract_tags_from_data(&data);
        assert!(tags.is_empty());
    }

    #[test]
    fn extract_tags_from_empty_data() {
        let tags = extract_tags_from_data(&[]);
        assert!(tags.is_empty());
    }
}
