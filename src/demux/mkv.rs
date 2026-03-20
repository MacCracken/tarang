//! Matroska/WebM container demuxer (pure Rust)
//!
//! Parses EBML-encoded Matroska containers to extract audio and video stream
//! metadata and produce raw codec packets from Clusters.

use crate::core::{
    AudioCodec, AudioStreamInfo, ContainerFormat, MediaInfo, PixelFormat, Result, SampleFormat,
    StreamInfo, TarangError, VideoCodec, VideoStreamInfo,
};
use bytes::Bytes;
use std::collections::HashMap;
use std::io::{Read, Seek, SeekFrom};
use std::time::Duration;
use uuid::Uuid;

use super::{Demuxer, Packet};

// EBML Element IDs
const EBML_HEADER: u32 = 0x1A45_DFA3;
const SEGMENT: u32 = 0x1853_8067;
const INFO: u32 = 0x1549_A966;
const TIMECODE_SCALE: u32 = 0x2AD7B1;
const DURATION: u32 = 0x4489;
const TRACKS: u32 = 0x1654_AE6B;
const TRACK_ENTRY: u32 = 0xAE;
const TRACK_NUMBER: u32 = 0xD7;
const TRACK_TYPE: u32 = 0x83;
const CODEC_ID: u32 = 0x86;
const AUDIO: u32 = 0xE1;
const SAMPLING_FREQ: u32 = 0xB5;
const CHANNELS: u32 = 0x9F;
const BIT_DEPTH: u32 = 0x6264;
const VIDEO: u32 = 0xE0;
const PIXEL_WIDTH: u32 = 0xB0;
const PIXEL_HEIGHT: u32 = 0xBA;
const CLUSTER: u32 = 0x1F43_B675;
const TIMECODE: u32 = 0xE7;
const SIMPLE_BLOCK: u32 = 0xA3;
const DOC_TYPE: u32 = 0x4282;
const LANGUAGE: u32 = 0x22B59C;

// Chapter element IDs
const CHAPTERS: u32 = 0x1043_A770;
const EDITION_ENTRY: u32 = 0x45B9;
const CHAPTER_ATOM: u32 = 0xB6;
const CHAPTER_TIME_START: u32 = 0x91;
const CHAPTER_DISPLAY: u32 = 0x80;
const CHAPTER_STRING: u32 = 0x85;

/// Track type values
const TRACK_TYPE_VIDEO: u64 = 1;
const TRACK_TYPE_AUDIO: u64 = 2;
const TRACK_TYPE_SUBTITLE: u64 = 0x11;

/// Parsed MKV track
#[derive(Debug, Clone)]
struct MkvTrack {
    number: u64,
    track_type: u64,
    codec_id: String,
    // Audio fields
    sample_rate: f64,
    channels: u64,
    bit_depth: u64,
    // Video fields
    width: u64,
    height: u64,
    // Subtitle/general fields
    language: Option<String>,
}

/// A chapter marker within an MKV file.
#[derive(Debug, Clone)]
pub struct MkvChapter {
    /// Chapter start time in nanoseconds.
    pub time_start_ns: u64,
    /// Chapter title, if present.
    pub title: Option<String>,
}

/// MKV/WebM demuxer
pub struct MkvDemuxer<R: Read + Seek> {
    reader: R,
    tracks: Vec<MkvTrack>,
    info: Option<MediaInfo>,
    timecode_scale: u64, // nanoseconds per timecode unit (default 1_000_000 = 1ms)
    duration_timecode: f64,
    is_webm: bool,
    /// Offset where clusters begin
    cluster_offset: u64,
    /// Current cluster timecode
    current_cluster_timecode: u64,
    segment_offset: u64,
    segment_size: u64,
    /// Track number -> stream index lookup for O(1) access
    track_map: HashMap<u64, usize>,
    /// Reusable buffer for reading packet data, avoiding per-packet allocation
    packet_buf: Vec<u8>,
    /// Parsed chapters (from Chapters element).
    chapters: Vec<MkvChapter>,
}

impl<R: Read + Seek> MkvDemuxer<R> {
    pub fn new(reader: R) -> Self {
        Self {
            reader,
            tracks: Vec::new(),
            info: None,
            timecode_scale: 1_000_000, // default 1ms
            duration_timecode: 0.0,
            is_webm: false,
            cluster_offset: 0,
            current_cluster_timecode: 0,
            segment_offset: 0,
            segment_size: 0,
            track_map: HashMap::new(),
            packet_buf: Vec::new(),
            chapters: Vec::new(),
        }
    }

    /// Skip `size` bytes forward safely, validating the value fits in i64.
    fn skip_bytes(&mut self, size: u64) -> Result<()> {
        if size > i64::MAX as u64 {
            return Err(TarangError::DemuxError(
                format!("element size {size} exceeds seekable range").into(),
            ));
        }
        self.reader
            .seek(SeekFrom::Current(size as i64))
            .map_err(io_err)?;
        Ok(())
    }

    /// Read a variable-length EBML integer (VINT).
    /// Returns (value, bytes_consumed).
    fn read_vint(&mut self) -> Result<(u64, usize)> {
        let mut first = [0u8; 1];
        self.reader
            .read_exact(&mut first)
            .map_err(|e| TarangError::DemuxError(format!("EBML read error: {e}").into()))?;

        let b = first[0];
        if b == 0 {
            return Err(TarangError::DemuxError("invalid VINT: zero".into()));
        }

        let len = b.leading_zeros() as usize + 1;
        if len > 8 {
            return Err(TarangError::DemuxError(
                format!("invalid VINT length: {len}").into(),
            ));
        }

        let mut value = (b as u64) & ((1u64 << (8 - len)) - 1);

        for _ in 1..len {
            let mut next = [0u8; 1];
            self.reader
                .read_exact(&mut next)
                .map_err(|e| TarangError::DemuxError(format!("EBML read error: {e}").into()))?;
            value = (value << 8) | next[0] as u64;
        }

        Ok((value, len))
    }

    /// Read an EBML element ID.
    fn read_element_id(&mut self) -> Result<(u32, usize)> {
        let mut first = [0u8; 1];
        self.reader
            .read_exact(&mut first)
            .map_err(|e| TarangError::DemuxError(format!("EBML read error: {e}").into()))?;

        let b = first[0];
        let len = b.leading_zeros() as usize + 1;
        if len > 4 || len == 0 {
            return Err(TarangError::DemuxError(
                format!("invalid EBML ID length: {len} (byte=0x{b:02X})").into(),
            ));
        }

        let mut value = b as u32;
        for _ in 1..len {
            let mut next = [0u8; 1];
            self.reader
                .read_exact(&mut next)
                .map_err(|e| TarangError::DemuxError(format!("EBML read error: {e}").into()))?;
            value = (value << 8) | next[0] as u32;
        }

        Ok((value, len))
    }

    /// Read an EBML element data size.
    fn read_element_size(&mut self) -> Result<(u64, usize)> {
        self.read_vint()
    }

    /// Read an unsigned integer of `size` bytes.
    fn read_uint(&mut self, size: u64) -> Result<u64> {
        let mut value = 0u64;
        for _ in 0..size {
            let mut b = [0u8; 1];
            self.reader
                .read_exact(&mut b)
                .map_err(|e| TarangError::DemuxError(format!("read error: {e}").into()))?;
            value = (value << 8) | b[0] as u64;
        }
        Ok(value)
    }

    /// Read a float of `size` bytes (4 or 8).
    fn read_float(&mut self, size: u64) -> Result<f64> {
        match size {
            4 => {
                let mut buf = [0u8; 4];
                self.reader
                    .read_exact(&mut buf)
                    .map_err(|e| TarangError::DemuxError(format!("read error: {e}").into()))?;
                Ok(f32::from_be_bytes(buf) as f64)
            }
            8 => {
                let mut buf = [0u8; 8];
                self.reader
                    .read_exact(&mut buf)
                    .map_err(|e| TarangError::DemuxError(format!("read error: {e}").into()))?;
                Ok(f64::from_be_bytes(buf))
            }
            _ => Err(TarangError::DemuxError(
                format!("invalid float size: {size}").into(),
            )),
        }
    }

    /// Maximum string allocation size (64 KiB) to prevent OOM from malformed files.
    const MAX_STRING_SIZE: u64 = 65536;

    /// Maximum number of tracks to prevent excessive memory use.
    const MAX_TRACKS: usize = 128;

    /// Read a UTF-8 string of `size` bytes.
    fn read_string(&mut self, size: u64) -> Result<String> {
        if size > Self::MAX_STRING_SIZE {
            return Err(TarangError::DemuxError(
                format!(
                    "string size {size} exceeds maximum ({})",
                    Self::MAX_STRING_SIZE
                )
                .into(),
            ));
        }
        let mut buf = vec![0u8; size as usize];
        self.reader
            .read_exact(&mut buf)
            .map_err(|e| TarangError::DemuxError(format!("read error: {e}").into()))?;
        // Strip trailing nulls
        while buf.last() == Some(&0) {
            buf.pop();
        }
        String::from_utf8(buf)
            .map_err(|e| TarangError::DemuxError(format!("invalid UTF-8: {e}").into()))
    }

    /// Parse the EBML header to identify MKV vs WebM.
    fn parse_ebml_header(&mut self) -> Result<()> {
        let (id, _) = self.read_element_id()?;
        if id != EBML_HEADER {
            return Err(TarangError::UnsupportedFormat(
                "not a Matroska/WebM file: missing EBML header".into(),
            ));
        }

        let (header_size, _) = self.read_element_size()?;
        let header_end = self.reader.stream_position().map_err(io_err)? + header_size;

        while self.reader.stream_position().map_err(io_err)? < header_end {
            let (eid, _) = self.read_element_id()?;
            let (esize, _) = self.read_element_size()?;

            if eid == DOC_TYPE {
                let doc = self.read_string(esize)?;
                self.is_webm = doc == "webm";
            } else {
                self.skip_bytes(esize)?;
            }
        }

        Ok(())
    }

