//! Audio file probing via symphonia

use tarang_core::{
    AudioCodec, AudioStreamInfo, ContainerFormat, MediaInfo, Result, SampleFormat, StreamInfo,
    TarangError,
};

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
                .map(|b| sample_rate * channels as u32 * b),
            duration,
        }));
    }

    let duration = streams.iter().find_map(|s| match s {
        StreamInfo::Audio(a) => a.duration,
        _ => None,
    });

    Ok(MediaInfo {
        id: uuid::Uuid::new_v4(),
        format: ContainerFormat::Mp4, // symphonia determines actual format
        streams,
        duration,
        file_size: None,
        title: None,
        artist: None,
        album: None,
    })
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
