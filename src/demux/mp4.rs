//! MP4/M4A container demuxer (pure Rust)
//!
//! Parses ISO Base Media File Format (ISOBMFF) boxes to extract audio stream
//! metadata and produce raw codec packets. Supports AAC, ALAC, FLAC, Opus, and MP3
//! audio tracks.

use crate::core::{
    AudioCodec, AudioStreamInfo, ContainerFormat, MediaInfo, Result, SampleFormat, StreamInfo,
    TarangError,
};
use bytes::Bytes;
use std::io::{Read, Seek, SeekFrom};
use std::time::Duration;
use uuid::Uuid;

use super::{Demuxer, Packet};

/// A parsed ISOBMFF box header
#[derive(Debug)]
struct BoxHeader {
    box_type: [u8; 4],
    /// Total box size including header (0 = extends to EOF)
    size: u64,
    /// Offset where the box payload starts (after the header)
    data_offset: u64,
    /// Size of just the payload (size - header_len)
    data_size: u64,
}

/// Audio track metadata extracted from the moov box
#[derive(Debug, Clone)]
struct Mp4Track {
    track_id: u32,
    codec: AudioCodec,
    sample_rate: u32,
    channels: u16,
    bitrate: Option<u32>,
    timescale: u32,
    duration_in_timescale: u64,
    /// Sample sizes from stsz (0 = variable, stored per-sample)
    default_sample_size: u32,
    sample_sizes: Vec<u32>,
    /// Chunk offsets from stco/co64
    chunk_offsets: Vec<u64>,
    /// Sample-to-chunk table: (first_chunk, samples_per_chunk, sample_description_index)
    sample_to_chunk: Vec<(u32, u32, u32)>,
    /// Time-to-sample table: (sample_count, sample_delta)
    time_to_sample: Vec<(u32, u32)>,
}

impl Mp4Track {
    fn duration(&self) -> Option<Duration> {
        if self.timescale > 0 && self.duration_in_timescale > 0 {
            Some(Duration::from_secs_f64(
                self.duration_in_timescale as f64 / self.timescale as f64,
            ))
        } else {
            None
        }
    }

    fn total_samples(&self) -> u32 {
        if !self.sample_sizes.is_empty() {
            self.sample_sizes.len() as u32
        } else {
            self.time_to_sample.iter().map(|(count, _)| count).sum()
        }
    }
}

/// Playback cursor state for reading packets
#[derive(Debug)]
struct PlaybackState {
    track_index: usize,
    /// Current sample index (0-based)
    current_sample: u32,
    /// Accumulated timestamp in timescale units
    current_ts: u64,
}

/// MP4/M4A container demuxer
pub struct Mp4Demuxer<R: Read + Seek> {
    reader: R,
    tracks: Vec<Mp4Track>,
    info: Option<MediaInfo>,
    /// Movie-level timescale from mvhd
    movie_timescale: u32,
    movie_duration: u64,
    playback: Option<PlaybackState>,
}

impl<R: Read + Seek> Mp4Demuxer<R> {
    pub fn new(reader: R) -> Self {
        Self {
            reader,
            tracks: Vec::new(),
            info: None,
            movie_timescale: 0,
            movie_duration: 0,
            playback: None,
        }
    }

    /// Read a box header at the current position.
    fn read_box_header(&mut self) -> Result<BoxHeader> {
        let offset = self
            .reader
            .stream_position()
            .map_err(|e| TarangError::DemuxError(format!("position error: {e}")))?;

        let mut buf = [0u8; 8];
        self.reader
            .read_exact(&mut buf)
            .map_err(|e| TarangError::DemuxError(format!("failed to read box header: {e}")))?;

        let mut size = u32::from_be_bytes(
            buf[..4]
                .try_into()
                .map_err(|_| TarangError::DemuxError("invalid box header".into()))?,
        ) as u64;
        let mut box_type = [0u8; 4];
        box_type.copy_from_slice(&buf[4..8]);

        let mut header_size = 8u64;

        // Extended size
        if size == 1 {
            let mut ext = [0u8; 8];
            self.reader.read_exact(&mut ext).map_err(|e| {
                TarangError::DemuxError(format!("failed to read extended size: {e}"))
            })?;
            size = u64::from_be_bytes(ext);
            header_size = 16;
        }

        let data_offset = offset + header_size;
        // Limit size-0 boxes (extends to EOF) to a reasonable max to prevent OOM
        const MAX_BOX_SIZE: u64 = 4 * 1024 * 1024 * 1024; // 4 GB
        let data_size = if size == 0 {
            // Box extends to EOF — cap at MAX_BOX_SIZE to prevent unbounded reads
            MAX_BOX_SIZE
        } else {
            size.saturating_sub(header_size)
        };

        Ok(BoxHeader {
            box_type,
            size,
            data_offset,
            data_size,
        })
    }

    /// Skip past a box's data payload.
    fn skip_box(&mut self, header: &BoxHeader) -> Result<()> {
        if header.size == 0 {
            self.reader
                .seek(SeekFrom::End(0))
                .map_err(|e| TarangError::DemuxError(format!("seek error: {e}")))?;
        } else {
            self.reader
                .seek(SeekFrom::Start(header.data_offset + header.data_size))
                .map_err(|e| TarangError::DemuxError(format!("seek error: {e}")))?;
        }
        Ok(())
    }

    /// Parse the ftyp box to validate this is an MP4/M4A file.
    fn parse_ftyp(&mut self, header: &BoxHeader) -> Result<()> {
        let mut brand = [0u8; 4];
        self.reader
            .seek(SeekFrom::Start(header.data_offset))
            .map_err(|e| TarangError::DemuxError(format!("seek error: {e}")))?;
        self.reader
            .read_exact(&mut brand)
            .map_err(|e| TarangError::DemuxError(format!("failed to read ftyp brand: {e}")))?;

        // Accept common MP4 brands
        let valid_brands = [
            b"isom", b"iso2", b"iso3", b"iso4", b"iso5", b"iso6", b"mp41", b"mp42", b"M4A ",
            b"M4B ", b"M4V ", b"avc1", b"dash", b"mmp4",
        ];

        if !valid_brands.contains(&&brand) {
            tracing::debug!(
                brand = std::str::from_utf8(&brand).unwrap_or("????"),
                "non-standard ftyp brand, continuing anyway"
            );
        }

        Ok(())
    }

    /// Parse the mvhd (movie header) box.
    fn parse_mvhd(&mut self, header: &BoxHeader) -> Result<()> {
        self.reader
            .seek(SeekFrom::Start(header.data_offset))
            .map_err(|e| TarangError::DemuxError(format!("seek error: {e}")))?;

        let mut buf = [0u8; 4];
        self.reader
            .read_exact(&mut buf)
            .map_err(|e| TarangError::DemuxError(format!("failed to read mvhd version: {e}")))?;
        let version = buf[0];

        if version == 0 {
            // Skip creation_time(4) + modification_time(4)
            self.reader
                .seek(SeekFrom::Current(8))
                .map_err(|e| TarangError::DemuxError(format!("seek error: {e}")))?;
            let mut ts = [0u8; 4];
            self.reader
                .read_exact(&mut ts)
                .map_err(|e| TarangError::DemuxError(format!("failed to read timescale: {e}")))?;
            self.movie_timescale = u32::from_be_bytes(ts);
            let mut dur = [0u8; 4];
            self.reader
                .read_exact(&mut dur)
                .map_err(|e| TarangError::DemuxError(format!("failed to read duration: {e}")))?;
            self.movie_duration = u32::from_be_bytes(dur) as u64;
        } else {
            // Version 1: skip creation_time(8) + modification_time(8)
            self.reader
                .seek(SeekFrom::Current(16))
                .map_err(|e| TarangError::DemuxError(format!("seek error: {e}")))?;
            let mut ts = [0u8; 4];
            self.reader
                .read_exact(&mut ts)
                .map_err(|e| TarangError::DemuxError(format!("failed to read timescale: {e}")))?;
            self.movie_timescale = u32::from_be_bytes(ts);
            let mut dur = [0u8; 8];
            self.reader
                .read_exact(&mut dur)
                .map_err(|e| TarangError::DemuxError(format!("failed to read duration: {e}")))?;
            self.movie_duration = u64::from_be_bytes(dur);
        }

        Ok(())
    }

