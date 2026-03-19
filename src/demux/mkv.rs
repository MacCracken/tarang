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

/// Track type values
const TRACK_TYPE_VIDEO: u64 = 1;
const TRACK_TYPE_AUDIO: u64 = 2;

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
        }
    }

    /// Read a variable-length EBML integer (VINT).
    /// Returns (value, bytes_consumed).
    fn read_vint(&mut self) -> Result<(u64, usize)> {
        let mut first = [0u8; 1];
        self.reader
            .read_exact(&mut first)
            .map_err(|e| TarangError::DemuxError(format!("EBML read error: {e}")))?;

        let b = first[0];
        if b == 0 {
            return Err(TarangError::DemuxError("invalid VINT: zero".to_string()));
        }

        let len = b.leading_zeros() as usize + 1;
        if len > 8 {
            return Err(TarangError::DemuxError(format!(
                "invalid VINT length: {len}"
            )));
        }

        let mut value = (b as u64) & ((1u64 << (8 - len)) - 1);

        for _ in 1..len {
            let mut next = [0u8; 1];
            self.reader
                .read_exact(&mut next)
                .map_err(|e| TarangError::DemuxError(format!("EBML read error: {e}")))?;
            value = (value << 8) | next[0] as u64;
        }

        Ok((value, len))
    }

    /// Read an EBML element ID.
    fn read_element_id(&mut self) -> Result<(u32, usize)> {
        let mut first = [0u8; 1];
        self.reader
            .read_exact(&mut first)
            .map_err(|e| TarangError::DemuxError(format!("EBML read error: {e}")))?;

        let b = first[0];
        let len = b.leading_zeros() as usize + 1;
        if len > 4 || len == 0 {
            return Err(TarangError::DemuxError(format!(
                "invalid EBML ID length: {len} (byte=0x{b:02X})"
            )));
        }

        let mut value = b as u32;
        for _ in 1..len {
            let mut next = [0u8; 1];
            self.reader
                .read_exact(&mut next)
                .map_err(|e| TarangError::DemuxError(format!("EBML read error: {e}")))?;
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
                .map_err(|e| TarangError::DemuxError(format!("read error: {e}")))?;
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
                    .map_err(|e| TarangError::DemuxError(format!("read error: {e}")))?;
                Ok(f32::from_be_bytes(buf) as f64)
            }
            8 => {
                let mut buf = [0u8; 8];
                self.reader
                    .read_exact(&mut buf)
                    .map_err(|e| TarangError::DemuxError(format!("read error: {e}")))?;
                Ok(f64::from_be_bytes(buf))
            }
            _ => Err(TarangError::DemuxError(format!(
                "invalid float size: {size}"
            ))),
        }
    }

    /// Maximum string allocation size (64 KiB) to prevent OOM from malformed files.
    const MAX_STRING_SIZE: u64 = 65536;

    /// Maximum number of tracks to prevent excessive memory use.
    const MAX_TRACKS: usize = 128;

    /// Read a UTF-8 string of `size` bytes.
    fn read_string(&mut self, size: u64) -> Result<String> {
        if size > Self::MAX_STRING_SIZE {
            return Err(TarangError::DemuxError(format!(
                "string size {size} exceeds maximum ({})",
                Self::MAX_STRING_SIZE
            )));
        }
        let mut buf = vec![0u8; size as usize];
        self.reader
            .read_exact(&mut buf)
            .map_err(|e| TarangError::DemuxError(format!("read error: {e}")))?;
        // Strip trailing nulls
        while buf.last() == Some(&0) {
            buf.pop();
        }
        String::from_utf8(buf).map_err(|e| TarangError::DemuxError(format!("invalid UTF-8: {e}")))
    }

    /// Parse the EBML header to identify MKV vs WebM.
    fn parse_ebml_header(&mut self) -> Result<()> {
        let (id, _) = self.read_element_id()?;
        if id != EBML_HEADER {
            return Err(TarangError::UnsupportedFormat(
                "not a Matroska/WebM file: missing EBML header".to_string(),
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
                self.reader
                    .seek(SeekFrom::Current(esize as i64))
                    .map_err(io_err)?;
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
                    self.reader
                        .seek(SeekFrom::Current(esize as i64))
                        .map_err(io_err)?;
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
                self.reader
                    .seek(SeekFrom::Current(esize as i64))
                    .map_err(io_err)?;
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
        };

        while self.reader.stream_position().map_err(io_err)? < end {
            let (eid, _) = self.read_element_id()?;
            let (esize, _) = self.read_element_size()?;

            match eid {
                TRACK_NUMBER => track.number = self.read_uint(esize)?,
                TRACK_TYPE => track.track_type = self.read_uint(esize)?,
                CODEC_ID => track.codec_id = self.read_string(esize)?,
                AUDIO => self.parse_audio_settings(esize, &mut track)?,
                VIDEO => self.parse_video_settings(esize, &mut track)?,
                _ => {
                    self.reader
                        .seek(SeekFrom::Current(esize as i64))
                        .map_err(io_err)?;
                }
            }
        }

        if track.number > 0 {
            if self.tracks.len() >= Self::MAX_TRACKS {
                return Err(TarangError::DemuxError(format!(
                    "too many tracks: exceeds maximum ({})",
                    Self::MAX_TRACKS
                )));
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
                    self.reader
                        .seek(SeekFrom::Current(esize as i64))
                        .map_err(io_err)?;
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
                    self.reader
                        .seek(SeekFrom::Current(esize as i64))
                        .map_err(io_err)?;
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
                "missing Segment element".to_string(),
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
                CLUSTER => {
                    // First cluster — record its position and stop parsing headers
                    self.cluster_offset = pos;
                    break;
                }
                _ => {
                    self.reader
                        .seek(SeekFrom::Current(esize as i64))
                        .map_err(io_err)?;
                }
            }
        }

        if !found_tracks {
            return Err(TarangError::DemuxError(
                "no Tracks element found".to_string(),
            ));
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
                _ => None,
            })
            .collect();

        if streams.is_empty() {
            return Err(TarangError::DemuxError(
                "no supported streams found".to_string(),
            ));
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
                    self.reader
                        .seek(SeekFrom::Current(esize as i64))
                        .map_err(io_err)?;
                }
            }
        }
    }

    fn seek(&mut self, timestamp: Duration) -> Result<()> {
        // Validate cluster_offset against a reasonable bound
        const MAX_REASONABLE_OFFSET: u64 = u64::MAX / 2;
        if self.cluster_offset > MAX_REASONABLE_OFFSET {
            return Err(TarangError::DemuxError(format!(
                "cluster offset {} exceeds reasonable bound",
                self.cluster_offset
            )));
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

            self.reader
                .seek(SeekFrom::Current(esize as i64))
                .map_err(io_err)?;
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
            .map_err(|e| TarangError::DemuxError(format!("read error: {e}")))?;
        let relative_tc = i16::from_be_bytes(tc_buf);

        // Flags
        let mut flags = [0u8; 1];
        self.reader
            .read_exact(&mut flags)
            .map_err(|e| TarangError::DemuxError(format!("read error: {e}")))?;
        let is_keyframe = flags[0] & 0x80 != 0;

        // Header size: vint_len + 2 (timecode) + 1 (flags)
        let header_size = vint_len as u64 + 3;
        if size < header_size {
            return Err(TarangError::DemuxError(format!(
                "SimpleBlock size {size} smaller than header {header_size}"
            )));
        }
        let data_size = size - header_size;

        let mut data = vec![0u8; data_size as usize];
        self.reader
            .read_exact(&mut data)
            .map_err(|e| TarangError::DemuxError(format!("read error: {e}")))?;

        // Absolute timecode (saturate to prevent overflow)
        let abs_tc = (self.current_cluster_timecode as i64).saturating_add(relative_tc as i64);
        let timestamp = self.timecode_to_duration(abs_tc.max(0) as u64);

        // Map track number to stream index via HashMap
        let stream_index = self.track_map.get(&track_num).copied().unwrap_or(0);

        Ok(Packet {
            stream_index,
            data: Bytes::from(data),
            timestamp,
            duration: None,
            is_keyframe,
        })
    }
}

fn io_err(e: std::io::Error) -> TarangError {
    TarangError::DemuxError(format!("I/O error: {e}"))
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
}