    /// Parse the Segment Info element.
    fn parse_info(&mut self, size: u64) -> Result<()> {
        let end = self.reader.stream_position().map_err(io_err)? + size;

        while self.reader.stream_position().map_err(io_err)? < end {
            let (eid, _) = self.read_element_id()?;
            let (esize, _) = self.read_element_size()?;

            match eid {
                TIMECODE_SCALE => {
                    self.timecode_scale = self.read_uint(esize)?;
                }
                DURATION => {
                    self.duration_timecode = self.read_float(esize)?;
                }
                _ => {
                    self.skip_bytes(esize)?;
                }
            }
        }
        Ok(())
    }

    /// Parse the Tracks element.
    fn parse_tracks(&mut self, size: u64) -> Result<()> {
        let end = self.reader.stream_position().map_err(io_err)? + size;

        while self.reader.stream_position().map_err(io_err)? < end {
            let (eid, _) = self.read_element_id()?;
            let (esize, _) = self.read_element_size()?;

            if eid == TRACK_ENTRY {
                self.parse_track_entry(esize)?;
            } else {
                self.skip_bytes(esize)?;
            }
        }
        Ok(())
    }

    /// Parse a single TrackEntry.
    fn parse_track_entry(&mut self, size: u64) -> Result<()> {
        let end = self.reader.stream_position().map_err(io_err)? + size;

        let mut track = MkvTrack {
            number: 0,
            track_type: 0,
            codec_id: String::new(),
            sample_rate: 0.0,
            channels: 0,
            bit_depth: 0,
            width: 0,
            height: 0,
            language: None,
        };

        while self.reader.stream_position().map_err(io_err)? < end {
            let (eid, _) = self.read_element_id()?;
            let (esize, _) = self.read_element_size()?;

            match eid {
                TRACK_NUMBER => track.number = self.read_uint(esize)?,
                TRACK_TYPE => track.track_type = self.read_uint(esize)?,
                CODEC_ID => track.codec_id = self.read_string(esize)?,
                LANGUAGE => track.language = Some(self.read_string(esize)?),
                AUDIO => self.parse_audio_settings(esize, &mut track)?,
                VIDEO => self.parse_video_settings(esize, &mut track)?,
                _ => {
                    self.skip_bytes(esize)?;
                }
            }
        }

        if track.number > 0 {
            if self.tracks.len() >= Self::MAX_TRACKS {
                return Err(TarangError::DemuxError(
                    format!("too many tracks: exceeds maximum ({})", Self::MAX_TRACKS).into(),
                ));
            }
            self.tracks.push(track);
        }
        Ok(())
    }

    fn parse_audio_settings(&mut self, size: u64, track: &mut MkvTrack) -> Result<()> {
        let end = self.reader.stream_position().map_err(io_err)? + size;

        while self.reader.stream_position().map_err(io_err)? < end {
            let (eid, _) = self.read_element_id()?;
            let (esize, _) = self.read_element_size()?;

            match eid {
                SAMPLING_FREQ => track.sample_rate = self.read_float(esize)?,
                CHANNELS => track.channels = self.read_uint(esize)?,
                BIT_DEPTH => track.bit_depth = self.read_uint(esize)?,
                _ => {
                    self.skip_bytes(esize)?;
                }
            }
        }
        Ok(())
    }

    fn parse_video_settings(&mut self, size: u64, track: &mut MkvTrack) -> Result<()> {
        let end = self.reader.stream_position().map_err(io_err)? + size;

        while self.reader.stream_position().map_err(io_err)? < end {
            let (eid, _) = self.read_element_id()?;
            let (esize, _) = self.read_element_size()?;

            match eid {
                PIXEL_WIDTH => track.width = self.read_uint(esize)?,
                PIXEL_HEIGHT => track.height = self.read_uint(esize)?,
                _ => {
                    self.skip_bytes(esize)?;
                }
            }
        }
        Ok(())
    }

    /// Map a Matroska CodecID to our types.
    fn map_audio_codec(codec_id: &str) -> Option<AudioCodec> {
        match codec_id {
            "A_VORBIS" => Some(AudioCodec::Vorbis),
            "A_OPUS" => Some(AudioCodec::Opus),
            "A_FLAC" => Some(AudioCodec::Flac),
            "A_AAC" | "A_AAC/MPEG2/MAIN" | "A_AAC/MPEG4/MAIN" | "A_AAC/MPEG4/LC" => {
                Some(AudioCodec::Aac)
            }
            "A_MPEG/L3" => Some(AudioCodec::Mp3),
            "A_PCM/INT/LIT" | "A_PCM/INT/BIG" | "A_PCM/FLOAT/IEEE" => Some(AudioCodec::Pcm),
            "A_ALAC" => Some(AudioCodec::Alac),
            _ => None,
        }
    }

    fn map_video_codec(codec_id: &str) -> Option<VideoCodec> {
        match codec_id {
            "V_AV1" => Some(VideoCodec::Av1),
            "V_VP8" => Some(VideoCodec::Vp8),
            "V_VP9" => Some(VideoCodec::Vp9),
            "V_MPEG4/ISO/AVC" => Some(VideoCodec::H264),
            "V_MPEGH/ISO/HEVC" => Some(VideoCodec::H265),
            "V_THEORA" => Some(VideoCodec::Theora),
            _ => None,
        }
    }

    /// Parse a Chapters element, extracting chapter time and title.
    fn parse_chapters(&mut self, size: u64) -> Result<()> {
        let end = self.reader.stream_position().map_err(io_err)? + size;

        while self.reader.stream_position().map_err(io_err)? < end {
            let (eid, _) = match self.read_element_id() {
                Ok(v) => v,
                Err(_) => break,
            };
            let (esize, _) = match self.read_element_size() {
                Ok(v) => v,
                Err(_) => break,
            };

            if eid == EDITION_ENTRY {
                self.parse_edition_entry(esize)?;
            } else {
                self.skip_bytes(esize)?;
            }
        }
        Ok(())
    }

    fn parse_edition_entry(&mut self, size: u64) -> Result<()> {
        let end = self.reader.stream_position().map_err(io_err)? + size;

        while self.reader.stream_position().map_err(io_err)? < end {
            let (eid, _) = match self.read_element_id() {
                Ok(v) => v,
                Err(_) => break,
            };
            let (esize, _) = match self.read_element_size() {
                Ok(v) => v,
                Err(_) => break,
            };

            if eid == CHAPTER_ATOM {
                self.parse_chapter_atom(esize)?;
            } else {
                self.skip_bytes(esize)?;
            }
        }
        Ok(())
    }

    fn parse_chapter_atom(&mut self, size: u64) -> Result<()> {
        let end = self.reader.stream_position().map_err(io_err)? + size;
        let mut time_start: u64 = 0;
        let mut title: Option<String> = None;

        while self.reader.stream_position().map_err(io_err)? < end {
            let (eid, _) = match self.read_element_id() {
                Ok(v) => v,
                Err(_) => break,
            };
            let (esize, _) = match self.read_element_size() {
                Ok(v) => v,
                Err(_) => break,
            };

            match eid {
                CHAPTER_TIME_START => {
                    time_start = self.read_uint(esize)?;
                }
                CHAPTER_DISPLAY => {
                    // Parse ChapterDisplay to get ChapString
                    let disp_end = self.reader.stream_position().map_err(io_err)? + esize;
                    while self.reader.stream_position().map_err(io_err)? < disp_end {
                        let (did, _) = match self.read_element_id() {
                            Ok(v) => v,
                            Err(_) => break,
                        };
                        let (dsize, _) = match self.read_element_size() {
                            Ok(v) => v,
                            Err(_) => break,
                        };
                        if did == CHAPTER_STRING {
                            title = Some(self.read_string(dsize)?);
                        } else {
                            self.skip_bytes(dsize)?;
                        }
                    }
                }
                _ => {
                    self.skip_bytes(esize)?;
                }
            }
        }

        self.chapters.push(MkvChapter {
            time_start_ns: time_start,
            title,
        });
        Ok(())
    }

    /// Access parsed chapters.
    pub fn chapters(&self) -> &[MkvChapter] {
        &self.chapters
    }

    /// Calculate duration in seconds from timecode scale and duration value.
    fn duration_secs(&self) -> Option<Duration> {
        if self.duration_timecode > 0.0 && self.timecode_scale > 0 {
            let ns = self.duration_timecode * self.timecode_scale as f64;
            Some(Duration::from_secs_f64(ns / 1_000_000_000.0))
        } else {
            None
        }
    }

    fn timecode_to_duration(&self, tc: u64) -> Duration {
        let ns = tc as f64 * self.timecode_scale as f64;
        Duration::from_secs_f64(ns / 1_000_000_000.0)
    }
}