    /// Parse a trak box, extracting audio track info.
    fn parse_trak(&mut self, header: &BoxHeader) -> Result<()> {
        let mut track = Mp4Track {
            track_id: 0,
            codec: AudioCodec::Aac,
            sample_rate: 0,
            channels: 0,
            bitrate: None,
            timescale: 0,
            duration_in_timescale: 0,
            default_sample_size: 0,
            sample_sizes: Vec::new(),
            chunk_offsets: Vec::new(),
            sample_to_chunk: Vec::new(),
            time_to_sample: Vec::new(),
        };

        let mut is_audio = false;

        // We need to manually iterate since we can't borrow self mutably in the closure
        // while also borrowing track. So we collect box positions first.
        let trak_end = header.data_offset + header.data_size;
        self.reader
            .seek(SeekFrom::Start(header.data_offset))
            .map_err(|e| TarangError::DemuxError(format!("seek error: {e}")))?;

        // First pass: find mdia to check handler type and get track info
        self.parse_trak_children(trak_end, &mut track, &mut is_audio)?;

        if is_audio {
            if track.sample_rate == 0 {
                return Err(TarangError::DemuxError(
                    "audio track has sample_rate of 0".to_string(),
                ));
            }
            self.tracks.push(track);
        }

        Ok(())
    }

    fn parse_trak_children(
        &mut self,
        end: u64,
        track: &mut Mp4Track,
        is_audio: &mut bool,
    ) -> Result<()> {
        while self
            .reader
            .stream_position()
            .map_err(|e| TarangError::DemuxError(format!("position error: {e}")))?
            < end
        {
            let child = match self.read_box_header() {
                Ok(h) => h,
                Err(_) => break,
            };
            let child_end = child.data_offset + child.data_size;

            match &child.box_type {
                b"tkhd" => self.parse_tkhd(&child, track)?,
                b"mdia" => self.parse_mdia_children(child_end.min(end), track, is_audio)?,
                _ => {}
            }

            self.reader
                .seek(SeekFrom::Start(child_end.min(end)))
                .map_err(|e| TarangError::DemuxError(format!("seek error: {e}")))?;
        }
        Ok(())
    }

    fn parse_tkhd(&mut self, header: &BoxHeader, track: &mut Mp4Track) -> Result<()> {
        self.reader
            .seek(SeekFrom::Start(header.data_offset))
            .map_err(|e| TarangError::DemuxError(format!("seek error: {e}")))?;

        let mut buf = [0u8; 4];
        self.reader
            .read_exact(&mut buf)
            .map_err(|e| TarangError::DemuxError(format!("failed to read tkhd: {e}")))?;
        let version = buf[0];

        if version == 0 {
            // Skip creation_time(4) + modification_time(4)
            self.reader
                .seek(SeekFrom::Current(8))
                .map_err(|e| TarangError::DemuxError(format!("seek error: {e}")))?;
        } else {
            // Skip creation_time(8) + modification_time(8)
            self.reader
                .seek(SeekFrom::Current(16))
                .map_err(|e| TarangError::DemuxError(format!("seek error: {e}")))?;
        }

        let mut id = [0u8; 4];
        self.reader
            .read_exact(&mut id)
            .map_err(|e| TarangError::DemuxError(format!("failed to read track_id: {e}")))?;
        track.track_id = u32::from_be_bytes(id);

        Ok(())
    }

    fn parse_mdia_children(
        &mut self,
        end: u64,
        track: &mut Mp4Track,
        is_audio: &mut bool,
    ) -> Result<()> {
        while self
            .reader
            .stream_position()
            .map_err(|e| TarangError::DemuxError(format!("position error: {e}")))?
            < end
        {
            let child = match self.read_box_header() {
                Ok(h) => h,
                Err(_) => break,
            };
            let child_end = child.data_offset + child.data_size;

            match &child.box_type {
                b"mdhd" => self.parse_mdhd(&child, track)?,
                b"hdlr" => {
                    *is_audio = self.parse_hdlr(&child)?;
                }
                b"minf" => {
                    if *is_audio {
                        self.parse_minf_children(child_end.min(end), track)?;
                    }
                }
                _ => {}
            }

            self.reader
                .seek(SeekFrom::Start(child_end.min(end)))
                .map_err(|e| TarangError::DemuxError(format!("seek error: {e}")))?;
        }
        Ok(())
    }

    fn parse_mdhd(&mut self, header: &BoxHeader, track: &mut Mp4Track) -> Result<()> {
        self.reader
            .seek(SeekFrom::Start(header.data_offset))
            .map_err(|e| TarangError::DemuxError(format!("seek error: {e}")))?;

        let mut buf = [0u8; 4];
        self.reader
            .read_exact(&mut buf)
            .map_err(|e| TarangError::DemuxError(format!("failed to read mdhd: {e}")))?;
        let version = buf[0];

        if version == 0 {
            self.reader
                .seek(SeekFrom::Current(8))
                .map_err(|e| TarangError::DemuxError(format!("seek error: {e}")))?;
            let mut ts = [0u8; 4];
            self.reader
                .read_exact(&mut ts)
                .map_err(|e| TarangError::DemuxError(format!("failed to read timescale: {e}")))?;
            track.timescale = u32::from_be_bytes(ts);
            let mut dur = [0u8; 4];
            self.reader
                .read_exact(&mut dur)
                .map_err(|e| TarangError::DemuxError(format!("failed to read duration: {e}")))?;
            track.duration_in_timescale = u32::from_be_bytes(dur) as u64;
        } else {
            self.reader
                .seek(SeekFrom::Current(16))
                .map_err(|e| TarangError::DemuxError(format!("seek error: {e}")))?;
            let mut ts = [0u8; 4];
            self.reader
                .read_exact(&mut ts)
                .map_err(|e| TarangError::DemuxError(format!("failed to read timescale: {e}")))?;
            track.timescale = u32::from_be_bytes(ts);
            let mut dur = [0u8; 8];
            self.reader
                .read_exact(&mut dur)
                .map_err(|e| TarangError::DemuxError(format!("failed to read duration: {e}")))?;
            track.duration_in_timescale = u64::from_be_bytes(dur);
        }

        Ok(())
    }

    /// Parse hdlr box to determine if this is an audio track.
    fn parse_hdlr(&mut self, header: &BoxHeader) -> Result<bool> {
        self.reader
            .seek(SeekFrom::Start(header.data_offset))
            .map_err(|e| TarangError::DemuxError(format!("seek error: {e}")))?;

        // version(4) + pre_defined(4) + handler_type(4)
        let mut buf = [0u8; 12];
        self.reader
            .read_exact(&mut buf)
            .map_err(|e| TarangError::DemuxError(format!("failed to read hdlr: {e}")))?;

        Ok(&buf[8..12] == b"soun")
    }

    fn parse_minf_children(&mut self, end: u64, track: &mut Mp4Track) -> Result<()> {
        while self
            .reader
            .stream_position()
            .map_err(|e| TarangError::DemuxError(format!("position error: {e}")))?
            < end
        {
            let child = match self.read_box_header() {
                Ok(h) => h,
                Err(_) => break,
            };
            let child_end = child.data_offset + child.data_size;

            if &child.box_type == b"stbl" {
                self.parse_stbl_children(child_end.min(end), track)?;
            }

            self.reader
                .seek(SeekFrom::Start(child_end.min(end)))
                .map_err(|e| TarangError::DemuxError(format!("seek error: {e}")))?;
        }
        Ok(())
    }

