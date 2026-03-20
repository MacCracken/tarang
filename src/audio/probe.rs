//! Audio file probing via symphonia

use crate::core::{
    AudioCodec, AudioStreamInfo, ContainerFormat, MediaInfo, Result, SampleFormat, StreamInfo,
    TarangError,
};
use std::collections::HashMap;

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
        .map_err(|e| TarangError::DemuxError(format!("symphonia probe failed: {e}").into()))?;

    let mut format = probed.format;
    let mut metadata_log = probed.metadata;

    // Extract metadata tags from both the probe metadata log and the format reader
    let mut tags = HashMap::new();
    if let Some(meta) = metadata_log.get() {
        if let Some(rev) = meta.current() {
            extract_tags(rev, &mut tags);
        }
    }
    if let Some(rev) = format.metadata().current() {
        extract_tags(rev, &mut tags);
    }

    // Detect container format from symphonia's codec registry name
    let container = {
        let name = format.default_track().map(|t| t.codec_params.codec);
        // Symphonia doesn't directly expose container format, but we can infer
        // from the format reader's track codec types and the probe result.
        // For now, map based on the first track's codec.
        match name {
            Some(c) if c == symphonia::core::codecs::CODEC_TYPE_FLAC => ContainerFormat::Flac,
            Some(c) if c == symphonia::core::codecs::CODEC_TYPE_VORBIS => ContainerFormat::Ogg,
            Some(c) if c == symphonia::core::codecs::CODEC_TYPE_OPUS => ContainerFormat::Ogg,
            Some(c) if c == symphonia::core::codecs::CODEC_TYPE_MP3 => ContainerFormat::Mp3,
            Some(c)
                if c == symphonia::core::codecs::CODEC_TYPE_PCM_S16LE
                    || c == symphonia::core::codecs::CODEC_TYPE_PCM_S16BE
                    || c == symphonia::core::codecs::CODEC_TYPE_PCM_S24LE
                    || c == symphonia::core::codecs::CODEC_TYPE_PCM_S32LE
                    || c == symphonia::core::codecs::CODEC_TYPE_PCM_F32LE
                    || c == symphonia::core::codecs::CODEC_TYPE_PCM_F64LE =>
            {
                ContainerFormat::Wav
            }
            Some(c) if c == symphonia::core::codecs::CODEC_TYPE_AAC => ContainerFormat::Mp4,
            Some(c) if c == symphonia::core::codecs::CODEC_TYPE_ALAC => ContainerFormat::Mp4,
            _ => ContainerFormat::Mp4, // fallback
        }
    };

    let mut streams = Vec::new();

    for track in format.tracks() {
        let params = &track.codec_params;
        let codec = map_symphonia_codec(params.codec);
        let Some(codec) = codec else { continue };

        let sample_rate = params.sample_rate.unwrap_or(44100);
        let channels = params.channels.map(|c| c.count() as u16).unwrap_or(2);

        let duration = params
            .n_frames
            .map(|n| std::time::Duration::from_secs_f64(n as f64 / sample_rate as f64));

        streams.push(StreamInfo::Audio(AudioStreamInfo {
            codec,
            sample_rate,
            channels,
            sample_format: SampleFormat::F32,
            bitrate: params
                .bits_per_coded_sample
                .and_then(|b| sample_rate.checked_mul(channels as u32)?.checked_mul(b)),
            duration,
        }));
    }

    let duration = streams.iter().find_map(|s| match s {
        StreamInfo::Audio(a) => a.duration,
        _ => None,
    });

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