impl<R: Read + Seek> Demuxer for MkvDemuxer<R> {
    fn probe(&mut self) -> Result<MediaInfo> {
        self.reader.seek(SeekFrom::Start(0)).map_err(io_err)?;
        self.tracks.clear();

        // Parse EBML header
        self.parse_ebml_header()?;

        // Parse Segment
        let (seg_id, _) = self.read_element_id()?;
        if seg_id != SEGMENT {
            return Err(TarangError::UnsupportedFormat(
                "missing Segment element".into(),
            ));
        }
        let (seg_size, _) = self.read_element_size()?;
        self.segment_offset = self.reader.stream_position().map_err(io_err)?;
        self.segment_size = seg_size;

        let segment_end = self.segment_offset + seg_size;

        // Parse top-level Segment children until we find Tracks + Info
        let mut found_tracks = false;
        let mut _found_info = false;

        while self.reader.stream_position().map_err(io_err)? < segment_end {
            let pos = self.reader.stream_position().map_err(io_err)?;
            let (eid, _) = match self.read_element_id() {
                Ok(v) => v,
                Err(_) => break,
            };
            let (esize, _) = match self.read_element_size() {
                Ok(v) => v,
                Err(_) => break,
            };

            match eid {
                INFO => {
                    self.parse_info(esize)?;
                    _found_info = true;
                }
                TRACKS => {
                    self.parse_tracks(esize)?;
                    found_tracks = true;
                }
                CHAPTERS => {
                    self.parse_chapters(esize)?;
                }
                CLUSTER => {
                    // First cluster — record its position and stop parsing headers
                    self.cluster_offset = pos;
                    break;
                }
                _ => {
                    self.skip_bytes(esize)?;
                }
            }
        }

        if !found_tracks {
            return Err(TarangError::DemuxError("no Tracks element found".into()));
        }

        // Build track number -> stream index map for O(1) lookup
        self.track_map.clear();
        for (idx, track) in self.tracks.iter().enumerate() {
            self.track_map.insert(track.number, idx);
        }

        let duration = self.duration_secs();

        // Build StreamInfo from tracks
        let streams: Vec<StreamInfo> = self
            .tracks
            .iter()
            .filter_map(|t| match t.track_type {
                TRACK_TYPE_AUDIO => {
                    let codec = Self::map_audio_codec(&t.codec_id)?;
                    Some(StreamInfo::Audio(AudioStreamInfo {
                        codec,
                        sample_rate: t.sample_rate as u32,
                        channels: t.channels as u16,
                        sample_format: SampleFormat::F32,
                        bitrate: None,
                        duration,
                    }))
                }
                TRACK_TYPE_VIDEO => {
                    let codec = Self::map_video_codec(&t.codec_id)?;
                    Some(StreamInfo::Video(VideoStreamInfo {
                        codec,
                        width: t.width as u32,
                        height: t.height as u32,
                        pixel_format: PixelFormat::Yuv420p,
                        frame_rate: 0.0, // MKV stores frame duration per-block, not global rate
                        bitrate: None,
                        duration,
                    }))
                }
                TRACK_TYPE_SUBTITLE => {
                    Some(StreamInfo::Subtitle {
                        language: t.language.clone(),
                    })
                }
                _ => None,
            })
            .collect();

        if streams.is_empty() {
            return Err(TarangError::DemuxError("no supported streams found".into()));
        }

        let format = if self.is_webm {
            ContainerFormat::WebM
        } else {
            ContainerFormat::Mkv
        };

        let info = MediaInfo {
            id: Uuid::new_v4(),
            format,
            streams,
            duration,
            file_size: None,
            title: None,
            artist: None,
            album: None,
            metadata: std::collections::HashMap::new(),
        };

        tracing::debug!(
            format = %info.format,
            streams = info.streams.len(),
            "MKV probe complete"
        );

        let ret = info.clone();
        self.info = Some(info);
        Ok(ret)
    }

    fn next_packet(&mut self) -> Result<Packet> {
        let segment_end = self.segment_offset + self.segment_size;

        loop {
            if self.reader.stream_position().map_err(io_err)? >= segment_end {
                return Err(TarangError::EndOfStream);
            }

            let (eid, _) = match self.read_element_id() {
                Ok(v) => v,
                Err(_) => return Err(TarangError::EndOfStream),
            };
            let (esize, _) = match self.read_element_size() {
                Ok(v) => v,
                Err(_) => return Err(TarangError::EndOfStream),
            };

            match eid {
                CLUSTER => {
                    // Enter cluster — don't skip, parse children
                    continue;
                }
                TIMECODE => {
                    self.current_cluster_timecode = self.read_uint(esize)?;
                    continue;
                }
                SIMPLE_BLOCK => {
                    return self.parse_simple_block(esize);
                }
                _ => {
                    self.skip_bytes(esize)?;
                }
            }
        }
    }

    fn seek(&mut self, timestamp: Duration) -> Result<()> {
        // Validate cluster_offset against a reasonable bound
        const MAX_REASONABLE_OFFSET: u64 = u64::MAX / 2;
        if self.cluster_offset > MAX_REASONABLE_OFFSET {
            return Err(TarangError::DemuxError(
                format!(
                    "cluster offset {} exceeds reasonable bound",
                    self.cluster_offset
                )
                .into(),
            ));
        }

        // Simple seek: scan clusters from the beginning
        self.reader
            .seek(SeekFrom::Start(self.cluster_offset))
            .map_err(io_err)?;

        let target_ns = timestamp.as_nanos() as u64;
        let segment_end = self.segment_offset + self.segment_size;

        while self.reader.stream_position().map_err(io_err)? < segment_end {
            let pos = self.reader.stream_position().map_err(io_err)?;
            let (eid, _) = match self.read_element_id() {
                Ok(v) => v,
                Err(_) => break,
            };
            let (esize, _) = match self.read_element_size() {
                Ok(v) => v,
                Err(_) => break,
            };

            if eid == CLUSTER {
                // Peek at cluster timecode
                let tc_pos = self.reader.stream_position().map_err(io_err)?;
                let (tc_eid, _) = self.read_element_id()?;
                let (tc_size, _) = self.read_element_size()?;

                if tc_eid == TIMECODE {
                    let tc = self.read_uint(tc_size)?;
                    let cluster_ns = tc * self.timecode_scale;
                    if cluster_ns > target_ns {
                        // Seek to previous cluster start
                        self.reader.seek(SeekFrom::Start(pos)).map_err(io_err)?;
                        return Ok(());
                    }
                    self.current_cluster_timecode = tc;
                } else {
                    self.reader.seek(SeekFrom::Start(tc_pos)).map_err(io_err)?;
                }
                continue;
            }

            self.skip_bytes(esize)?;
        }

        Ok(())
    }
}

impl<R: Read + Seek> MkvDemuxer<R> {
    fn parse_simple_block(&mut self, size: u64) -> Result<Packet> {
        let _start = self.reader.stream_position().map_err(io_err)?;

        // Track number (VINT)
        let (track_num, vint_len) = self.read_vint()?;

        // Relative timecode (i16, big-endian)
        let mut tc_buf = [0u8; 2];
        self.reader
            .read_exact(&mut tc_buf)
            .map_err(|e| TarangError::DemuxError(format!("read error: {e}").into()))?;
        let relative_tc = i16::from_be_bytes(tc_buf);

        // Flags
        let mut flags = [0u8; 1];
        self.reader
            .read_exact(&mut flags)
            .map_err(|e| TarangError::DemuxError(format!("read error: {e}").into()))?;
        let is_keyframe = flags[0] & 0x80 != 0;

        // Header size: vint_len + 2 (timecode) + 1 (flags)
        let header_size = vint_len as u64 + 3;
        if size < header_size {
            return Err(TarangError::DemuxError(
                format!("SimpleBlock size {size} smaller than header {header_size}").into(),
            ));
        }
        let data_size = size - header_size;

        // Cap SimpleBlock data to prevent OOM on malformed files
        const MAX_BLOCK_SIZE: u64 = 64 * 1024 * 1024; // 64 MB
        if data_size > MAX_BLOCK_SIZE {
            return Err(TarangError::DemuxError(
                format!("SimpleBlock data size {data_size} exceeds {MAX_BLOCK_SIZE} byte limit")
                    .into(),
            ));
        }

        self.packet_buf.clear();
        self.packet_buf.resize(data_size as usize, 0);
        self.reader
            .read_exact(&mut self.packet_buf)
            .map_err(|e| TarangError::DemuxError(format!("read error: {e}").into()))?;
        let data = Bytes::copy_from_slice(&self.packet_buf);

        // Absolute timecode (saturate to prevent overflow)
        let abs_tc = (self.current_cluster_timecode as i64).saturating_add(relative_tc as i64);
        let timestamp = self.timecode_to_duration(abs_tc.max(0) as u64);

        // Map track number to stream index via HashMap
        let stream_index = self.track_map.get(&track_num).copied().unwrap_or(0);

        Ok(Packet {
            stream_index,
            data,
            timestamp,
            duration: None,
            is_keyframe,
        })
    }
}