    fn parse_stbl_children(&mut self, end: u64, track: &mut Mp4Track) -> Result<()> {
        while self
            .reader
            .stream_position()
            .map_err(|e| TarangError::DemuxError(format!("position error: {e}")))?
            < end
        {
            let child = match self.read_box_header() {
                Ok(h) => h,
                Err(_) => break,
            };
            let child_end = child.data_offset + child.data_size;

            match &child.box_type {
                b"stsd" => self.parse_stsd(&child, track)?,
                b"stts" => self.parse_stts(&child, track)?,
                b"stsc" => self.parse_stsc(&child, track)?,
                b"stsz" => self.parse_stsz(&child, track)?,
                b"stco" => self.parse_stco(&child, track)?,
                b"co64" => self.parse_co64(&child, track)?,
                _ => {}
            }

            self.reader
                .seek(SeekFrom::Start(child_end.min(end)))
                .map_err(|e| TarangError::DemuxError(format!("seek error: {e}")))?;
        }
        Ok(())
    }

    /// Parse stsd (sample description) to get codec type, sample rate, channels.
    fn parse_stsd(&mut self, header: &BoxHeader, track: &mut Mp4Track) -> Result<()> {
        self.reader
            .seek(SeekFrom::Start(header.data_offset))
            .map_err(|e| TarangError::DemuxError(format!("seek error: {e}")))?;

        // version(1) + flags(3) + entry_count(4)
        let mut buf = [0u8; 8];
        self.reader
            .read_exact(&mut buf)
            .map_err(|e| TarangError::DemuxError(format!("failed to read stsd header: {e}")))?;
        let entry_count = u32::from_be_bytes(
            buf[4..8]
                .try_into()
                .map_err(|_| TarangError::DemuxError("invalid header bytes".into()))?,
        );

        if entry_count == 0 {
            return Ok(());
        }

        // Read the first sample entry (audio sample entry format)
        let entry_header = self.read_box_header()?;
        let codec = match &entry_header.box_type {
            b"mp4a" => AudioCodec::Aac,
            b"alac" => AudioCodec::Alac,
            b"fLaC" => AudioCodec::Flac,
            b"Opus" => AudioCodec::Opus,
            b".mp3" | b"mp3 " => AudioCodec::Mp3,
            other => {
                let name = std::str::from_utf8(other).unwrap_or("????");
                tracing::debug!(codec = name, "unrecognized audio sample entry");
                return Ok(());
            }
        };
        track.codec = codec;

        // Audio sample entry: reserved(6) + data_ref_index(2) + reserved(8) +
        // channel_count(2) + sample_size(2) + pre_defined(2) + reserved(2) + sample_rate(4)
        let mut audio_entry = [0u8; 20];
        self.reader
            .seek(SeekFrom::Current(6 + 2))
            .map_err(|e| TarangError::DemuxError(format!("seek error: {e}")))?;
        self.reader.read_exact(&mut audio_entry).map_err(|e| {
            TarangError::DemuxError(format!("failed to read audio sample entry: {e}"))
        })?;

        // reserved(8) at offset 0..8
        track.channels = u16::from_be_bytes(
            audio_entry[8..10]
                .try_into()
                .map_err(|_| TarangError::DemuxError("invalid header bytes".into()))?,
        );
        // sample_size at 10..12, pre_defined at 12..14, reserved at 14..16
        // sample_rate is 16.16 fixed point at 16..20
        let sr_fixed = u32::from_be_bytes(
            audio_entry[16..20]
                .try_into()
                .map_err(|_| TarangError::DemuxError("invalid header bytes".into()))?,
        );
        track.sample_rate = sr_fixed >> 16;

        Ok(())
    }

    /// Maximum entries allowed in stts/stsz/stsc/stco/co64 tables to prevent OOM.
    const MAX_TABLE_ENTRIES: u32 = 50_000_000;

    /// Parse stts (time-to-sample) table.
    fn parse_stts(&mut self, header: &BoxHeader, track: &mut Mp4Track) -> Result<()> {
        self.reader
            .seek(SeekFrom::Start(header.data_offset))
            .map_err(|e| TarangError::DemuxError(format!("seek error: {e}")))?;

        let mut buf = [0u8; 8];
        self.reader
            .read_exact(&mut buf)
            .map_err(|e| TarangError::DemuxError(format!("failed to read stts: {e}")))?;
        let entry_count = u32::from_be_bytes(
            buf[4..8]
                .try_into()
                .map_err(|_| TarangError::DemuxError("invalid header bytes".into()))?,
        );

        if entry_count > Self::MAX_TABLE_ENTRIES {
            return Err(TarangError::DemuxError(format!(
                "stts entry count {entry_count} exceeds maximum ({})",
                Self::MAX_TABLE_ENTRIES
            )));
        }

        track.time_to_sample.clear();
        for _ in 0..entry_count {
            let mut entry = [0u8; 8];
            self.reader
                .read_exact(&mut entry)
                .map_err(|e| TarangError::DemuxError(format!("failed to read stts entry: {e}")))?;
            let count = u32::from_be_bytes(
                entry[0..4]
                    .try_into()
                    .map_err(|_| TarangError::DemuxError("invalid header bytes".into()))?,
            );
            let delta = u32::from_be_bytes(
                entry[4..8]
                    .try_into()
                    .map_err(|_| TarangError::DemuxError("invalid header bytes".into()))?,
            );
            track.time_to_sample.push((count, delta));
        }

        Ok(())
    }

    /// Parse stsc (sample-to-chunk) table.
    fn parse_stsc(&mut self, header: &BoxHeader, track: &mut Mp4Track) -> Result<()> {
        self.reader
            .seek(SeekFrom::Start(header.data_offset))
            .map_err(|e| TarangError::DemuxError(format!("seek error: {e}")))?;

        let mut buf = [0u8; 8];
        self.reader
            .read_exact(&mut buf)
            .map_err(|e| TarangError::DemuxError(format!("failed to read stsc: {e}")))?;
        let entry_count = u32::from_be_bytes(
            buf[4..8]
                .try_into()
                .map_err(|_| TarangError::DemuxError("invalid header bytes".into()))?,
        );

        if entry_count > Self::MAX_TABLE_ENTRIES {
            return Err(TarangError::DemuxError(format!(
                "stsc entry count {entry_count} exceeds maximum ({})",
                Self::MAX_TABLE_ENTRIES
            )));
        }

        track.sample_to_chunk.clear();
        for _ in 0..entry_count {
            let mut entry = [0u8; 12];
            self.reader
                .read_exact(&mut entry)
                .map_err(|e| TarangError::DemuxError(format!("failed to read stsc entry: {e}")))?;
            let first_chunk = u32::from_be_bytes(
                entry[0..4]
                    .try_into()
                    .map_err(|_| TarangError::DemuxError("invalid header bytes".into()))?,
            );
            let samples_per_chunk = u32::from_be_bytes(
                entry[4..8]
                    .try_into()
                    .map_err(|_| TarangError::DemuxError("invalid header bytes".into()))?,
            );
            let desc_index = u32::from_be_bytes(
                entry[8..12]
                    .try_into()
                    .map_err(|_| TarangError::DemuxError("invalid header bytes".into()))?,
            );
            track
                .sample_to_chunk
                .push((first_chunk, samples_per_chunk, desc_index));
        }

        Ok(())
    }

    /// Parse stsz (sample size) table.
    fn parse_stsz(&mut self, header: &BoxHeader, track: &mut Mp4Track) -> Result<()> {
        self.reader
            .seek(SeekFrom::Start(header.data_offset))
            .map_err(|e| TarangError::DemuxError(format!("seek error: {e}")))?;

        // version(1) + flags(3) + default_sample_size(4) + sample_count(4)
        let mut buf = [0u8; 12];
        self.reader
            .read_exact(&mut buf)
            .map_err(|e| TarangError::DemuxError(format!("failed to read stsz: {e}")))?;
        track.default_sample_size = u32::from_be_bytes(
            buf[4..8]
                .try_into()
                .map_err(|_| TarangError::DemuxError("invalid header bytes".into()))?,
        );
        let sample_count = u32::from_be_bytes(
            buf[8..12]
                .try_into()
                .map_err(|_| TarangError::DemuxError("invalid header bytes".into()))?,
        );

        if sample_count > Self::MAX_TABLE_ENTRIES {
            return Err(TarangError::DemuxError(format!(
                "stsz sample count {sample_count} exceeds maximum ({})",
                Self::MAX_TABLE_ENTRIES
            )));
        }

        track.sample_sizes.clear();
        if track.default_sample_size == 0 {
            // Variable sizes — read per-sample sizes
            for _ in 0..sample_count {
                let mut size = [0u8; 4];
                self.reader.read_exact(&mut size).map_err(|e| {
                    TarangError::DemuxError(format!("failed to read sample size: {e}"))
                })?;
                track.sample_sizes.push(u32::from_be_bytes(size));
            }
        } else {
            // Fixed size — generate entries
            for _ in 0..sample_count {
                track.sample_sizes.push(track.default_sample_size);
            }
        }

        Ok(())
    }