/// Extract metadata tags from a symphonia metadata revision into a HashMap.
fn extract_tags(
    rev: &symphonia::core::meta::MetadataRevision,
    tags: &mut HashMap<String, String>,
) {
    use symphonia::core::meta::StandardTagKey;

    for tag in rev.tags() {
        let value = tag.value.to_string();
        if value.is_empty() {
            continue;
        }

        // Map standard tag keys to well-known names
        if let Some(std_key) = tag.std_key {
            let key = match std_key {
                StandardTagKey::TrackTitle => "title",
                StandardTagKey::Artist => "artist",
                StandardTagKey::Album => "album",
                StandardTagKey::TrackNumber => "tracknumber",
                StandardTagKey::Date => "date",
                StandardTagKey::Genre => "genre",
                StandardTagKey::Composer => "composer",
                StandardTagKey::AlbumArtist => "album_artist",
                StandardTagKey::Comment => "comment",
                _ => continue,
            };
            tags.insert(key.to_string(), value);
        }
    }
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

/// Map a symphonia codec type to our AudioCodec enum
pub(crate) fn map_symphonia_codec(codec: symphonia::core::codecs::CodecType) -> Option<AudioCodec> {
    use symphonia::core::codecs::*;
    match codec {
        CODEC_TYPE_FLAC => Some(AudioCodec::Flac),
        CODEC_TYPE_MP3 => Some(AudioCodec::Mp3),
        CODEC_TYPE_VORBIS => Some(AudioCodec::Vorbis),
        CODEC_TYPE_AAC => Some(AudioCodec::Aac),
        CODEC_TYPE_ALAC => Some(AudioCodec::Alac),
        CODEC_TYPE_OPUS => Some(AudioCodec::Opus),
        CODEC_TYPE_PCM_S16LE | CODEC_TYPE_PCM_S16BE | CODEC_TYPE_PCM_S24LE
        | CODEC_TYPE_PCM_S32LE | CODEC_TYPE_PCM_F32LE | CODEC_TYPE_PCM_F64LE => {
            Some(AudioCodec::Pcm)
        }
        _ => None,
    }
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
    fn map_codec_flac() {
        use symphonia::core::codecs::*;
        assert_eq!(map_symphonia_codec(CODEC_TYPE_FLAC), Some(AudioCodec::Flac));
    }

    #[test]
    fn map_codec_mp3() {
        use symphonia::core::codecs::*;
        assert_eq!(map_symphonia_codec(CODEC_TYPE_MP3), Some(AudioCodec::Mp3));
    }

    #[test]
    fn map_codec_vorbis() {
        use symphonia::core::codecs::*;
        assert_eq!(
            map_symphonia_codec(CODEC_TYPE_VORBIS),
            Some(AudioCodec::Vorbis)
        );
    }

    #[test]
    fn map_codec_aac() {
        use symphonia::core::codecs::*;
        assert_eq!(map_symphonia_codec(CODEC_TYPE_AAC), Some(AudioCodec::Aac));
    }

    #[test]
    fn map_codec_opus() {
        use symphonia::core::codecs::*;
        assert_eq!(map_symphonia_codec(CODEC_TYPE_OPUS), Some(AudioCodec::Opus));
    }

    #[test]
    fn map_codec_pcm_variants() {
        use symphonia::core::codecs::*;
        for codec in [
            CODEC_TYPE_PCM_S16LE,
            CODEC_TYPE_PCM_S16BE,
            CODEC_TYPE_PCM_S24LE,
            CODEC_TYPE_PCM_S32LE,
            CODEC_TYPE_PCM_F32LE,
            CODEC_TYPE_PCM_F64LE,
        ] {
            assert_eq!(map_symphonia_codec(codec), Some(AudioCodec::Pcm));
        }
    }

    #[test]
    fn map_codec_unknown_returns_none() {
        use symphonia::core::codecs::CODEC_TYPE_NULL;
        assert_eq!(map_symphonia_codec(CODEC_TYPE_NULL), None);
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
    fn extract_tags_populates_hashmap() {
        use symphonia::core::meta::{MetadataBuilder, StandardTagKey, Tag, Value};

        let mut builder = MetadataBuilder::new();
        builder.add_tag(Tag::new(
            Some(StandardTagKey::TrackTitle),
            "TITLE",
            Value::from("Test Song"),
        ));
        builder.add_tag(Tag::new(
            Some(StandardTagKey::Artist),
            "ARTIST",
            Value::from("Test Artist"),
        ));
        builder.add_tag(Tag::new(
            Some(StandardTagKey::Album),
            "ALBUM",
            Value::from("Test Album"),
        ));
        builder.add_tag(Tag::new(
            Some(StandardTagKey::Genre),
            "GENRE",
            Value::from("Rock"),
        ));
        builder.add_tag(Tag::new(
            Some(StandardTagKey::TrackNumber),
            "TRACKNUMBER",
            Value::from("3"),
        ));

        let rev = builder.metadata();
        let mut tags = HashMap::new();
        super::extract_tags(&rev, &mut tags);

        assert_eq!(tags.get("title").unwrap(), "Test Song");
        assert_eq!(tags.get("artist").unwrap(), "Test Artist");
        assert_eq!(tags.get("album").unwrap(), "Test Album");
        assert_eq!(tags.get("genre").unwrap(), "Rock");
        assert_eq!(tags.get("tracknumber").unwrap(), "3");
    }

    #[test]
    fn extract_tags_skips_empty_values() {
        use symphonia::core::meta::{MetadataBuilder, StandardTagKey, Tag, Value};

        let mut builder = MetadataBuilder::new();
        builder.add_tag(Tag::new(
            Some(StandardTagKey::TrackTitle),
            "TITLE",
            Value::from(""),
        ));
        builder.add_tag(Tag::new(
            Some(StandardTagKey::Artist),
            "ARTIST",
            Value::from("Valid Artist"),
        ));

        let rev = builder.metadata();
        let mut tags = HashMap::new();
        super::extract_tags(&rev, &mut tags);

        assert!(!tags.contains_key("title"));
        assert_eq!(tags.get("artist").unwrap(), "Valid Artist");
    }

    #[test]
    fn extract_tags_skips_unknown_standard_keys() {
        use symphonia::core::meta::{MetadataBuilder, Tag, Value};

        let mut builder = MetadataBuilder::new();
        // Tag with no standard key
        builder.add_tag(Tag::new(None, "CUSTOM_TAG", Value::from("custom value")));

        let rev = builder.metadata();
        let mut tags = HashMap::new();
        super::extract_tags(&rev, &mut tags);

        assert!(tags.is_empty());
    }
}