fn io_err(e: std::io::Error) -> TarangError {
    TarangError::DemuxError(format!("I/O error: {e}").into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::demux::ebml;
    use std::io::Cursor;

    fn write_vint(buf: &mut Vec<u8>, value: u64) {
        ebml::write_vint(buf, value);
    }

    fn write_id(buf: &mut Vec<u8>, id: u32) {
        ebml::write_id(buf, id);
    }

    fn write_uint_element(buf: &mut Vec<u8>, id: u32, value: u64) {
        ebml::write_uint(buf, id, value);
    }

    fn write_float_element(buf: &mut Vec<u8>, id: u32, value: f64) {
        ebml::write_float(buf, id, value);
    }

    fn write_string_element(buf: &mut Vec<u8>, id: u32, value: &str) {
        ebml::write_string(buf, id, value);
    }

    fn write_master_element(buf: &mut Vec<u8>, id: u32, children: &[u8]) {
        ebml::write_master(buf, id, children);
    }

    /// Build a minimal MKV file with one audio track.
    fn make_mkv_audio(codec_id: &str, sample_rate: f64, channels: u64) -> Vec<u8> {
        let mut file = Vec::new();

        // EBML Header
        let mut header = Vec::new();
        write_string_element(&mut header, DOC_TYPE, "matroska");
        write_master_element(&mut file, EBML_HEADER, &header);

        // Segment
        let mut segment = Vec::new();

        // Info
        let mut info = Vec::new();
        write_uint_element(&mut info, TIMECODE_SCALE, 1_000_000);
        write_float_element(&mut info, DURATION, 1000.0); // 1 second
        write_master_element(&mut segment, INFO, &info);

        // Tracks
        let mut tracks = Vec::new();
        let mut track_entry = Vec::new();
        write_uint_element(&mut track_entry, TRACK_NUMBER, 1);
        write_uint_element(&mut track_entry, TRACK_TYPE, TRACK_TYPE_AUDIO);
        write_string_element(&mut track_entry, CODEC_ID, codec_id);

        let mut audio = Vec::new();
        write_float_element(&mut audio, SAMPLING_FREQ, sample_rate);
        write_uint_element(&mut audio, CHANNELS, channels);
        write_master_element(&mut track_entry, AUDIO, &audio);

        write_master_element(&mut tracks, TRACK_ENTRY, &track_entry);
        write_master_element(&mut segment, TRACKS, &tracks);

        // Cluster with one SimpleBlock
        let mut cluster = Vec::new();
        write_uint_element(&mut cluster, TIMECODE, 0);

        // SimpleBlock: track=1, timecode=0, flags=keyframe, data
        let mut block = Vec::new();
        write_vint(&mut block, 1); // track number
        block.extend_from_slice(&0i16.to_be_bytes()); // relative timecode
        block.push(0x80); // flags: keyframe
        block.extend_from_slice(&[0x42u8; 64]); // data

        write_id(&mut cluster, SIMPLE_BLOCK);
        write_vint(&mut cluster, block.len() as u64);
        cluster.extend_from_slice(&block);

        write_master_element(&mut segment, CLUSTER, &cluster);

        write_master_element(&mut file, SEGMENT, &segment);
        file
    }

    /// Build a minimal MKV file with one video track.
    fn make_mkv_video(codec_id: &str, width: u64, height: u64) -> Vec<u8> {
        let mut file = Vec::new();

        let mut header = Vec::new();
        write_string_element(&mut header, DOC_TYPE, "matroska");
        write_master_element(&mut file, EBML_HEADER, &header);

        let mut segment = Vec::new();

        let mut info = Vec::new();
        write_uint_element(&mut info, TIMECODE_SCALE, 1_000_000);
        write_float_element(&mut info, DURATION, 5000.0);
        write_master_element(&mut segment, INFO, &info);

        let mut tracks = Vec::new();
        let mut track_entry = Vec::new();
        write_uint_element(&mut track_entry, TRACK_NUMBER, 1);
        write_uint_element(&mut track_entry, TRACK_TYPE, TRACK_TYPE_VIDEO);
        write_string_element(&mut track_entry, CODEC_ID, codec_id);

        let mut video = Vec::new();
        write_uint_element(&mut video, PIXEL_WIDTH, width);
        write_uint_element(&mut video, PIXEL_HEIGHT, height);
        write_master_element(&mut track_entry, VIDEO, &video);

        write_master_element(&mut tracks, TRACK_ENTRY, &track_entry);
        write_master_element(&mut segment, TRACKS, &tracks);

        let mut cluster = Vec::new();
        write_uint_element(&mut cluster, TIMECODE, 0);

        let mut block = Vec::new();
        write_vint(&mut block, 1);
        block.extend_from_slice(&0i16.to_be_bytes());
        block.push(0x80);
        block.extend_from_slice(&[0xABu8; 128]);

        write_id(&mut cluster, SIMPLE_BLOCK);
        write_vint(&mut cluster, block.len() as u64);
        cluster.extend_from_slice(&block);

        write_master_element(&mut segment, CLUSTER, &cluster);
        write_master_element(&mut file, SEGMENT, &segment);
        file
    }

    #[test]
    fn mkv_opus_probe() {
        let mkv = make_mkv_audio("A_OPUS", 48000.0, 2);
        let cursor = Cursor::new(mkv);
        let mut demuxer = MkvDemuxer::new(cursor);
        let info = demuxer.probe().unwrap();

        assert_eq!(info.format, ContainerFormat::Mkv);
        assert!(info.has_audio());
        let audio = info.audio_streams().collect::<Vec<_>>();
        assert_eq!(audio[0].codec, AudioCodec::Opus);
        assert_eq!(audio[0].sample_rate, 48000);
        assert_eq!(audio[0].channels, 2);
    }

    #[test]
    fn mkv_vorbis_probe() {
        let mkv = make_mkv_audio("A_VORBIS", 44100.0, 2);
        let cursor = Cursor::new(mkv);
        let mut demuxer = MkvDemuxer::new(cursor);
        let info = demuxer.probe().unwrap();

        let audio = info.audio_streams().collect::<Vec<_>>();
        assert_eq!(audio[0].codec, AudioCodec::Vorbis);
        assert_eq!(audio[0].sample_rate, 44100);
    }

    #[test]
    fn mkv_flac_probe() {
        let mkv = make_mkv_audio("A_FLAC", 96000.0, 2);
        let cursor = Cursor::new(mkv);
        let mut demuxer = MkvDemuxer::new(cursor);
        let info = demuxer.probe().unwrap();

        let audio = info.audio_streams().collect::<Vec<_>>();
        assert_eq!(audio[0].codec, AudioCodec::Flac);
        assert_eq!(audio[0].sample_rate, 96000);
    }

    #[test]
    fn mkv_av1_video_probe() {
        let mkv = make_mkv_video("V_AV1", 1920, 1080);
        let cursor = Cursor::new(mkv);
        let mut demuxer = MkvDemuxer::new(cursor);
        let info = demuxer.probe().unwrap();

        assert!(info.has_video());
        let video = info.video_streams().collect::<Vec<_>>();
        assert_eq!(video[0].codec, VideoCodec::Av1);
        assert_eq!(video[0].width, 1920);
        assert_eq!(video[0].height, 1080);
    }

    #[test]
    fn mkv_vp9_video_probe() {
        let mkv = make_mkv_video("V_VP9", 3840, 2160);
        let cursor = Cursor::new(mkv);
        let mut demuxer = MkvDemuxer::new(cursor);
        let info = demuxer.probe().unwrap();

        let video = info.video_streams().collect::<Vec<_>>();
        assert_eq!(video[0].codec, VideoCodec::Vp9);
        assert_eq!(video[0].width, 3840);
        assert_eq!(video[0].height, 2160);
    }

    #[test]
    fn mkv_duration() {
        let mkv = make_mkv_audio("A_OPUS", 48000.0, 2);
        let cursor = Cursor::new(mkv);
        let mut demuxer = MkvDemuxer::new(cursor);
        let info = demuxer.probe().unwrap();

        let duration = info.duration.unwrap();
        assert!((duration.as_secs_f64() - 1.0).abs() < 0.01);
    }

    #[test]
    fn mkv_read_packet() {
        let mkv = make_mkv_audio("A_OPUS", 48000.0, 2);
        let cursor = Cursor::new(mkv);
        let mut demuxer = MkvDemuxer::new(cursor);
        demuxer.probe().unwrap();

        let packet = demuxer.next_packet().unwrap();
        assert_eq!(packet.stream_index, 0);
        assert!(packet.is_keyframe);
        assert_eq!(packet.data.len(), 64);
    }

    #[test]
    fn mkv_invalid_header() {
        let cursor = Cursor::new(vec![0u8; 100]);
        let mut demuxer = MkvDemuxer::new(cursor);
        assert!(demuxer.probe().is_err());
    }

    #[test]
    fn test_mkv_string_too_large() {
        // Build an MKV where a string element declares size > 65536
        let mut file = Vec::new();

        // EBML Header with an oversized DocType string
        let mut header = Vec::new();
        // Write DocType element with size = 65537 (exceeds MAX_STRING_SIZE)
        write_id(&mut header, DOC_TYPE);
        // Encode size 65537 as a 4-byte VINT
        let big_size: u64 = 65537;
        write_vint(&mut header, big_size);
        // Write enough junk bytes to fill it (we don't need all, just enough for the reader)
        header.extend_from_slice(&vec![0x41u8; 256]);
        write_master_element(&mut file, EBML_HEADER, &header);

        let cursor = Cursor::new(file);
        let mut demuxer = MkvDemuxer::new(cursor);
        let result = demuxer.probe();
        assert!(result.is_err(), "should reject string > 65536 bytes");
        let err_msg = format!("{}", result.unwrap_err());
        assert!(
            err_msg.contains("exceeds maximum"),
            "error should mention exceeds maximum, got: {err_msg}"
        );
    }

    #[test]
    fn test_mkv_too_many_tracks() {
        // Build an MKV with 129 tracks, exceeding MAX_TRACKS (128)
        let mut file = Vec::new();

        let mut header = Vec::new();
        write_string_element(&mut header, DOC_TYPE, "matroska");
        write_master_element(&mut file, EBML_HEADER, &header);

        let mut segment = Vec::new();

        // Info
        let mut info = Vec::new();
        write_uint_element(&mut info, TIMECODE_SCALE, 1_000_000);
        write_float_element(&mut info, DURATION, 1000.0);
        write_master_element(&mut segment, INFO, &info);

        // Tracks: 129 track entries, each audio
        let mut tracks = Vec::new();
        for i in 1..=129u64 {
            let mut entry = Vec::new();
            write_uint_element(&mut entry, TRACK_NUMBER, i);
            write_uint_element(&mut entry, TRACK_TYPE, TRACK_TYPE_AUDIO);
            write_string_element(&mut entry, CODEC_ID, "A_OPUS");

            let mut audio = Vec::new();
            write_float_element(&mut audio, SAMPLING_FREQ, 48000.0);
            write_uint_element(&mut audio, CHANNELS, 2);
            write_master_element(&mut entry, AUDIO, &audio);

            write_master_element(&mut tracks, TRACK_ENTRY, &entry);
        }
        write_master_element(&mut segment, TRACKS, &tracks);

        write_master_element(&mut file, SEGMENT, &segment);

        let cursor = Cursor::new(file);
        let mut demuxer = MkvDemuxer::new(cursor);
        let result = demuxer.probe();
        assert!(result.is_err(), "should reject > 128 tracks");
        let err_msg = format!("{}", result.unwrap_err());
        assert!(
            err_msg.contains("too many tracks"),
            "error should mention too many tracks, got: {err_msg}"
        );
    }

    #[test]
    fn test_mkv_seek_invalid_offset() {
        let mkv = make_mkv_audio("A_OPUS", 48000.0, 2);
        let cursor = Cursor::new(mkv);
        let mut demuxer = MkvDemuxer::new(cursor);
        demuxer.probe().unwrap();

        // Set cluster_offset to something beyond u64::MAX/2
        demuxer.cluster_offset = u64::MAX / 2 + 1;
        let result = demuxer.seek(Duration::from_secs(1));
        assert!(result.is_err(), "should reject cluster_offset > u64::MAX/2");
        let err_msg = format!("{}", result.unwrap_err());
        assert!(
            err_msg.contains("exceeds reasonable bound"),
            "error should mention reasonable bound, got: {err_msg}"
        );
    }

    #[test]
    fn test_mkv_vint_parsing() {
        // Test 1-byte VINT: value 5 encoded as 0x85
        {
            let data = vec![0x85u8];
            let cursor = Cursor::new(data);
            let mut demuxer = MkvDemuxer::new(cursor);
            let (val, len) = demuxer.read_vint().unwrap();
            assert_eq!(val, 5);
            assert_eq!(len, 1);
        }

        // Test 2-byte VINT: value 0x80 (128) encoded as 0x40, 0x80
        {
            let data = vec![0x40u8, 0x80];
            let cursor = Cursor::new(data);
            let mut demuxer = MkvDemuxer::new(cursor);
            let (val, len) = demuxer.read_vint().unwrap();
            assert_eq!(val, 128);
            assert_eq!(len, 2);
        }

        // Test 4-byte VINT: value 0x20_0000 encoded with 4-byte marker
        {
            let mut buf = Vec::new();
            ebml::write_vint(&mut buf, 0x20_0000);
            assert_eq!(buf.len(), 4);
            let cursor = Cursor::new(buf);
            let mut demuxer = MkvDemuxer::new(cursor);
            let (val, len) = demuxer.read_vint().unwrap();
            assert_eq!(val, 0x20_0000);
            assert_eq!(len, 4);
        }

        // Test 1-byte VINT: value 0 encoded as 0x80
        {
            let data = vec![0x80u8];
            let cursor = Cursor::new(data);
            let mut demuxer = MkvDemuxer::new(cursor);
            let (val, len) = demuxer.read_vint().unwrap();
            assert_eq!(val, 0);
            assert_eq!(len, 1);
        }

        // Test invalid VINT: zero byte
        {
            let data = vec![0x00u8];
            let cursor = Cursor::new(data);
            let mut demuxer = MkvDemuxer::new(cursor);
            let result = demuxer.read_vint();
            assert!(result.is_err(), "zero byte should be invalid VINT");
        }
    }

    #[test]
    fn test_mkv_empty_cluster() {
        // Build an MKV where the cluster has a Timecode but no SimpleBlocks
        let mut file = Vec::new();

        let mut header = Vec::new();
        write_string_element(&mut header, DOC_TYPE, "matroska");
        write_master_element(&mut file, EBML_HEADER, &header);

        let mut segment = Vec::new();

        let mut info = Vec::new();
        write_uint_element(&mut info, TIMECODE_SCALE, 1_000_000);
        write_float_element(&mut info, DURATION, 1000.0);
        write_master_element(&mut segment, INFO, &info);

        let mut tracks = Vec::new();
        let mut track_entry = Vec::new();
        write_uint_element(&mut track_entry, TRACK_NUMBER, 1);
        write_uint_element(&mut track_entry, TRACK_TYPE, TRACK_TYPE_AUDIO);
        write_string_element(&mut track_entry, CODEC_ID, "A_OPUS");

        let mut audio = Vec::new();
        write_float_element(&mut audio, SAMPLING_FREQ, 48000.0);
        write_uint_element(&mut audio, CHANNELS, 2);
        write_master_element(&mut track_entry, AUDIO, &audio);

        write_master_element(&mut tracks, TRACK_ENTRY, &track_entry);
        write_master_element(&mut segment, TRACKS, &tracks);

        // Cluster with only a Timecode, no SimpleBlocks
        let mut cluster = Vec::new();
        write_uint_element(&mut cluster, TIMECODE, 0);
        write_master_element(&mut segment, CLUSTER, &cluster);

        write_master_element(&mut file, SEGMENT, &segment);

        let cursor = Cursor::new(file);
        let mut demuxer = MkvDemuxer::new(cursor);
        demuxer.probe().unwrap();

        // next_packet should return EndOfStream since there are no SimpleBlocks
        match demuxer.next_packet() {
            Err(TarangError::EndOfStream) => {} // expected
            other => panic!("expected EndOfStream, got {:?}", other),
        }
    }

    // ---- Additional coverage tests ----

    #[test]
    fn mkv_aac_audio_probe() {
        let mkv = make_mkv_audio("A_AAC", 44100.0, 2);
        let cursor = Cursor::new(mkv);
        let mut demuxer = MkvDemuxer::new(cursor);
        let info = demuxer.probe().unwrap();

        let audio = info.audio_streams().collect::<Vec<_>>();
        assert_eq!(audio[0].codec, AudioCodec::Aac);
        assert_eq!(audio[0].sample_rate, 44100);
    }

    #[test]
    fn mkv_aac_mpeg4_lc_audio_probe() {
        let mkv = make_mkv_audio("A_AAC/MPEG4/LC", 48000.0, 2);
        let cursor = Cursor::new(mkv);
        let mut demuxer = MkvDemuxer::new(cursor);
        let info = demuxer.probe().unwrap();

        let audio = info.audio_streams().collect::<Vec<_>>();
        assert_eq!(audio[0].codec, AudioCodec::Aac);
    }

    #[test]
    fn mkv_mp3_audio_probe() {
        let mkv = make_mkv_audio("A_MPEG/L3", 44100.0, 2);
        let cursor = Cursor::new(mkv);
        let mut demuxer = MkvDemuxer::new(cursor);
        let info = demuxer.probe().unwrap();

        let audio = info.audio_streams().collect::<Vec<_>>();
        assert_eq!(audio[0].codec, AudioCodec::Mp3);
    }

    #[test]
    fn mkv_pcm_audio_probe() {
        let mkv = make_mkv_audio("A_PCM/INT/LIT", 48000.0, 1);
        let cursor = Cursor::new(mkv);
        let mut demuxer = MkvDemuxer::new(cursor);
        let info = demuxer.probe().unwrap();

        let audio = info.audio_streams().collect::<Vec<_>>();
        assert_eq!(audio[0].codec, AudioCodec::Pcm);
        assert_eq!(audio[0].channels, 1);
    }

    #[test]
    fn mkv_pcm_float_audio_probe() {
        let mkv = make_mkv_audio("A_PCM/FLOAT/IEEE", 96000.0, 2);
        let cursor = Cursor::new(mkv);
        let mut demuxer = MkvDemuxer::new(cursor);
        let info = demuxer.probe().unwrap();

        let audio = info.audio_streams().collect::<Vec<_>>();
        assert_eq!(audio[0].codec, AudioCodec::Pcm);
        assert_eq!(audio[0].sample_rate, 96000);
    }

    #[test]
    fn mkv_alac_audio_probe() {
        let mkv = make_mkv_audio("A_ALAC", 44100.0, 2);
        let cursor = Cursor::new(mkv);
        let mut demuxer = MkvDemuxer::new(cursor);
        let info = demuxer.probe().unwrap();

        let audio = info.audio_streams().collect::<Vec<_>>();
        assert_eq!(audio[0].codec, AudioCodec::Alac);
    }

    #[test]
    fn mkv_unsupported_audio_codec() {
        // A_DTS is not in the codec map, so the file should have no supported streams
        let mkv = make_mkv_audio("A_DTS", 48000.0, 6);
        let cursor = Cursor::new(mkv);
        let mut demuxer = MkvDemuxer::new(cursor);
        let result = demuxer.probe();
        assert!(result.is_err(), "unsupported codec should produce error");
    }

    #[test]
    fn mkv_h264_video_probe() {
        let mkv = make_mkv_video("V_MPEG4/ISO/AVC", 1920, 1080);
        let cursor = Cursor::new(mkv);
        let mut demuxer = MkvDemuxer::new(cursor);
        let info = demuxer.probe().unwrap();

        let video = info.video_streams().collect::<Vec<_>>();
        assert_eq!(video[0].codec, VideoCodec::H264);
        assert_eq!(video[0].width, 1920);
        assert_eq!(video[0].height, 1080);
    }

    #[test]
    fn mkv_h265_video_probe() {
        let mkv = make_mkv_video("V_MPEGH/ISO/HEVC", 3840, 2160);
        let cursor = Cursor::new(mkv);
        let mut demuxer = MkvDemuxer::new(cursor);
        let info = demuxer.probe().unwrap();

        let video = info.video_streams().collect::<Vec<_>>();
        assert_eq!(video[0].codec, VideoCodec::H265);
    }

    #[test]
    fn mkv_vp8_video_probe() {
        let mkv = make_mkv_video("V_VP8", 640, 480);
        let cursor = Cursor::new(mkv);
        let mut demuxer = MkvDemuxer::new(cursor);
        let info = demuxer.probe().unwrap();

        let video = info.video_streams().collect::<Vec<_>>();
        assert_eq!(video[0].codec, VideoCodec::Vp8);
        assert_eq!(video[0].width, 640);
        assert_eq!(video[0].height, 480);
    }

    #[test]
    fn mkv_theora_video_probe() {
        let mkv = make_mkv_video("V_THEORA", 720, 576);
        let cursor = Cursor::new(mkv);
        let mut demuxer = MkvDemuxer::new(cursor);
        let info = demuxer.probe().unwrap();

        let video = info.video_streams().collect::<Vec<_>>();
        assert_eq!(video[0].codec, VideoCodec::Theora);
    }

    #[test]
    fn mkv_unsupported_video_codec() {
        let mkv = make_mkv_video("V_DIRAC", 1280, 720);
        let cursor = Cursor::new(mkv);
        let mut demuxer = MkvDemuxer::new(cursor);
        let result = demuxer.probe();
        assert!(
            result.is_err(),
            "unsupported video codec should produce error"
        );
    }

    /// Build an MKV file with both an audio and a video track.
    fn make_mkv_audio_video(
        audio_codec: &str,
        video_codec: &str,
        sample_rate: f64,
        width: u64,
        height: u64,
    ) -> Vec<u8> {
        let mut file = Vec::new();

        let mut header = Vec::new();
        write_string_element(&mut header, DOC_TYPE, "matroska");
        write_master_element(&mut file, EBML_HEADER, &header);

        let mut segment = Vec::new();

        let mut info = Vec::new();
        write_uint_element(&mut info, TIMECODE_SCALE, 1_000_000);
        write_float_element(&mut info, DURATION, 5000.0);
        write_master_element(&mut segment, INFO, &info);

        let mut tracks = Vec::new();

        // Audio track (track 1)
        let mut audio_entry = Vec::new();
        write_uint_element(&mut audio_entry, TRACK_NUMBER, 1);
        write_uint_element(&mut audio_entry, TRACK_TYPE, TRACK_TYPE_AUDIO);
        write_string_element(&mut audio_entry, CODEC_ID, audio_codec);
        let mut audio = Vec::new();
        write_float_element(&mut audio, SAMPLING_FREQ, sample_rate);
        write_uint_element(&mut audio, CHANNELS, 2);
        write_uint_element(&mut audio, BIT_DEPTH, 16);
        write_master_element(&mut audio_entry, AUDIO, &audio);
        write_master_element(&mut tracks, TRACK_ENTRY, &audio_entry);

        // Video track (track 2)
        let mut video_entry = Vec::new();
        write_uint_element(&mut video_entry, TRACK_NUMBER, 2);
        write_uint_element(&mut video_entry, TRACK_TYPE, TRACK_TYPE_VIDEO);
        write_string_element(&mut video_entry, CODEC_ID, video_codec);
        let mut video = Vec::new();
        write_uint_element(&mut video, PIXEL_WIDTH, width);
        write_uint_element(&mut video, PIXEL_HEIGHT, height);
        write_master_element(&mut video_entry, VIDEO, &video);
        write_master_element(&mut tracks, TRACK_ENTRY, &video_entry);

        write_master_element(&mut segment, TRACKS, &tracks);

        // Cluster with SimpleBlocks for both tracks
        let mut cluster = Vec::new();
        write_uint_element(&mut cluster, TIMECODE, 0);

        // Audio block (track 1)
        let mut block1 = Vec::new();
        write_vint(&mut block1, 1);
        block1.extend_from_slice(&0i16.to_be_bytes());
        block1.push(0x80);
        block1.extend_from_slice(&[0xAA; 64]);
        write_id(&mut cluster, SIMPLE_BLOCK);
        write_vint(&mut cluster, block1.len() as u64);
        cluster.extend_from_slice(&block1);

        // Video block (track 2)
        let mut block2 = Vec::new();
        write_vint(&mut block2, 2);
        block2.extend_from_slice(&0i16.to_be_bytes());
        block2.push(0x80);
        block2.extend_from_slice(&[0xBB; 128]);
        write_id(&mut cluster, SIMPLE_BLOCK);
        write_vint(&mut cluster, block2.len() as u64);
        cluster.extend_from_slice(&block2);

        write_master_element(&mut segment, CLUSTER, &cluster);
        write_master_element(&mut file, SEGMENT, &segment);
        file
    }

    #[test]
    fn mkv_audio_video_probe() {
        let mkv = make_mkv_audio_video("A_OPUS", "V_AV1", 48000.0, 1920, 1080);
        let cursor = Cursor::new(mkv);
        let mut demuxer = MkvDemuxer::new(cursor);
        let info = demuxer.probe().unwrap();

        assert!(info.has_audio());
        assert!(info.has_video());

        let audio = info.audio_streams().collect::<Vec<_>>();
        assert_eq!(audio[0].codec, AudioCodec::Opus);

        let video = info.video_streams().collect::<Vec<_>>();
        assert_eq!(video[0].codec, VideoCodec::Av1);
        assert_eq!(video[0].width, 1920);
        assert_eq!(video[0].height, 1080);
    }

    #[test]
    fn mkv_audio_video_read_packets() {
        let mkv = make_mkv_audio_video("A_OPUS", "V_VP9", 48000.0, 1280, 720);
        let cursor = Cursor::new(mkv);
        let mut demuxer = MkvDemuxer::new(cursor);
        demuxer.probe().unwrap();

        // Read two packets (one audio, one video)
        let pkt1 = demuxer.next_packet().unwrap();
        let pkt2 = demuxer.next_packet().unwrap();

        // Check that we got packets from different stream indices
        let indices: Vec<usize> = vec![pkt1.stream_index, pkt2.stream_index];
        assert!(indices.contains(&0));
        assert!(indices.contains(&1));
    }

    /// Build an MKV with multiple clusters for seek testing.
    fn make_mkv_multi_cluster(num_clusters: usize, packets_per_cluster: usize) -> Vec<u8> {
        let mut file = Vec::new();

        let mut header = Vec::new();
        write_string_element(&mut header, DOC_TYPE, "matroska");
        write_master_element(&mut file, EBML_HEADER, &header);

        let mut segment = Vec::new();

        let mut info = Vec::new();
        write_uint_element(&mut info, TIMECODE_SCALE, 1_000_000); // 1ms
        let total_ms = (num_clusters * 1000) as f64;
        write_float_element(&mut info, DURATION, total_ms);
        write_master_element(&mut segment, INFO, &info);

        let mut tracks = Vec::new();
        let mut track_entry = Vec::new();
        write_uint_element(&mut track_entry, TRACK_NUMBER, 1);
        write_uint_element(&mut track_entry, TRACK_TYPE, TRACK_TYPE_AUDIO);
        write_string_element(&mut track_entry, CODEC_ID, "A_OPUS");
        let mut audio = Vec::new();
        write_float_element(&mut audio, SAMPLING_FREQ, 48000.0);
        write_uint_element(&mut audio, CHANNELS, 2);
        write_master_element(&mut track_entry, AUDIO, &audio);
        write_master_element(&mut tracks, TRACK_ENTRY, &track_entry);
        write_master_element(&mut segment, TRACKS, &tracks);

        // Multiple clusters
        for c in 0..num_clusters {
            let cluster_tc = (c * 1000) as u64; // each cluster = 1 second

            let mut cluster = Vec::new();
            write_uint_element(&mut cluster, TIMECODE, cluster_tc);

            for p in 0..packets_per_cluster {
                let relative_tc = (p * 20) as i16; // 20ms between packets
                let mut block = Vec::new();
                write_vint(&mut block, 1);
                block.extend_from_slice(&relative_tc.to_be_bytes());
                block.push(0x80); // keyframe
                block.extend_from_slice(&[0x42u8; 64]);

                write_id(&mut cluster, SIMPLE_BLOCK);
                write_vint(&mut cluster, block.len() as u64);
                cluster.extend_from_slice(&block);
            }

            write_master_element(&mut segment, CLUSTER, &cluster);
        }

        write_master_element(&mut file, SEGMENT, &segment);
        file
    }

    #[test]
    fn mkv_seek_multi_cluster() {
        let mkv = make_mkv_multi_cluster(5, 10); // 5 clusters, 10 packets each
        let cursor = Cursor::new(mkv);
        let mut demuxer = MkvDemuxer::new(cursor);
        demuxer.probe().unwrap();

        // Seek to 2.5 seconds — should land in cluster 2 or 3
        demuxer.seek(Duration::from_millis(2500)).unwrap();

        let pkt = demuxer.next_packet().unwrap();
        let ts = pkt.timestamp.as_secs_f64();
        assert!(
            ts >= 1.5,
            "after seeking to 2.5s, got packet at {ts}s which is too early"
        );
    }

    #[test]
    fn mkv_seek_to_start() {
        let mkv = make_mkv_multi_cluster(3, 5);
        let cursor = Cursor::new(mkv);
        let mut demuxer = MkvDemuxer::new(cursor);
        demuxer.probe().unwrap();

        // Read some packets first
        let _ = demuxer.next_packet().unwrap();
        let _ = demuxer.next_packet().unwrap();

        // Seek to a very early timestamp — the first cluster has tc=0,
        // so any target > 0 will pass cluster 0 before stopping.
        demuxer.seek(Duration::from_millis(500)).unwrap();
        let pkt = demuxer.next_packet().unwrap();
        assert!(
            pkt.timestamp.as_secs_f64() < 1.5,
            "seek to 0.5s should return packet before 1.5s, got {:.4}s",
            pkt.timestamp.as_secs_f64()
        );
    }

    #[test]
    fn mkv_seek_past_end() {
        let mkv = make_mkv_multi_cluster(2, 5);
        let cursor = Cursor::new(mkv);
        let mut demuxer = MkvDemuxer::new(cursor);
        demuxer.probe().unwrap();

        // Seek past the end of the file — should not panic
        let result = demuxer.seek(Duration::from_secs(100));
        assert!(result.is_ok(), "seeking past end should not error");
    }

    #[test]
    fn mkv_simple_block_truncated() {
        // Build an MKV where the SimpleBlock claims a size larger than remaining data
        let mut file = Vec::new();

        let mut header = Vec::new();
        write_string_element(&mut header, DOC_TYPE, "matroska");
        write_master_element(&mut file, EBML_HEADER, &header);

        let mut segment = Vec::new();

        let mut info = Vec::new();
        write_uint_element(&mut info, TIMECODE_SCALE, 1_000_000);
        write_float_element(&mut info, DURATION, 1000.0);
        write_master_element(&mut segment, INFO, &info);

        let mut tracks = Vec::new();
        let mut track_entry = Vec::new();
        write_uint_element(&mut track_entry, TRACK_NUMBER, 1);
        write_uint_element(&mut track_entry, TRACK_TYPE, TRACK_TYPE_AUDIO);
        write_string_element(&mut track_entry, CODEC_ID, "A_OPUS");
        let mut audio = Vec::new();
        write_float_element(&mut audio, SAMPLING_FREQ, 48000.0);
        write_uint_element(&mut audio, CHANNELS, 2);
        write_master_element(&mut track_entry, AUDIO, &audio);
        write_master_element(&mut tracks, TRACK_ENTRY, &track_entry);
        write_master_element(&mut segment, TRACKS, &tracks);

        // Cluster with a SimpleBlock that has size < header
        let mut cluster = Vec::new();
        write_uint_element(&mut cluster, TIMECODE, 0);

        // SimpleBlock with size=2 but that's less than header (vint+2+1 = 4 minimum)
        write_id(&mut cluster, SIMPLE_BLOCK);
        write_vint(&mut cluster, 2); // size = 2 bytes (too small for header)
        cluster.extend_from_slice(&[0x81, 0x00]); // track=1, partial timecode

        write_master_element(&mut segment, CLUSTER, &cluster);
        write_master_element(&mut file, SEGMENT, &segment);

        let cursor = Cursor::new(file);
        let mut demuxer = MkvDemuxer::new(cursor);
        demuxer.probe().unwrap();

        // Reading the packet should fail due to truncated block
        let result = demuxer.next_packet();
        assert!(
            result.is_err(),
            "truncated SimpleBlock should produce error"
        );
    }

    #[test]
    fn mkv_webm_detection() {
        let mut file = Vec::new();

        let mut header = Vec::new();
        write_string_element(&mut header, DOC_TYPE, "webm");
        write_master_element(&mut file, EBML_HEADER, &header);

        let mut segment = Vec::new();
        let mut info = Vec::new();
        write_uint_element(&mut info, TIMECODE_SCALE, 1_000_000);
        write_float_element(&mut info, DURATION, 1000.0);
        write_master_element(&mut segment, INFO, &info);

        let mut tracks = Vec::new();
        let mut track_entry = Vec::new();
        write_uint_element(&mut track_entry, TRACK_NUMBER, 1);
        write_uint_element(&mut track_entry, TRACK_TYPE, TRACK_TYPE_AUDIO);
        write_string_element(&mut track_entry, CODEC_ID, "A_OPUS");
        let mut audio = Vec::new();
        write_float_element(&mut audio, SAMPLING_FREQ, 48000.0);
        write_uint_element(&mut audio, CHANNELS, 2);
        write_master_element(&mut track_entry, AUDIO, &audio);
        write_master_element(&mut tracks, TRACK_ENTRY, &track_entry);
        write_master_element(&mut segment, TRACKS, &tracks);

        let mut cluster = Vec::new();
        write_uint_element(&mut cluster, TIMECODE, 0);
        let mut block = Vec::new();
        write_vint(&mut block, 1);
        block.extend_from_slice(&0i16.to_be_bytes());
        block.push(0x80);
        block.extend_from_slice(&[0x42u8; 32]);
        write_id(&mut cluster, SIMPLE_BLOCK);
        write_vint(&mut cluster, block.len() as u64);
        cluster.extend_from_slice(&block);
        write_master_element(&mut segment, CLUSTER, &cluster);

        write_master_element(&mut file, SEGMENT, &segment);

        let cursor = Cursor::new(file);
        let mut demuxer = MkvDemuxer::new(cursor);
        let info = demuxer.probe().unwrap();

        assert_eq!(info.format, ContainerFormat::WebM);
    }

    #[test]
    fn mkv_end_of_stream_after_all_packets() {
        let mkv = make_mkv_audio("A_OPUS", 48000.0, 2);
        let cursor = Cursor::new(mkv);
        let mut demuxer = MkvDemuxer::new(cursor);
        demuxer.probe().unwrap();

        // Read the single packet
        demuxer.next_packet().unwrap();

        // Next read should be EndOfStream
        match demuxer.next_packet() {
            Err(TarangError::EndOfStream) => {}
            other => panic!("expected EndOfStream, got {other:?}"),
        }
    }

    #[test]
    fn mkv_no_tracks_element() {
        // Build MKV with only EBML header + Segment containing Info but no Tracks
        let mut file = Vec::new();

        let mut header = Vec::new();
        write_string_element(&mut header, DOC_TYPE, "matroska");
        write_master_element(&mut file, EBML_HEADER, &header);

        let mut segment = Vec::new();
        let mut info = Vec::new();
        write_uint_element(&mut info, TIMECODE_SCALE, 1_000_000);
        write_master_element(&mut segment, INFO, &info);
        // No Tracks element!
        write_master_element(&mut file, SEGMENT, &segment);

        let cursor = Cursor::new(file);
        let mut demuxer = MkvDemuxer::new(cursor);
        let result = demuxer.probe();
        assert!(result.is_err());
    }

    #[test]
    fn mkv_missing_segment() {
        // Build MKV with EBML header but no Segment
        let mut file = Vec::new();
        let mut header = Vec::new();
        write_string_element(&mut header, DOC_TYPE, "matroska");
        write_master_element(&mut file, EBML_HEADER, &header);
        // Write something that isn't SEGMENT
        let dummy = vec![0x42u8; 8];
        write_master_element(&mut file, INFO, &dummy);

        let cursor = Cursor::new(file);
        let mut demuxer = MkvDemuxer::new(cursor);
        let result = demuxer.probe();
        assert!(result.is_err(), "missing Segment should produce error");
    }

    #[test]
    fn mkv_non_keyframe_packet() {
        // Build an MKV with a non-keyframe SimpleBlock
        let mut file = Vec::new();

        let mut header = Vec::new();
        write_string_element(&mut header, DOC_TYPE, "matroska");
        write_master_element(&mut file, EBML_HEADER, &header);

        let mut segment = Vec::new();

        let mut info = Vec::new();
        write_uint_element(&mut info, TIMECODE_SCALE, 1_000_000);
        write_float_element(&mut info, DURATION, 1000.0);
        write_master_element(&mut segment, INFO, &info);

        let mut tracks = Vec::new();
        let mut track_entry = Vec::new();
        write_uint_element(&mut track_entry, TRACK_NUMBER, 1);
        write_uint_element(&mut track_entry, TRACK_TYPE, TRACK_TYPE_AUDIO);
        write_string_element(&mut track_entry, CODEC_ID, "A_OPUS");
        let mut audio = Vec::new();
        write_float_element(&mut audio, SAMPLING_FREQ, 48000.0);
        write_uint_element(&mut audio, CHANNELS, 2);
        write_master_element(&mut track_entry, AUDIO, &audio);
        write_master_element(&mut tracks, TRACK_ENTRY, &track_entry);
        write_master_element(&mut segment, TRACKS, &tracks);

        let mut cluster = Vec::new();
        write_uint_element(&mut cluster, TIMECODE, 0);

        // SimpleBlock with flags=0x00 (not keyframe)
        let mut block = Vec::new();
        write_vint(&mut block, 1);
        block.extend_from_slice(&10i16.to_be_bytes()); // relative tc = 10
        block.push(0x00); // flags: not keyframe
        block.extend_from_slice(&[0x42u8; 32]);

        write_id(&mut cluster, SIMPLE_BLOCK);
        write_vint(&mut cluster, block.len() as u64);
        cluster.extend_from_slice(&block);

        write_master_element(&mut segment, CLUSTER, &cluster);
        write_master_element(&mut file, SEGMENT, &segment);

        let cursor = Cursor::new(file);
        let mut demuxer = MkvDemuxer::new(cursor);
        demuxer.probe().unwrap();

        let pkt = demuxer.next_packet().unwrap();
        assert!(!pkt.is_keyframe, "packet should not be a keyframe");
        // Timestamp: cluster_tc=0 + relative=10, in ms = 10ms
        assert!(
            (pkt.timestamp.as_secs_f64() - 0.010).abs() < 0.002,
            "expected ~10ms timestamp, got {:.4}s",
            pkt.timestamp.as_secs_f64()
        );
    }

    #[test]
    fn mkv_audio_with_bit_depth() {
        // Build MKV with audio track that has bit_depth set
        let mut file = Vec::new();

        let mut header = Vec::new();
        write_string_element(&mut header, DOC_TYPE, "matroska");
        write_master_element(&mut file, EBML_HEADER, &header);

        let mut segment = Vec::new();

        let mut info = Vec::new();
        write_uint_element(&mut info, TIMECODE_SCALE, 1_000_000);
        write_float_element(&mut info, DURATION, 1000.0);
        write_master_element(&mut segment, INFO, &info);

        let mut tracks = Vec::new();
        let mut track_entry = Vec::new();
        write_uint_element(&mut track_entry, TRACK_NUMBER, 1);
        write_uint_element(&mut track_entry, TRACK_TYPE, TRACK_TYPE_AUDIO);
        write_string_element(&mut track_entry, CODEC_ID, "A_FLAC");
        let mut audio = Vec::new();
        write_float_element(&mut audio, SAMPLING_FREQ, 96000.0);
        write_uint_element(&mut audio, CHANNELS, 2);
        write_uint_element(&mut audio, BIT_DEPTH, 24);
        write_master_element(&mut track_entry, AUDIO, &audio);
        write_master_element(&mut tracks, TRACK_ENTRY, &track_entry);
        write_master_element(&mut segment, TRACKS, &tracks);

        let mut cluster = Vec::new();
        write_uint_element(&mut cluster, TIMECODE, 0);
        let mut block = Vec::new();
        write_vint(&mut block, 1);
        block.extend_from_slice(&0i16.to_be_bytes());
        block.push(0x80);
        block.extend_from_slice(&[0x42u8; 32]);
        write_id(&mut cluster, SIMPLE_BLOCK);
        write_vint(&mut cluster, block.len() as u64);
        cluster.extend_from_slice(&block);
        write_master_element(&mut segment, CLUSTER, &cluster);

        write_master_element(&mut file, SEGMENT, &segment);

        let cursor = Cursor::new(file);
        let mut demuxer = MkvDemuxer::new(cursor);
        let info = demuxer.probe().unwrap();

        let audio_streams = info.audio_streams().collect::<Vec<_>>();
        assert_eq!(audio_streams[0].codec, AudioCodec::Flac);
        assert_eq!(audio_streams[0].sample_rate, 96000);
    }

    #[test]
    fn mkv_seek_cluster_timecode_not_first_child() {
        // Build an MKV where the cluster's first child is NOT a Timecode element
        // to exercise the else branch in seek()
        let mut file = Vec::new();

        let mut header = Vec::new();
        write_string_element(&mut header, DOC_TYPE, "matroska");
        write_master_element(&mut file, EBML_HEADER, &header);

        let mut segment = Vec::new();

        let mut info = Vec::new();
        write_uint_element(&mut info, TIMECODE_SCALE, 1_000_000);
        write_float_element(&mut info, DURATION, 2000.0);
        write_master_element(&mut segment, INFO, &info);

        let mut tracks = Vec::new();
        let mut track_entry = Vec::new();
        write_uint_element(&mut track_entry, TRACK_NUMBER, 1);
        write_uint_element(&mut track_entry, TRACK_TYPE, TRACK_TYPE_AUDIO);
        write_string_element(&mut track_entry, CODEC_ID, "A_OPUS");
        let mut audio = Vec::new();
        write_float_element(&mut audio, SAMPLING_FREQ, 48000.0);
        write_uint_element(&mut audio, CHANNELS, 2);
        write_master_element(&mut track_entry, AUDIO, &audio);
        write_master_element(&mut tracks, TRACK_ENTRY, &track_entry);
        write_master_element(&mut segment, TRACKS, &tracks);

        // Cluster 1: Timecode first (normal)
        let mut cluster1 = Vec::new();
        write_uint_element(&mut cluster1, TIMECODE, 0);
        let mut block = Vec::new();
        write_vint(&mut block, 1);
        block.extend_from_slice(&0i16.to_be_bytes());
        block.push(0x80);
        block.extend_from_slice(&[0x42u8; 32]);
        write_id(&mut cluster1, SIMPLE_BLOCK);
        write_vint(&mut cluster1, block.len() as u64);
        cluster1.extend_from_slice(&block);
        write_master_element(&mut segment, CLUSTER, &cluster1);

        // Cluster 2: Timecode at 1000ms
        let mut cluster2 = Vec::new();
        write_uint_element(&mut cluster2, TIMECODE, 1000);
        let mut block2 = Vec::new();
        write_vint(&mut block2, 1);
        block2.extend_from_slice(&0i16.to_be_bytes());
        block2.push(0x80);
        block2.extend_from_slice(&[0x43u8; 32]);
        write_id(&mut cluster2, SIMPLE_BLOCK);
        write_vint(&mut cluster2, block2.len() as u64);
        cluster2.extend_from_slice(&block2);
        write_master_element(&mut segment, CLUSTER, &cluster2);

        write_master_element(&mut file, SEGMENT, &segment);

        let cursor = Cursor::new(file);
        let mut demuxer = MkvDemuxer::new(cursor);
        demuxer.probe().unwrap();

        // Seek to 1.5s — should find cluster 2
        demuxer.seek(Duration::from_millis(1500)).unwrap();
        // Should still be able to read a packet (even if seek overshot)
        // The important thing is that seek didn't panic
    }

    #[test]
    fn mkv_read_float_f32() {
        // Test reading a 4-byte float (f32)
        let value: f32 = 44100.0;
        let data = value.to_be_bytes().to_vec();
        let cursor = Cursor::new(data);
        let mut demuxer = MkvDemuxer::new(cursor);
        let result = demuxer.read_float(4).unwrap();
        assert!((result - 44100.0).abs() < 0.1);
    }

    #[test]
    fn mkv_read_float_invalid_size() {
        let data = vec![0u8; 16];
        let cursor = Cursor::new(data);
        let mut demuxer = MkvDemuxer::new(cursor);
        let result = demuxer.read_float(3);
        assert!(result.is_err(), "float size 3 should be invalid");
    }

    /// Build a minimal MKV with a subtitle track.
    fn make_mkv_subtitle(codec_id: &str, language: Option<&str>) -> Vec<u8> {
        let mut file = Vec::new();

        let mut header = Vec::new();
        write_string_element(&mut header, DOC_TYPE, "matroska");
        write_master_element(&mut file, EBML_HEADER, &header);

        let mut segment = Vec::new();

        let mut info = Vec::new();
        write_uint_element(&mut info, TIMECODE_SCALE, 1_000_000);
        write_float_element(&mut info, DURATION, 5000.0);
        write_master_element(&mut segment, INFO, &info);

        let mut tracks = Vec::new();
        let mut track_entry = Vec::new();
        write_uint_element(&mut track_entry, TRACK_NUMBER, 1);
        write_uint_element(&mut track_entry, TRACK_TYPE, TRACK_TYPE_SUBTITLE);
        write_string_element(&mut track_entry, CODEC_ID, codec_id);
        if let Some(lang) = language {
            write_string_element(&mut track_entry, LANGUAGE, lang);
        }

        write_master_element(&mut tracks, TRACK_ENTRY, &track_entry);
        write_master_element(&mut segment, TRACKS, &tracks);

        let mut cluster = Vec::new();
        write_uint_element(&mut cluster, TIMECODE, 0);

        let mut block = Vec::new();
        write_vint(&mut block, 1);
        block.extend_from_slice(&0i16.to_be_bytes());
        block.push(0x80);
        block.extend_from_slice(b"Hello world");

        write_id(&mut cluster, SIMPLE_BLOCK);
        write_vint(&mut cluster, block.len() as u64);
        cluster.extend_from_slice(&block);

        write_master_element(&mut segment, CLUSTER, &cluster);
        write_master_element(&mut file, SEGMENT, &segment);
        file
    }

    #[test]
    fn mkv_subtitle_probe() {
        let mkv = make_mkv_subtitle("S_TEXT/UTF8", Some("en"));
        let cursor = Cursor::new(mkv);
        let mut demuxer = MkvDemuxer::new(cursor);
        let info = demuxer.probe().unwrap();

        assert_eq!(info.format, ContainerFormat::Mkv);
        assert!(!info.has_audio());
        assert!(!info.has_video());
        assert_eq!(info.streams.len(), 1);

        match &info.streams[0] {
            StreamInfo::Subtitle { language } => {
                assert_eq!(language.as_deref(), Some("en"));
            }
            other => panic!("expected Subtitle stream, got {other:?}"),
        }
    }

    #[test]
    fn mkv_subtitle_no_language() {
        let mkv = make_mkv_subtitle("S_TEXT/ASS", None);
        let cursor = Cursor::new(mkv);
        let mut demuxer = MkvDemuxer::new(cursor);
        let info = demuxer.probe().unwrap();

        assert_eq!(info.streams.len(), 1);
        match &info.streams[0] {
            StreamInfo::Subtitle { language } => {
                assert!(language.is_none());
            }
            other => panic!("expected Subtitle stream, got {other:?}"),
        }
    }

    #[test]
    fn mkv_subtitle_read_packet() {
        let mkv = make_mkv_subtitle("S_TEXT/UTF8", Some("fr"));
        let cursor = Cursor::new(mkv);
        let mut demuxer = MkvDemuxer::new(cursor);
        let _info = demuxer.probe().unwrap();

        let packet = demuxer.next_packet().unwrap();
        assert_eq!(&*packet.data, b"Hello world");
    }

    /// Build a minimal MKV with chapters.
    fn make_mkv_with_chapters(chapters: &[(u64, &str)]) -> Vec<u8> {
        let mut file = Vec::new();

        let mut header = Vec::new();
        write_string_element(&mut header, DOC_TYPE, "matroska");
        write_master_element(&mut file, EBML_HEADER, &header);

        let mut segment = Vec::new();

        // Info
        let mut info = Vec::new();
        write_uint_element(&mut info, TIMECODE_SCALE, 1_000_000);
        write_float_element(&mut info, DURATION, 60000.0);
        write_master_element(&mut segment, INFO, &info);

        // Tracks (need at least one)
        let mut tracks = Vec::new();
        let mut track_entry = Vec::new();
        write_uint_element(&mut track_entry, TRACK_NUMBER, 1);
        write_uint_element(&mut track_entry, TRACK_TYPE, TRACK_TYPE_AUDIO);
        write_string_element(&mut track_entry, CODEC_ID, "A_OPUS");
        let mut audio = Vec::new();
        write_float_element(&mut audio, SAMPLING_FREQ, 48000.0);
        write_uint_element(&mut audio, CHANNELS, 2);
        write_master_element(&mut track_entry, AUDIO, &audio);
        write_master_element(&mut tracks, TRACK_ENTRY, &track_entry);
        write_master_element(&mut segment, TRACKS, &tracks);

        // Chapters
        let mut chapters_elem = Vec::new();
        let mut edition = Vec::new();
        for &(time_ns, title) in chapters {
            let mut atom = Vec::new();
            write_uint_element(&mut atom, CHAPTER_TIME_START, time_ns);
            let mut display = Vec::new();
            write_string_element(&mut display, CHAPTER_STRING, title);
            write_master_element(&mut atom, CHAPTER_DISPLAY, &display);
            write_master_element(&mut edition, CHAPTER_ATOM, &atom);
        }
        write_master_element(&mut chapters_elem, EDITION_ENTRY, &edition);
        write_master_element(&mut segment, CHAPTERS, &chapters_elem);

        // Cluster
        let mut cluster = Vec::new();
        write_uint_element(&mut cluster, TIMECODE, 0);
        let mut block = Vec::new();
        write_vint(&mut block, 1);
        block.extend_from_slice(&0i16.to_be_bytes());
        block.push(0x80);
        block.extend_from_slice(&[0x42u8; 64]);
        write_id(&mut cluster, SIMPLE_BLOCK);
        write_vint(&mut cluster, block.len() as u64);
        cluster.extend_from_slice(&block);
        write_master_element(&mut segment, CLUSTER, &cluster);

        write_master_element(&mut file, SEGMENT, &segment);
        file
    }

    #[test]
    fn mkv_chapters_parsed() {
        let mkv = make_mkv_with_chapters(&[
            (0, "Intro"),
            (10_000_000_000, "Chapter 1"),
            (30_000_000_000, "Chapter 2"),
        ]);
        let cursor = Cursor::new(mkv);
        let mut demuxer = MkvDemuxer::new(cursor);
        let _info = demuxer.probe().unwrap();

        let chapters = demuxer.chapters();
        assert_eq!(chapters.len(), 3);
        assert_eq!(chapters[0].time_start_ns, 0);
        assert_eq!(chapters[0].title.as_deref(), Some("Intro"));
        assert_eq!(chapters[1].time_start_ns, 10_000_000_000);
        assert_eq!(chapters[1].title.as_deref(), Some("Chapter 1"));
        assert_eq!(chapters[2].time_start_ns, 30_000_000_000);
        assert_eq!(chapters[2].title.as_deref(), Some("Chapter 2"));
    }

    #[test]
    fn mkv_no_chapters() {
        let mkv = make_mkv_audio("A_OPUS", 48000.0, 2);
        let cursor = Cursor::new(mkv);
        let mut demuxer = MkvDemuxer::new(cursor);
        let _info = demuxer.probe().unwrap();

        assert!(demuxer.chapters().is_empty());
    }
}