    /// Parse stco (chunk offset, 32-bit) table.
    fn parse_stco(&mut self, header: &BoxHeader, track: &mut Mp4Track) -> Result<()> {
        self.reader
            .seek(SeekFrom::Start(header.data_offset))
            .map_err(|e| TarangError::DemuxError(format!("seek error: {e}")))?;

        let mut buf = [0u8; 8];
        self.reader
            .read_exact(&mut buf)
            .map_err(|e| TarangError::DemuxError(format!("failed to read stco: {e}")))?;
        let entry_count = u32::from_be_bytes(
            buf[4..8]
                .try_into()
                .map_err(|_| TarangError::DemuxError("invalid header bytes".into()))?,
        );

        if entry_count > Self::MAX_TABLE_ENTRIES {
            return Err(TarangError::DemuxError(format!(
                "stco entry count {entry_count} exceeds maximum ({})",
                Self::MAX_TABLE_ENTRIES
            )));
        }

        track.chunk_offsets.clear();
        for _ in 0..entry_count {
            let mut offset = [0u8; 4];
            self.reader.read_exact(&mut offset).map_err(|e| {
                TarangError::DemuxError(format!("failed to read chunk offset: {e}"))
            })?;
            track.chunk_offsets.push(u32::from_be_bytes(offset) as u64);
        }

        Ok(())
    }

    /// Parse co64 (chunk offset, 64-bit) table.
    fn parse_co64(&mut self, header: &BoxHeader, track: &mut Mp4Track) -> Result<()> {
        self.reader
            .seek(SeekFrom::Start(header.data_offset))
            .map_err(|e| TarangError::DemuxError(format!("seek error: {e}")))?;

        let mut buf = [0u8; 8];
        self.reader
            .read_exact(&mut buf)
            .map_err(|e| TarangError::DemuxError(format!("failed to read co64: {e}")))?;
        let entry_count = u32::from_be_bytes(
            buf[4..8]
                .try_into()
                .map_err(|_| TarangError::DemuxError("invalid header bytes".into()))?,
        );

        if entry_count > Self::MAX_TABLE_ENTRIES {
            return Err(TarangError::DemuxError(format!(
                "co64 entry count {entry_count} exceeds maximum ({})",
                Self::MAX_TABLE_ENTRIES
            )));
        }

        track.chunk_offsets.clear();
        for _ in 0..entry_count {
            let mut offset = [0u8; 8];
            self.reader.read_exact(&mut offset).map_err(|e| {
                TarangError::DemuxError(format!("failed to read chunk offset: {e}"))
            })?;
            track.chunk_offsets.push(u64::from_be_bytes(offset));
        }

        Ok(())
    }

    /// Resolve which chunk and offset within the chunk a given sample index maps to.
    fn resolve_sample_offset(&self, track: &Mp4Track, sample_index: u32) -> Option<u64> {
        if track.chunk_offsets.is_empty() || track.sample_to_chunk.is_empty() {
            return None;
        }

        // Walk the sample-to-chunk table to find which chunk contains this sample
        let mut sample_cursor: u32 = 0;
        let num_chunks = track.chunk_offsets.len() as u32;

        for (i, &(first_chunk, samples_per_chunk, _)) in track.sample_to_chunk.iter().enumerate() {
            // Avoid division/modulo by zero from malformed stsc entries
            if samples_per_chunk == 0 {
                continue;
            }

            // first_chunk is 1-based
            let start_chunk = first_chunk - 1;
            let end_chunk = if i + 1 < track.sample_to_chunk.len() {
                track.sample_to_chunk[i + 1].0 - 1
            } else {
                num_chunks
            };

            let chunks_in_run = end_chunk - start_chunk;
            let samples_in_run = chunks_in_run * samples_per_chunk;

            if sample_index < sample_cursor + samples_in_run {
                // Sample is in this run
                let sample_in_run = sample_index - sample_cursor;
                let chunk_in_run = sample_in_run / samples_per_chunk;
                let sample_in_chunk = sample_in_run % samples_per_chunk;

                let chunk_index = (start_chunk + chunk_in_run) as usize;
                if chunk_index >= track.chunk_offsets.len() {
                    return None;
                }

                let mut offset = track.chunk_offsets[chunk_index];

                // Add sizes of preceding samples within this chunk
                let first_sample_in_chunk = sample_cursor + chunk_in_run * samples_per_chunk;
                for s in first_sample_in_chunk..(first_sample_in_chunk + sample_in_chunk) {
                    if let Some(&size) = track.sample_sizes.get(s as usize) {
                        offset = offset.checked_add(size as u64)?;
                    }
                }

                return Some(offset);
            }

            sample_cursor += samples_in_run;
        }

        None
    }

    /// Get the timestamp for a given sample index using the stts table.
    fn sample_timestamp(&self, track: &Mp4Track, sample_index: u32) -> Duration {
        let mut ts: u64 = 0;
        let mut remaining = sample_index;

        for &(count, delta) in &track.time_to_sample {
            if remaining <= count {
                ts += remaining as u64 * delta as u64;
                break;
            }
            ts += count as u64 * delta as u64;
            remaining -= count;
        }

        if track.timescale > 0 {
            Duration::from_secs_f64(ts as f64 / track.timescale as f64)
        } else {
            Duration::ZERO
        }
    }
}

impl<R: Read + Seek> Demuxer for Mp4Demuxer<R> {
    fn probe(&mut self) -> Result<MediaInfo> {
        self.reader
            .seek(SeekFrom::Start(0))
            .map_err(|e| TarangError::DemuxError(format!("seek error: {e}")))?;

        self.tracks.clear();

        // Parse top-level boxes
        let file_size = self
            .reader
            .seek(SeekFrom::End(0))
            .map_err(|e| TarangError::DemuxError(format!("seek error: {e}")))?;
        self.reader
            .seek(SeekFrom::Start(0))
            .map_err(|e| TarangError::DemuxError(format!("seek error: {e}")))?;

        let mut found_ftyp = false;
        let mut found_moov = false;

        while self
            .reader
            .stream_position()
            .map_err(|e| TarangError::DemuxError(format!("position error: {e}")))?
            < file_size
        {
            let header = match self.read_box_header() {
                Ok(h) => h,
                Err(_) => break,
            };

            match &header.box_type {
                b"ftyp" => {
                    self.parse_ftyp(&header)?;
                    found_ftyp = true;
                    self.skip_box(&header)?;
                }
                b"moov" => {
                    found_moov = true;
                    let moov_end = header.data_offset + header.data_size;
                    // Parse moov children
                    while self
                        .reader
                        .stream_position()
                        .map_err(|e| TarangError::DemuxError(format!("position error: {e}")))?
                        < moov_end
                    {
                        let child = match self.read_box_header() {
                            Ok(h) => h,
                            Err(_) => break,
                        };
                        let child_end = child.data_offset + child.data_size;

                        match &child.box_type {
                            b"mvhd" => self.parse_mvhd(&child)?,
                            b"trak" => self.parse_trak(&child)?,
                            _ => {}
                        }

                        self.reader
                            .seek(SeekFrom::Start(child_end.min(moov_end)))
                            .map_err(|e| TarangError::DemuxError(format!("seek error: {e}")))?;
                    }
                }
                _ => {
                    self.skip_box(&header)?;
                }
            }
        }

        if !found_ftyp && !found_moov {
            return Err(TarangError::UnsupportedFormat(
                "not an MP4 file: no ftyp or moov box found".to_string(),
            ));
        }

        if self.tracks.is_empty() {
            return Err(TarangError::DemuxError(
                "no audio tracks found in MP4".to_string(),
            ));
        }

        // Build MediaInfo
        let duration = if self.movie_timescale > 0 && self.movie_duration > 0 {
            Some(Duration::from_secs_f64(
                self.movie_duration as f64 / self.movie_timescale as f64,
            ))
        } else {
            self.tracks.first().and_then(|t| t.duration())
        };

        let streams: Vec<StreamInfo> = self
            .tracks
            .iter()
            .map(|t| {
                StreamInfo::Audio(AudioStreamInfo {
                    codec: t.codec,
                    sample_rate: t.sample_rate,
                    channels: t.channels,
                    sample_format: SampleFormat::F32,
                    bitrate: t.bitrate,
                    duration: t.duration(),
                })
            })
            .collect();

        let info = MediaInfo {
            id: Uuid::new_v4(),
            format: ContainerFormat::Mp4,
            streams,
            duration,
            file_size: Some(file_size),
            title: None,
            artist: None,
            album: None,
        };

        tracing::debug!(
            format = %info.format,
            streams = info.streams.len(),
            "MP4 probe complete"
        );

        self.playback = Some(PlaybackState {
            track_index: 0,
            current_sample: 0,
            current_ts: 0,
        });

        self.info = Some(info);
        Ok(self.info.as_ref().unwrap().clone())
    }

    fn next_packet(&mut self) -> Result<Packet> {
        let playback = self
            .playback
            .as_mut()
            .ok_or_else(|| TarangError::Pipeline("not probed yet".to_string()))?;

        let track_idx = playback.track_index;
        let sample_idx = playback.current_sample;

        let track = self
            .tracks
            .get(track_idx)
            .ok_or_else(|| TarangError::Pipeline("invalid track index".to_string()))?;

        let total = track.total_samples();
        if sample_idx >= total {
            return Err(TarangError::EndOfStream);
        }

        let sample_size = track
            .sample_sizes
            .get(sample_idx as usize)
            .copied()
            .unwrap_or(track.default_sample_size);

        let file_offset = self
            .resolve_sample_offset(track, sample_idx)
            .ok_or_else(|| {
                TarangError::DemuxError(format!("cannot resolve offset for sample {sample_idx}"))
            })?;

        let timestamp = self.sample_timestamp(track, sample_idx);

        // Read sample data
        self.reader
            .seek(SeekFrom::Start(file_offset))
            .map_err(|e| TarangError::DemuxError(format!("seek error: {e}")))?;

        const MAX_SAMPLE_SIZE: u32 = 64 * 1024 * 1024; // 64 MB guard
        if sample_size > MAX_SAMPLE_SIZE {
            return Err(TarangError::DemuxError(format!(
                "sample size {sample_size} exceeds maximum ({MAX_SAMPLE_SIZE})"
            )));
        }
        let mut buf = vec![0u8; sample_size as usize];
        self.reader
            .read_exact(&mut buf)
            .map_err(|e| TarangError::DemuxError(format!("failed to read sample: {e}")))?;

        // Advance
        let playback = self
            .playback
            .as_mut()
            .ok_or_else(|| TarangError::Pipeline("not probed yet".to_string()))?;
        playback.current_sample += 1;

        Ok(Packet {
            stream_index: track_idx,
            data: Bytes::from(buf),
            timestamp,
            duration: None,
            is_keyframe: true,
        })
    }

    fn seek(&mut self, timestamp: Duration) -> Result<()> {
        let playback = self
            .playback
            .as_mut()
            .ok_or_else(|| TarangError::Pipeline("not probed yet".to_string()))?;

        let track = self
            .tracks
            .get(playback.track_index)
            .ok_or_else(|| TarangError::Pipeline("invalid track index".to_string()))?;

        let target_ts = if track.timescale > 0 {
            (timestamp.as_secs_f64() * track.timescale as f64) as u64
        } else {
            return Err(TarangError::DemuxError("zero timescale".to_string()));
        };

        // Binary search through stts to find the target sample
        let mut sample: u32 = 0;
        let mut ts: u64 = 0;

        for &(count, delta) in &track.time_to_sample {
            // Skip entries with delta=0 to avoid infinite looping / division by zero
            if delta == 0 {
                sample += count;
                continue;
            }
            let run_duration = count as u64 * delta as u64;
            if ts + run_duration > target_ts {
                // Target is within this run
                let samples_in = ((target_ts - ts) / delta as u64) as u32;
                sample += samples_in;
                ts += samples_in as u64 * delta as u64;
                break;
            }
            ts += run_duration;
            sample += count;
        }

        let playback = self
            .playback
            .as_mut()
            .ok_or_else(|| TarangError::Pipeline("not probed yet".to_string()))?;
        playback.current_sample = sample;
        playback.current_ts = ts;

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    /// Write a box header (size + type). Returns the offset where the size was written
    /// so it can be patched later.
    fn write_box_header(buf: &mut Vec<u8>, box_type: &[u8; 4]) -> usize {
        let size_offset = buf.len();
        buf.extend_from_slice(&0u32.to_be_bytes()); // placeholder
        buf.extend_from_slice(box_type);
        size_offset
    }

    /// Patch a box's size field after writing its contents.
    fn patch_box_size(buf: &mut [u8], size_offset: usize) {
        let size = (buf.len() - size_offset) as u32;
        buf[size_offset..size_offset + 4].copy_from_slice(&size.to_be_bytes());
    }

    /// Build a minimal valid MP4 file with one AAC audio track.
    fn make_mp4_aac(
        sample_rate: u32,
        channels: u16,
        num_samples: u32,
        sample_size: u32,
    ) -> Vec<u8> {
        let mut buf = Vec::new();

        // ftyp box
        let ftyp_start = write_box_header(&mut buf, b"ftyp");
        buf.extend_from_slice(b"isom"); // major brand
        buf.extend_from_slice(&0u32.to_be_bytes()); // minor version
        buf.extend_from_slice(b"isom"); // compatible brand
        patch_box_size(&mut buf, ftyp_start);

        // moov box
        let moov_start = write_box_header(&mut buf, b"moov");

        // mvhd
        let mvhd_start = write_box_header(&mut buf, b"mvhd");
        let timescale = sample_rate;
        let duration_ts = num_samples as u64 * 1024; // AAC typically 1024 samples per frame
        buf.extend_from_slice(&0u32.to_be_bytes()); // version + flags
        buf.extend_from_slice(&0u32.to_be_bytes()); // creation_time
        buf.extend_from_slice(&0u32.to_be_bytes()); // modification_time
        buf.extend_from_slice(&timescale.to_be_bytes());
        buf.extend_from_slice(&(duration_ts as u32).to_be_bytes());
        // Remaining mvhd fields (rate, volume, reserved, matrix, pre_defined, next_track_id)
        buf.extend_from_slice(&[0u8; 80]);
        patch_box_size(&mut buf, mvhd_start);

        // trak box
        let trak_start = write_box_header(&mut buf, b"trak");

        // tkhd
        let tkhd_start = write_box_header(&mut buf, b"tkhd");
        buf.extend_from_slice(&0u32.to_be_bytes()); // version + flags
        buf.extend_from_slice(&0u32.to_be_bytes()); // creation_time
        buf.extend_from_slice(&0u32.to_be_bytes()); // modification_time
        buf.extend_from_slice(&1u32.to_be_bytes()); // track_id
        buf.extend_from_slice(&[0u8; 68]); // remaining tkhd
        patch_box_size(&mut buf, tkhd_start);

        // mdia box
        let mdia_start = write_box_header(&mut buf, b"mdia");

        // mdhd
        let mdhd_start = write_box_header(&mut buf, b"mdhd");
        buf.extend_from_slice(&0u32.to_be_bytes()); // version + flags
        buf.extend_from_slice(&0u32.to_be_bytes()); // creation_time
        buf.extend_from_slice(&0u32.to_be_bytes()); // modification_time
        buf.extend_from_slice(&timescale.to_be_bytes());
        buf.extend_from_slice(&(duration_ts as u32).to_be_bytes());
        buf.extend_from_slice(&0u32.to_be_bytes()); // language + pre_defined
        patch_box_size(&mut buf, mdhd_start);

        // hdlr
        let hdlr_start = write_box_header(&mut buf, b"hdlr");
        buf.extend_from_slice(&0u32.to_be_bytes()); // version + flags
        buf.extend_from_slice(&0u32.to_be_bytes()); // pre_defined
        buf.extend_from_slice(b"soun"); // handler_type
        buf.extend_from_slice(&[0u8; 12]); // reserved
        buf.push(0); // name (null-terminated)
        patch_box_size(&mut buf, hdlr_start);

        // minf box
        let minf_start = write_box_header(&mut buf, b"minf");

        // stbl box
        let stbl_start = write_box_header(&mut buf, b"stbl");

        // stsd
        let stsd_start = write_box_header(&mut buf, b"stsd");
        buf.extend_from_slice(&0u32.to_be_bytes()); // version + flags
        buf.extend_from_slice(&1u32.to_be_bytes()); // entry_count

        // mp4a sample entry
        let mp4a_start = write_box_header(&mut buf, b"mp4a");
        buf.extend_from_slice(&[0u8; 6]); // reserved
        buf.extend_from_slice(&1u16.to_be_bytes()); // data_ref_index
        buf.extend_from_slice(&[0u8; 8]); // reserved
        buf.extend_from_slice(&channels.to_be_bytes());
        buf.extend_from_slice(&16u16.to_be_bytes()); // sample_size (bits)
        buf.extend_from_slice(&0u16.to_be_bytes()); // pre_defined
        buf.extend_from_slice(&0u16.to_be_bytes()); // reserved
        buf.extend_from_slice(&(sample_rate << 16).to_be_bytes()); // sample_rate 16.16
        patch_box_size(&mut buf, mp4a_start);

        patch_box_size(&mut buf, stsd_start);

        // stts
        let stts_start = write_box_header(&mut buf, b"stts");
        buf.extend_from_slice(&0u32.to_be_bytes()); // version + flags
        buf.extend_from_slice(&1u32.to_be_bytes()); // entry_count
        buf.extend_from_slice(&num_samples.to_be_bytes()); // sample_count
        buf.extend_from_slice(&1024u32.to_be_bytes()); // sample_delta (1024 for AAC)
        patch_box_size(&mut buf, stts_start);

        // stsc
        let stsc_start = write_box_header(&mut buf, b"stsc");
        buf.extend_from_slice(&0u32.to_be_bytes()); // version + flags
        buf.extend_from_slice(&1u32.to_be_bytes()); // entry_count
        buf.extend_from_slice(&1u32.to_be_bytes()); // first_chunk
        buf.extend_from_slice(&num_samples.to_be_bytes()); // samples_per_chunk (all in one chunk)
        buf.extend_from_slice(&1u32.to_be_bytes()); // sample_description_index
        patch_box_size(&mut buf, stsc_start);

        // stsz
        let stsz_start = write_box_header(&mut buf, b"stsz");
        buf.extend_from_slice(&0u32.to_be_bytes()); // version + flags
        buf.extend_from_slice(&sample_size.to_be_bytes()); // default_sample_size
        buf.extend_from_slice(&num_samples.to_be_bytes()); // sample_count
        patch_box_size(&mut buf, stsz_start);

        // stco
        let _mdat_offset = buf.len() + 100; // approximate; will be patched
        let stco_start = write_box_header(&mut buf, b"stco");
        buf.extend_from_slice(&0u32.to_be_bytes()); // version + flags
        buf.extend_from_slice(&1u32.to_be_bytes()); // entry_count
        let stco_offset_pos = buf.len();
        buf.extend_from_slice(&0u32.to_be_bytes()); // chunk_offset placeholder
        patch_box_size(&mut buf, stco_start);

        patch_box_size(&mut buf, stbl_start);
        patch_box_size(&mut buf, minf_start);
        patch_box_size(&mut buf, mdia_start);
        patch_box_size(&mut buf, trak_start);
        patch_box_size(&mut buf, moov_start);

        // mdat box
        let mdat_data_offset = buf.len() + 8; // after mdat header
        // Patch stco to point to mdat data
        buf[stco_offset_pos..stco_offset_pos + 4]
            .copy_from_slice(&(mdat_data_offset as u32).to_be_bytes());

        let total_data = num_samples * sample_size;
        let mdat_start = write_box_header(&mut buf, b"mdat");
        buf.extend_from_slice(&vec![0xAAu8; total_data as usize]);
        patch_box_size(&mut buf, mdat_start);

        buf
    }

    #[test]
    fn mp4_aac_probe() {
        let mp4 = make_mp4_aac(44100, 2, 100, 512);
        let cursor = Cursor::new(mp4);
        let mut demuxer = Mp4Demuxer::new(cursor);
        let info = demuxer.probe().unwrap();

        assert_eq!(info.format, ContainerFormat::Mp4);
        assert!(info.has_audio());
        assert!(!info.has_video());

        let audio = info.audio_streams().collect::<Vec<_>>();
        assert_eq!(audio.len(), 1);
        assert_eq!(audio[0].codec, AudioCodec::Aac);
        assert_eq!(audio[0].sample_rate, 44100);
        assert_eq!(audio[0].channels, 2);
    }

    #[test]
    fn mp4_aac_mono() {
        let mp4 = make_mp4_aac(48000, 1, 50, 256);
        let cursor = Cursor::new(mp4);
        let mut demuxer = Mp4Demuxer::new(cursor);
        let info = demuxer.probe().unwrap();

        let audio = info.audio_streams().collect::<Vec<_>>();
        assert_eq!(audio[0].channels, 1);
        assert_eq!(audio[0].sample_rate, 48000);
    }

    #[test]
    fn mp4_duration() {
        let mp4 = make_mp4_aac(44100, 2, 100, 512);
        let cursor = Cursor::new(mp4);
        let mut demuxer = Mp4Demuxer::new(cursor);
        let info = demuxer.probe().unwrap();

        let duration = info.duration.unwrap();
        // 100 samples * 1024 sample_delta / 44100 timescale ≈ 2.32 seconds
        assert!((duration.as_secs_f64() - 2.32).abs() < 0.1);
    }

    #[test]
    fn mp4_file_size() {
        let mp4 = make_mp4_aac(44100, 2, 10, 128);
        let expected_size = mp4.len() as u64;
        let cursor = Cursor::new(mp4);
        let mut demuxer = Mp4Demuxer::new(cursor);
        let info = demuxer.probe().unwrap();

        assert_eq!(info.file_size, Some(expected_size));
    }

    #[test]
    fn mp4_read_packets() {
        let mp4 = make_mp4_aac(44100, 2, 10, 128);
        let cursor = Cursor::new(mp4);
        let mut demuxer = Mp4Demuxer::new(cursor);
        demuxer.probe().unwrap();

        let packet = demuxer.next_packet().unwrap();
        assert_eq!(packet.stream_index, 0);
        assert_eq!(packet.data.len(), 128);
        assert!(packet.is_keyframe);
    }

    #[test]
    fn mp4_read_all_packets() {
        let num_samples = 5u32;
        let mp4 = make_mp4_aac(44100, 2, num_samples, 64);
        let cursor = Cursor::new(mp4);
        let mut demuxer = Mp4Demuxer::new(cursor);
        demuxer.probe().unwrap();

        let mut count = 0;
        loop {
            match demuxer.next_packet() {
                Ok(_) => count += 1,
                Err(TarangError::EndOfStream) => break,
                Err(e) => panic!("unexpected error: {e}"),
            }
        }
        assert_eq!(count, num_samples);
    }

    #[test]
    fn mp4_packet_timestamps_increase() {
        let mp4 = make_mp4_aac(44100, 2, 10, 64);
        let cursor = Cursor::new(mp4);
        let mut demuxer = Mp4Demuxer::new(cursor);
        demuxer.probe().unwrap();

        let mut prev_ts = Duration::ZERO;
        for i in 0..10 {
            let packet = demuxer.next_packet().unwrap();
            if i > 0 {
                assert!(packet.timestamp > prev_ts, "timestamps must increase");
            }
            prev_ts = packet.timestamp;
        }
    }

    #[test]
    fn mp4_seek() {
        let mp4 = make_mp4_aac(44100, 2, 100, 64);
        let cursor = Cursor::new(mp4);
        let mut demuxer = Mp4Demuxer::new(cursor);
        demuxer.probe().unwrap();

        // Seek to ~1 second
        demuxer.seek(Duration::from_secs(1)).unwrap();
        let packet = demuxer.next_packet().unwrap();
        // Should be near 1 second (within one frame)
        assert!(packet.timestamp.as_secs_f64() >= 0.9);
    }

    #[test]
    fn mp4_invalid_header() {
        let cursor = Cursor::new(vec![0u8; 100]);
        let mut demuxer = Mp4Demuxer::new(cursor);
        assert!(demuxer.probe().is_err());
    }

    #[test]
    fn mp4_truncated_file_mid_box() {
        // Build a valid MP4 and truncate it in the middle of the moov box
        let mp4 = make_mp4_aac(44100, 2, 10, 128);
        // Find where moov starts and cut partway through it
        let mut moov_start = 0;
        let mut pos = 0;
        while pos + 8 <= mp4.len() {
            let size = u32::from_be_bytes(mp4[pos..pos + 4].try_into().unwrap()) as usize;
            let btype = &mp4[pos + 4..pos + 8];
            if btype == b"moov" {
                moov_start = pos;
                break;
            }
            if size == 0 {
                break;
            }
            pos += size;
        }
        assert!(moov_start > 0, "should find moov box");
        // Truncate 20 bytes into the moov box (partway through mvhd)
        let truncated = &mp4[..moov_start + 20];
        let cursor = Cursor::new(truncated.to_vec());
        let mut demuxer = Mp4Demuxer::new(cursor);
        // Should fail because moov is incomplete
        assert!(demuxer.probe().is_err());
    }

    #[test]
    fn mp4_no_moov_box() {
        // Build a file with only an ftyp box and an mdat box, no moov
        let mut buf = Vec::new();

        // ftyp box
        let ftyp_start = write_box_header(&mut buf, b"ftyp");
        buf.extend_from_slice(b"isom");
        buf.extend_from_slice(&0u32.to_be_bytes());
        buf.extend_from_slice(b"isom");
        patch_box_size(&mut buf, ftyp_start);

        // mdat box (just some data, no moov)
        let mdat_start = write_box_header(&mut buf, b"mdat");
        buf.extend_from_slice(&[0xAAu8; 256]);
        patch_box_size(&mut buf, mdat_start);

        let cursor = Cursor::new(buf);
        let mut demuxer = Mp4Demuxer::new(cursor);
        let result = demuxer.probe();
        assert!(result.is_err(), "should fail with no moov box");
    }

    /// Build an MP4 with a custom stsc table (samples_per_chunk control).
    fn make_mp4_custom_stsc(samples_per_chunk: u32, num_samples: u32, sample_size: u32) -> Vec<u8> {
        let mut buf = Vec::new();
        let sample_rate = 44100u32;
        let channels = 2u16;

        // ftyp box
        let ftyp_start = write_box_header(&mut buf, b"ftyp");
        buf.extend_from_slice(b"isom");
        buf.extend_from_slice(&0u32.to_be_bytes());
        buf.extend_from_slice(b"isom");
        patch_box_size(&mut buf, ftyp_start);

        // moov box
        let moov_start = write_box_header(&mut buf, b"moov");

        // mvhd
        let mvhd_start = write_box_header(&mut buf, b"mvhd");
        buf.extend_from_slice(&0u32.to_be_bytes()); // version + flags
        buf.extend_from_slice(&0u32.to_be_bytes()); // creation_time
        buf.extend_from_slice(&0u32.to_be_bytes()); // modification_time
        buf.extend_from_slice(&sample_rate.to_be_bytes());
        buf.extend_from_slice(&(num_samples * 1024).to_be_bytes());
        buf.extend_from_slice(&[0u8; 80]);
        patch_box_size(&mut buf, mvhd_start);

        // trak box
        let trak_start = write_box_header(&mut buf, b"trak");

        // tkhd
        let tkhd_start = write_box_header(&mut buf, b"tkhd");
        buf.extend_from_slice(&0u32.to_be_bytes());
        buf.extend_from_slice(&0u32.to_be_bytes());
        buf.extend_from_slice(&0u32.to_be_bytes());
        buf.extend_from_slice(&1u32.to_be_bytes());
        buf.extend_from_slice(&[0u8; 68]);
        patch_box_size(&mut buf, tkhd_start);

        // mdia box
        let mdia_start = write_box_header(&mut buf, b"mdia");

        // mdhd
        let mdhd_start = write_box_header(&mut buf, b"mdhd");
        buf.extend_from_slice(&0u32.to_be_bytes());
        buf.extend_from_slice(&0u32.to_be_bytes());
        buf.extend_from_slice(&0u32.to_be_bytes());
        buf.extend_from_slice(&sample_rate.to_be_bytes());
        buf.extend_from_slice(&(num_samples * 1024).to_be_bytes());
        buf.extend_from_slice(&0u32.to_be_bytes());
        patch_box_size(&mut buf, mdhd_start);

        // hdlr
        let hdlr_start = write_box_header(&mut buf, b"hdlr");
        buf.extend_from_slice(&0u32.to_be_bytes());
        buf.extend_from_slice(&0u32.to_be_bytes());
        buf.extend_from_slice(b"soun");
        buf.extend_from_slice(&[0u8; 12]);
        buf.push(0);
        patch_box_size(&mut buf, hdlr_start);

        // minf box
        let minf_start = write_box_header(&mut buf, b"minf");
        let stbl_start = write_box_header(&mut buf, b"stbl");

        // stsd
        let stsd_start = write_box_header(&mut buf, b"stsd");
        buf.extend_from_slice(&0u32.to_be_bytes());
        buf.extend_from_slice(&1u32.to_be_bytes());
        let mp4a_start = write_box_header(&mut buf, b"mp4a");
        buf.extend_from_slice(&[0u8; 6]);
        buf.extend_from_slice(&1u16.to_be_bytes());
        buf.extend_from_slice(&[0u8; 8]);
        buf.extend_from_slice(&channels.to_be_bytes());
        buf.extend_from_slice(&16u16.to_be_bytes());
        buf.extend_from_slice(&0u16.to_be_bytes());
        buf.extend_from_slice(&0u16.to_be_bytes());
        buf.extend_from_slice(&(sample_rate << 16).to_be_bytes());
        patch_box_size(&mut buf, mp4a_start);
        patch_box_size(&mut buf, stsd_start);

        // stts
        let stts_start = write_box_header(&mut buf, b"stts");
        buf.extend_from_slice(&0u32.to_be_bytes());
        buf.extend_from_slice(&1u32.to_be_bytes());
        buf.extend_from_slice(&num_samples.to_be_bytes());
        buf.extend_from_slice(&1024u32.to_be_bytes());
        patch_box_size(&mut buf, stts_start);

        // stsc — with custom samples_per_chunk
        let stsc_start = write_box_header(&mut buf, b"stsc");
        buf.extend_from_slice(&0u32.to_be_bytes());
        buf.extend_from_slice(&1u32.to_be_bytes());
        buf.extend_from_slice(&1u32.to_be_bytes()); // first_chunk
        buf.extend_from_slice(&samples_per_chunk.to_be_bytes());
        buf.extend_from_slice(&1u32.to_be_bytes());
        patch_box_size(&mut buf, stsc_start);

        // stsz
        let stsz_start = write_box_header(&mut buf, b"stsz");
        buf.extend_from_slice(&0u32.to_be_bytes());
        buf.extend_from_slice(&sample_size.to_be_bytes());
        buf.extend_from_slice(&num_samples.to_be_bytes());
        patch_box_size(&mut buf, stsz_start);

        // stco
        let stco_start = write_box_header(&mut buf, b"stco");
        buf.extend_from_slice(&0u32.to_be_bytes());
        buf.extend_from_slice(&1u32.to_be_bytes());
        let stco_offset_pos = buf.len();
        buf.extend_from_slice(&0u32.to_be_bytes());
        patch_box_size(&mut buf, stco_start);

        patch_box_size(&mut buf, stbl_start);
        patch_box_size(&mut buf, minf_start);
        patch_box_size(&mut buf, mdia_start);
        patch_box_size(&mut buf, trak_start);
        patch_box_size(&mut buf, moov_start);

        // mdat box
        let mdat_data_offset = buf.len() + 8;
        buf[stco_offset_pos..stco_offset_pos + 4]
            .copy_from_slice(&(mdat_data_offset as u32).to_be_bytes());
        let total_data = num_samples * sample_size;
        let mdat_start = write_box_header(&mut buf, b"mdat");
        buf.extend_from_slice(&vec![0xAAu8; total_data as usize]);
        patch_box_size(&mut buf, mdat_start);

        buf
    }

    /// Build an MP4 with a custom stts table (delta control).
    fn make_mp4_custom_stts(stts_entries: &[(u32, u32)], num_samples: u32) -> Vec<u8> {
        let mut buf = Vec::new();
        let sample_rate = 44100u32;
        let channels = 2u16;
        let sample_size = 64u32;

        // ftyp box
        let ftyp_start = write_box_header(&mut buf, b"ftyp");
        buf.extend_from_slice(b"isom");
        buf.extend_from_slice(&0u32.to_be_bytes());
        buf.extend_from_slice(b"isom");
        patch_box_size(&mut buf, ftyp_start);

        // moov box
        let moov_start = write_box_header(&mut buf, b"moov");

        // mvhd
        let mvhd_start = write_box_header(&mut buf, b"mvhd");
        buf.extend_from_slice(&0u32.to_be_bytes());
        buf.extend_from_slice(&0u32.to_be_bytes());
        buf.extend_from_slice(&0u32.to_be_bytes());
        buf.extend_from_slice(&sample_rate.to_be_bytes());
        buf.extend_from_slice(&(num_samples * 1024).to_be_bytes());
        buf.extend_from_slice(&[0u8; 80]);
        patch_box_size(&mut buf, mvhd_start);

        // trak box
        let trak_start = write_box_header(&mut buf, b"trak");
        let tkhd_start = write_box_header(&mut buf, b"tkhd");
        buf.extend_from_slice(&0u32.to_be_bytes());
        buf.extend_from_slice(&0u32.to_be_bytes());
        buf.extend_from_slice(&0u32.to_be_bytes());
        buf.extend_from_slice(&1u32.to_be_bytes());
        buf.extend_from_slice(&[0u8; 68]);
        patch_box_size(&mut buf, tkhd_start);

        let mdia_start = write_box_header(&mut buf, b"mdia");
        let mdhd_start = write_box_header(&mut buf, b"mdhd");
        buf.extend_from_slice(&0u32.to_be_bytes());
        buf.extend_from_slice(&0u32.to_be_bytes());
        buf.extend_from_slice(&0u32.to_be_bytes());
        buf.extend_from_slice(&sample_rate.to_be_bytes());
        buf.extend_from_slice(&(num_samples * 1024).to_be_bytes());
        buf.extend_from_slice(&0u32.to_be_bytes());
        patch_box_size(&mut buf, mdhd_start);

        let hdlr_start = write_box_header(&mut buf, b"hdlr");
        buf.extend_from_slice(&0u32.to_be_bytes());
        buf.extend_from_slice(&0u32.to_be_bytes());
        buf.extend_from_slice(b"soun");
        buf.extend_from_slice(&[0u8; 12]);
        buf.push(0);
        patch_box_size(&mut buf, hdlr_start);

        let minf_start = write_box_header(&mut buf, b"minf");
        let stbl_start = write_box_header(&mut buf, b"stbl");

        // stsd
        let stsd_start = write_box_header(&mut buf, b"stsd");
        buf.extend_from_slice(&0u32.to_be_bytes());
        buf.extend_from_slice(&1u32.to_be_bytes());
        let mp4a_start = write_box_header(&mut buf, b"mp4a");
        buf.extend_from_slice(&[0u8; 6]);
        buf.extend_from_slice(&1u16.to_be_bytes());
        buf.extend_from_slice(&[0u8; 8]);
        buf.extend_from_slice(&channels.to_be_bytes());
        buf.extend_from_slice(&16u16.to_be_bytes());
        buf.extend_from_slice(&0u16.to_be_bytes());
        buf.extend_from_slice(&0u16.to_be_bytes());
        buf.extend_from_slice(&(sample_rate << 16).to_be_bytes());
        patch_box_size(&mut buf, mp4a_start);
        patch_box_size(&mut buf, stsd_start);

        // stts — custom entries
        let stts_start = write_box_header(&mut buf, b"stts");
        buf.extend_from_slice(&0u32.to_be_bytes());
        buf.extend_from_slice(&(stts_entries.len() as u32).to_be_bytes());
        for &(count, delta) in stts_entries {
            buf.extend_from_slice(&count.to_be_bytes());
            buf.extend_from_slice(&delta.to_be_bytes());
        }
        patch_box_size(&mut buf, stts_start);

        // stsc
        let stsc_start = write_box_header(&mut buf, b"stsc");
        buf.extend_from_slice(&0u32.to_be_bytes());
        buf.extend_from_slice(&1u32.to_be_bytes());
        buf.extend_from_slice(&1u32.to_be_bytes());
        buf.extend_from_slice(&num_samples.to_be_bytes());
        buf.extend_from_slice(&1u32.to_be_bytes());
        patch_box_size(&mut buf, stsc_start);

        // stsz
        let stsz_start = write_box_header(&mut buf, b"stsz");
        buf.extend_from_slice(&0u32.to_be_bytes());
        buf.extend_from_slice(&sample_size.to_be_bytes());
        buf.extend_from_slice(&num_samples.to_be_bytes());
        patch_box_size(&mut buf, stsz_start);

        // stco
        let stco_start = write_box_header(&mut buf, b"stco");
        buf.extend_from_slice(&0u32.to_be_bytes());
        buf.extend_from_slice(&1u32.to_be_bytes());
        let stco_offset_pos = buf.len();
        buf.extend_from_slice(&0u32.to_be_bytes());
        patch_box_size(&mut buf, stco_start);

        patch_box_size(&mut buf, stbl_start);
        patch_box_size(&mut buf, minf_start);
        patch_box_size(&mut buf, mdia_start);
        patch_box_size(&mut buf, trak_start);
        patch_box_size(&mut buf, moov_start);

        let mdat_data_offset = buf.len() + 8;
        buf[stco_offset_pos..stco_offset_pos + 4]
            .copy_from_slice(&(mdat_data_offset as u32).to_be_bytes());
        let total_data = num_samples * sample_size;
        let mdat_start = write_box_header(&mut buf, b"mdat");
        buf.extend_from_slice(&vec![0xAAu8; total_data as usize]);
        patch_box_size(&mut buf, mdat_start);

        buf
    }

    #[test]
    fn test_mp4_zero_samples_per_chunk() {
        // Craft an MP4 with samples_per_chunk=0 in stsc — should not panic
        let mp4 = make_mp4_custom_stsc(0, 10, 64);
        let cursor = Cursor::new(mp4);
        let mut demuxer = Mp4Demuxer::new(cursor);
        demuxer.probe().unwrap();

        // resolve_sample_offset should return None (not panic) for any sample
        let track = &demuxer.tracks[0];
        assert!(demuxer.resolve_sample_offset(track, 0).is_none());
        assert!(demuxer.resolve_sample_offset(track, 5).is_none());
    }

    #[test]
    fn test_mp4_zero_delta_seek() {
        // Craft an MP4 with stts containing delta=0 entries, then some with delta>0
        let stts_entries = vec![
            (5, 0),     // 5 samples with delta=0 (should be skipped)
            (10, 1024), // 10 samples with normal delta
        ];
        let mp4 = make_mp4_custom_stts(&stts_entries, 15);
        let cursor = Cursor::new(mp4);
        let mut demuxer = Mp4Demuxer::new(cursor);
        demuxer.probe().unwrap();

        // Seek should not infinite loop and should succeed
        demuxer.seek(Duration::from_millis(100)).unwrap();

        // Verify we can read a packet after seeking
        let packet = demuxer.next_packet().unwrap();
        assert!(!packet.data.is_empty());
    }
}
