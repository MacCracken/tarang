//! OGG container demuxer (pure Rust)
//!
//! Parses OGG bitstream pages and extracts codec packets.
//! Identifies Vorbis, Opus, and FLAC streams from their identification headers.

use bytes::Bytes;
use std::collections::HashMap;
use std::io::{Read, Seek};
use std::time::Duration;
use tarang_core::{
    AudioCodec, AudioStreamInfo, ContainerFormat, MediaInfo, Result, SampleFormat, StreamInfo,
    TarangError,
};
use uuid::Uuid;

use crate::{Demuxer, Packet};

/// OGG page header type flags
const HEADER_TYPE_CONTINUATION: u8 = 0x01;
const HEADER_TYPE_BOS: u8 = 0x02;
const HEADER_TYPE_EOS: u8 = 0x04;

/// OGG page header size (fixed portion before segment table)
const PAGE_HEADER_SIZE: usize = 27;

/// Parsed OGG page header
#[derive(Debug)]
struct OggPage {
    header_type: u8,
    granule_position: i64,
    serial_number: u32,
    _page_sequence: u32,
    _num_segments: u8,
    _segment_table: Vec<u8>,
    /// Complete packets assembled from segments (a segment < 255 terminates a packet)
    packets: Vec<Vec<u8>>,
    /// If the last segment was 255, there's an incomplete packet carried to the next page
    partial: Option<Vec<u8>>,
}

/// Per-stream state tracked during demuxing
#[derive(Debug, Clone)]
struct OggStream {
    codec: AudioCodec,
    sample_rate: u32,
    channels: u16,
    bitrate: Option<u32>,
    /// Carry-over partial packet from a previous page
    partial_packet: Option<Vec<u8>>,
    /// Granule position from last seen page (for timestamp calculation)
    last_granule: i64,
    /// Opus pre-skip value (samples to discard at start)
    pre_skip: u32,
}

/// OGG container demuxer
pub struct OggDemuxer<R: Read + Seek> {
    reader: R,
    streams: HashMap<u32, OggStream>,
    stream_indices: Vec<u32>,
    info: Option<MediaInfo>,
    /// Duration determined by scanning to the last page
    duration: Option<Duration>,
}

impl<R: Read + Seek> OggDemuxer<R> {
    pub fn new(reader: R) -> Self {
        Self {
            reader,
            streams: HashMap::new(),
            stream_indices: Vec::new(),
            info: None,
            duration: None,
        }
    }

    /// Read and parse a single OGG page from the current reader position.
    fn read_page(&mut self) -> Result<OggPage> {
        let mut header = [0u8; PAGE_HEADER_SIZE];
        self.reader
            .read_exact(&mut header)
            .map_err(|e| TarangError::DemuxError(format!("failed to read OGG page header: {e}")))?;

        // Validate capture pattern
        if &header[0..4] != b"OggS" {
            return Err(TarangError::DemuxError(
                "invalid OGG page: bad capture pattern".to_string(),
            ));
        }

        let version = header[4];
        if version != 0 {
            return Err(TarangError::DemuxError(format!(
                "unsupported OGG version: {version}"
            )));
        }

        let header_type = header[5];
        let granule_position = i64::from_le_bytes(header[6..14].try_into().unwrap());
        let serial_number = u32::from_le_bytes(header[14..18].try_into().unwrap());
        let page_sequence = u32::from_le_bytes(header[18..22].try_into().unwrap());
        // checksum at [22..26] — we skip validation for now
        let num_segments = header[26];

        // Read segment table
        let mut segment_table = vec![0u8; num_segments as usize];
        self.reader
            .read_exact(&mut segment_table)
            .map_err(|e| TarangError::DemuxError(format!("failed to read segment table: {e}")))?;

        // Read page body (sum of all segment sizes)
        let body_size: usize = segment_table.iter().map(|&s| s as usize).sum();
        let mut body = vec![0u8; body_size];
        self.reader
            .read_exact(&mut body)
            .map_err(|e| TarangError::DemuxError(format!("failed to read page body: {e}")))?;

        // Assemble packets from segments.
        // A packet boundary occurs after any segment with size < 255.
        // A segment of exactly 255 means the packet continues in the next segment.
        let mut packets = Vec::new();
        let mut current_packet = Vec::new();
        let mut offset = 0;

        for &seg_size in &segment_table {
            let end = offset + seg_size as usize;
            current_packet.extend_from_slice(&body[offset..end]);
            offset = end;

            if seg_size < 255 {
                // Packet complete
                packets.push(std::mem::take(&mut current_packet));
            }
        }

        // If current_packet is non-empty, it's a partial packet spanning to next page
        let partial = if current_packet.is_empty() {
            None
        } else {
            Some(current_packet)
        };

        Ok(OggPage {
            header_type,
            granule_position,
            serial_number,
            _page_sequence: page_sequence,
            _num_segments: num_segments,
            _segment_table: segment_table,
            packets,
            partial,
        })
    }

    /// Identify codec from a BOS (beginning of stream) packet.
    fn identify_codec(packet: &[u8]) -> Result<OggStream> {
        // Vorbis identification header: 0x01 + "vorbis" + version(4) + channels(1) + sample_rate(4)
        if packet.len() >= 30 && packet[0] == 0x01 && &packet[1..7] == b"vorbis" {
            let channels = packet[11] as u16;
            let sample_rate = u32::from_le_bytes(packet[12..16].try_into().unwrap());
            let bitrate_max = i32::from_le_bytes(packet[16..20].try_into().unwrap());
            let bitrate_nominal = i32::from_le_bytes(packet[20..24].try_into().unwrap());

            let bitrate = if bitrate_nominal > 0 {
                Some(bitrate_nominal as u32)
            } else if bitrate_max > 0 {
                Some(bitrate_max as u32)
            } else {
                None
            };

            return Ok(OggStream {
                codec: AudioCodec::Vorbis,
                sample_rate,
                channels,
                bitrate,
                partial_packet: None,
                last_granule: 0,
                pre_skip: 0,
            });
        }

        // Opus identification header: "OpusHead" + version(1) + channels(1) + pre_skip(2) + sample_rate(4)
        if packet.len() >= 19 && &packet[0..8] == b"OpusHead" {
            let _version = packet[8];
            let channels = packet[9] as u16;
            let pre_skip = u16::from_le_bytes(packet[10..12].try_into().unwrap()) as u32;
            let sample_rate = u32::from_le_bytes(packet[12..16].try_into().unwrap());

            return Ok(OggStream {
                codec: AudioCodec::Opus,
                // Opus in OGG always uses 48kHz for granule position,
                // but the original sample rate is stored for informational purposes.
                // We report the original sample rate in metadata but use 48kHz for timestamps.
                sample_rate,
                channels,
                bitrate: None,
                partial_packet: None,
                last_granule: 0,
                pre_skip,
            });
        }

        // FLAC in OGG: 0x7F + "FLAC" + mapping version(2) + header packets(2) + "fLaC" + STREAMINFO
        if packet.len() >= 51 && packet[0] == 0x7F && &packet[1..5] == b"FLAC" {
            // STREAMINFO starts at byte 13 (after 0x7F + "FLAC" + 2 + 2 + "fLaC")
            let streaminfo_offset = 13;
            // STREAMINFO: min_block(2) + max_block(2) + min_frame(3) + max_frame(3)
            // + sample_rate(20 bits) + channels(3 bits) + bps(5 bits) + total_samples(36 bits)
            let sr_bytes = &packet[streaminfo_offset + 10..streaminfo_offset + 14];
            let sr_bits = u32::from_be_bytes(sr_bytes.try_into().unwrap());
            let sample_rate = sr_bits >> 12;
            let channels = ((sr_bits >> 9) & 0x07) as u16 + 1;

            return Ok(OggStream {
                codec: AudioCodec::Flac,
                sample_rate,
                channels,
                bitrate: None,
                partial_packet: None,
                last_granule: 0,
                pre_skip: 0,
            });
        }

        Err(TarangError::UnsupportedCodec(
            "unrecognized OGG codec".to_string(),
        ))
    }

    /// Scan backwards from the end of the file to find the last page's granule position,
    /// which gives us the total duration.
    fn scan_duration(&mut self) -> Result<Option<Duration>> {
        let end = self
            .reader
            .seek(std::io::SeekFrom::End(0))
            .map_err(|e| TarangError::DemuxError(format!("seek error: {e}")))?;

        // Search backwards for "OggS" capture pattern in the last 65536 bytes
        let search_size = 65536u64.min(end);
        let search_start = end - search_size;
        self.reader
            .seek(std::io::SeekFrom::Start(search_start))
            .map_err(|e| TarangError::DemuxError(format!("seek error: {e}")))?;

        let mut buf = vec![0u8; search_size as usize];
        self.reader
            .read_exact(&mut buf)
            .map_err(|e| TarangError::DemuxError(format!("read error: {e}")))?;

        // Find last "OggS" in the buffer
        let mut last_oggs = None;
        for i in (0..buf.len().saturating_sub(PAGE_HEADER_SIZE)).rev() {
            if &buf[i..i + 4] == b"OggS" {
                last_oggs = Some(i);
                break;
            }
        }

        let Some(offset) = last_oggs else {
            return Ok(None);
        };

        // Parse granule and serial from this page header
        let header = &buf[offset..];
        if header.len() < PAGE_HEADER_SIZE {
            return Ok(None);
        }

        let granule = i64::from_le_bytes(header[6..14].try_into().unwrap());
        let serial = u32::from_le_bytes(header[14..18].try_into().unwrap());

        if granule <= 0 {
            return Ok(None);
        }

        // Find the stream to get its sample rate
        if let Some(stream) = self.streams.get(&serial) {
            let effective_samples = if stream.pre_skip > 0 {
                (granule as u64).saturating_sub(stream.pre_skip as u64)
            } else {
                granule as u64
            };

            // Opus granule positions are always at 48kHz
            let sr = if stream.codec == AudioCodec::Opus {
                48000
            } else {
                stream.sample_rate
            };

            if sr > 0 {
                return Ok(Some(Duration::from_secs_f64(
                    effective_samples as f64 / sr as f64,
                )));
            }
        }

        Ok(None)
    }
}

impl<R: Read + Seek> Demuxer for OggDemuxer<R> {
    fn probe(&mut self) -> Result<MediaInfo> {
        // Reset to start
        self.reader
            .seek(std::io::SeekFrom::Start(0))
            .map_err(|e| TarangError::DemuxError(format!("seek error: {e}")))?;

        self.streams.clear();
        self.stream_indices.clear();

        // Read BOS pages to discover all streams
        loop {
            let page = self.read_page()?;

            if page.header_type & HEADER_TYPE_BOS != 0 {
                // BOS page — identify the codec from first packet
                if let Some(first_packet) = page.packets.first() {
                    match Self::identify_codec(first_packet) {
                        Ok(stream) => {
                            self.stream_indices.push(page.serial_number);
                            self.streams.insert(page.serial_number, stream);
                        }
                        Err(_) => {
                            // Skip unrecognized streams (could be Theora video, etc.)
                            tracing::debug!(
                                serial = page.serial_number,
                                "skipping unrecognized OGG stream"
                            );
                        }
                    }
                }
            } else {
                // First non-BOS page — all streams discovered
                break;
            }
        }

        if self.streams.is_empty() {
            return Err(TarangError::DemuxError(
                "no supported audio streams found in OGG".to_string(),
            ));
        }

        // Scan for duration
        self.duration = self.scan_duration().unwrap_or(None);

        // Build MediaInfo
        let streams: Vec<StreamInfo> = self
            .stream_indices
            .iter()
            .filter_map(|serial| self.streams.get(serial))
            .map(|s| {
                StreamInfo::Audio(AudioStreamInfo {
                    codec: s.codec,
                    sample_rate: s.sample_rate,
                    channels: s.channels,
                    sample_format: SampleFormat::F32,
                    bitrate: s.bitrate,
                    duration: self.duration,
                })
            })
            .collect();

        let info = MediaInfo {
            id: Uuid::new_v4(),
            format: ContainerFormat::Ogg,
            streams,
            duration: self.duration,
            file_size: None,
            title: None,
            artist: None,
            album: None,
        };

        self.info = Some(info.clone());

        // Seek back to start of data (after BOS pages) for packet reading
        self.reader
            .seek(std::io::SeekFrom::Start(0))
            .map_err(|e| TarangError::DemuxError(format!("seek error: {e}")))?;

        Ok(info)
    }

    fn next_packet(&mut self) -> Result<Packet> {
        loop {
            let page = self.read_page()?;
            let serial = page.serial_number;

            // Skip streams we don't track (e.g. video)
            let Some(stream) = self.streams.get_mut(&serial) else {
                continue;
            };

            // Determine stream index
            let stream_index = self
                .stream_indices
                .iter()
                .position(|&s| s == serial)
                .unwrap_or(0);

            // Handle continuation: prepend any partial packet from previous page
            let mut page_packets = page.packets;
            if page.header_type & HEADER_TYPE_CONTINUATION != 0 {
                if let Some(mut partial) = stream.partial_packet.take()
                    && let Some(first) = page_packets.first_mut()
                {
                    let mut combined = std::mem::take(&mut partial);
                    combined.extend_from_slice(first);
                    *first = combined;
                }
            } else {
                // Not a continuation — discard any leftover partial
                stream.partial_packet = None;
            }

            // Store new partial if this page has one
            if let Some(partial) = page.partial {
                stream.partial_packet = Some(partial);
            }

            // Calculate timestamp from granule position
            let granule = page.granule_position;
            if granule >= 0 {
                stream.last_granule = granule;
            }

            let sr = if stream.codec == AudioCodec::Opus {
                48000u32
            } else {
                stream.sample_rate
            };

            let timestamp = if stream.last_granule > 0 && sr > 0 {
                let samples = (stream.last_granule as u64).saturating_sub(stream.pre_skip as u64);
                Duration::from_secs_f64(samples as f64 / sr as f64)
            } else {
                Duration::ZERO
            };

            // Return the first complete data packet (skip header packets: Vorbis has 3, Opus has 2)
            for packet_data in page_packets {
                // Skip Vorbis header packets (type byte is odd for headers)
                if stream.codec == AudioCodec::Vorbis
                    && !packet_data.is_empty()
                    && packet_data[0] & 1 != 0
                    && packet_data.len() >= 7
                    && &packet_data[1..7] == b"vorbis"
                {
                    continue;
                }

                // Skip Opus header packets
                if stream.codec == AudioCodec::Opus
                    && packet_data.len() >= 8
                    && (&packet_data[0..8] == b"OpusHead" || &packet_data[0..8] == b"OpusTags")
                {
                    continue;
                }

                // Skip FLAC header packets
                if stream.codec == AudioCodec::Flac
                    && !packet_data.is_empty()
                    && (packet_data[0] == 0x7F || (packet_data[0] & 0x80 == 0))
                    && packet_data.len() >= 5
                    && &packet_data[1..5] == b"FLAC"
                {
                    continue;
                }

                if packet_data.is_empty() {
                    continue;
                }

                return Ok(Packet {
                    stream_index,
                    data: Bytes::from(packet_data),
                    timestamp,
                    duration: None,
                    is_keyframe: true,
                });
            }

            // EOS with no data packets
            if page.header_type & HEADER_TYPE_EOS != 0 {
                return Err(TarangError::EndOfStream);
            }

            // No data packets on this page, continue to next
        }
    }

    fn seek(&mut self, timestamp: Duration) -> Result<()> {
        // For OGG, seeking requires bisection search since pages aren't fixed size.
        // Simple approach: scan from start to find the target granule position.
        // A proper implementation would use bisection, but this is correct for now.

        let target_seconds = timestamp.as_secs_f64();

        // Reset to beginning
        self.reader
            .seek(std::io::SeekFrom::Start(0))
            .map_err(|e| TarangError::DemuxError(format!("seek error: {e}")))?;

        // Scan pages until we find one with a granule position past our target
        let mut last_page_start = 0u64;

        loop {
            let pos = self
                .reader
                .stream_position()
                .map_err(|e| TarangError::DemuxError(format!("position error: {e}")))?;

            match self.read_page() {
                Ok(page) => {
                    if page.granule_position >= 0
                        && let Some(stream) = self.streams.get(&page.serial_number)
                    {
                        let sr = if stream.codec == AudioCodec::Opus {
                            48000u32
                        } else {
                            stream.sample_rate
                        };

                        if sr > 0 {
                            let page_time = page.granule_position as f64 / sr as f64;
                            if page_time >= target_seconds {
                                // Seek to this page's start
                                self.reader
                                    .seek(std::io::SeekFrom::Start(last_page_start))
                                    .map_err(|e| {
                                        TarangError::DemuxError(format!("seek error: {e}"))
                                    })?;
                                return Ok(());
                            }
                        }
                    }
                    last_page_start = pos;
                }
                Err(TarangError::DemuxError(_)) => {
                    // Likely hit EOF
                    return Err(TarangError::EndOfStream);
                }
                Err(e) => return Err(e),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    /// Build a minimal valid OGG/Vorbis file with a single BOS page + one data page.
    fn make_ogg_vorbis(sample_rate: u32, channels: u8, num_data_bytes: usize) -> Vec<u8> {
        let mut buf = Vec::new();

        // --- BOS page with Vorbis identification header ---
        let mut vorbis_id = Vec::new();
        vorbis_id.push(0x01); // packet type: identification
        vorbis_id.extend_from_slice(b"vorbis");
        vorbis_id.extend_from_slice(&0u32.to_le_bytes()); // version
        vorbis_id.push(channels);
        vorbis_id.extend_from_slice(&sample_rate.to_le_bytes());
        vorbis_id.extend_from_slice(&0i32.to_le_bytes()); // bitrate max
        vorbis_id.extend_from_slice(&128000i32.to_le_bytes()); // bitrate nominal
        vorbis_id.extend_from_slice(&0i32.to_le_bytes()); // bitrate min
        vorbis_id.push(0x08); // blocksize (4 bits + 4 bits)
        vorbis_id.push(0x01); // framing

        let serial: u32 = 1;

        write_ogg_page(
            &mut buf,
            HEADER_TYPE_BOS,
            0, // granule
            serial,
            0, // page seq
            &[&vorbis_id],
        );

        // --- Data page with dummy audio data ---
        let data = vec![0x42u8; num_data_bytes];
        let granule = (sample_rate as i64) * 1; // 1 second of audio

        write_ogg_page(&mut buf, HEADER_TYPE_EOS, granule, serial, 1, &[&data]);

        buf
    }

    /// Build a minimal valid OGG/Opus file.
    fn make_ogg_opus(sample_rate: u32, channels: u8) -> Vec<u8> {
        let mut buf = Vec::new();

        // Opus identification header
        let mut opus_head = Vec::new();
        opus_head.extend_from_slice(b"OpusHead");
        opus_head.push(1); // version
        opus_head.push(channels);
        opus_head.extend_from_slice(&312u16.to_le_bytes()); // pre-skip
        opus_head.extend_from_slice(&sample_rate.to_le_bytes());
        opus_head.extend_from_slice(&0u16.to_le_bytes()); // output gain
        opus_head.push(0); // channel mapping family

        let serial: u32 = 1;

        write_ogg_page(&mut buf, HEADER_TYPE_BOS, 0, serial, 0, &[&opus_head]);

        // OpusTags page (required second header)
        let mut opus_tags = Vec::new();
        opus_tags.extend_from_slice(b"OpusTags");
        opus_tags.extend_from_slice(&7u32.to_le_bytes()); // vendor string length
        opus_tags.extend_from_slice(b"tarang\0");
        opus_tags.extend_from_slice(&0u32.to_le_bytes()); // user comment count

        write_ogg_page(&mut buf, 0, 0, serial, 1, &[&opus_tags]);

        // Data page with dummy packet
        let data = vec![0xFCu8; 64]; // fake Opus frame
        let granule: i64 = 48000; // 1 second at 48kHz

        write_ogg_page(&mut buf, HEADER_TYPE_EOS, granule, serial, 2, &[&data]);

        buf
    }

    /// Write a single OGG page with the given packets.
    fn write_ogg_page(
        buf: &mut Vec<u8>,
        header_type: u8,
        granule: i64,
        serial: u32,
        page_seq: u32,
        packets: &[&[u8]],
    ) {
        // Build segment table
        let mut segment_table = Vec::new();
        for (i, packet) in packets.iter().enumerate() {
            let len = packet.len();
            let full_segments = len / 255;
            let remainder = len % 255;
            for _ in 0..full_segments {
                segment_table.push(255u8);
            }
            // Terminal segment (< 255) — unless this is the last packet and it's
            // exactly a multiple of 255, in which case we need a 0-length terminator
            if remainder > 0 || (len > 0 && len % 255 == 0) {
                segment_table.push(remainder as u8);
            } else if i < packets.len() - 1 {
                segment_table.push(0);
            }
        }

        // Page header
        buf.extend_from_slice(b"OggS");
        buf.push(0); // version
        buf.push(header_type);
        buf.extend_from_slice(&granule.to_le_bytes());
        buf.extend_from_slice(&serial.to_le_bytes());
        buf.extend_from_slice(&page_seq.to_le_bytes());
        buf.extend_from_slice(&0u32.to_le_bytes()); // CRC (0 — we skip validation)
        buf.push(segment_table.len() as u8);
        buf.extend_from_slice(&segment_table);

        // Page body
        for packet in packets {
            buf.extend_from_slice(packet);
        }
    }

    #[test]
    fn ogg_vorbis_probe() {
        let ogg = make_ogg_vorbis(44100, 2, 256);
        let cursor = Cursor::new(ogg);
        let mut demuxer = OggDemuxer::new(cursor);
        let info = demuxer.probe().unwrap();

        assert_eq!(info.format, ContainerFormat::Ogg);
        assert!(info.has_audio());
        assert!(!info.has_video());

        let audio = info.audio_streams();
        assert_eq!(audio.len(), 1);
        assert_eq!(audio[0].codec, AudioCodec::Vorbis);
        assert_eq!(audio[0].sample_rate, 44100);
        assert_eq!(audio[0].channels, 2);
        assert_eq!(audio[0].bitrate, Some(128000));
    }

    #[test]
    fn ogg_vorbis_mono() {
        let ogg = make_ogg_vorbis(48000, 1, 128);
        let cursor = Cursor::new(ogg);
        let mut demuxer = OggDemuxer::new(cursor);
        let info = demuxer.probe().unwrap();

        let audio = info.audio_streams();
        assert_eq!(audio[0].channels, 1);
        assert_eq!(audio[0].sample_rate, 48000);
    }

    #[test]
    fn ogg_opus_probe() {
        let ogg = make_ogg_opus(48000, 2);
        let cursor = Cursor::new(ogg);
        let mut demuxer = OggDemuxer::new(cursor);
        let info = demuxer.probe().unwrap();

        let audio = info.audio_streams();
        assert_eq!(audio.len(), 1);
        assert_eq!(audio[0].codec, AudioCodec::Opus);
        assert_eq!(audio[0].sample_rate, 48000);
        assert_eq!(audio[0].channels, 2);
    }

    #[test]
    fn ogg_vorbis_duration() {
        let ogg = make_ogg_vorbis(44100, 2, 256);
        let cursor = Cursor::new(ogg);
        let mut demuxer = OggDemuxer::new(cursor);
        let info = demuxer.probe().unwrap();

        let duration = info.duration.unwrap();
        assert!((duration.as_secs_f64() - 1.0).abs() < 0.01);
    }

    #[test]
    fn ogg_vorbis_read_packets() {
        let ogg = make_ogg_vorbis(44100, 2, 256);
        let cursor = Cursor::new(ogg);
        let mut demuxer = OggDemuxer::new(cursor);
        demuxer.probe().unwrap();

        // Skip past BOS page, should get data packet
        let packet = demuxer.next_packet().unwrap();
        assert_eq!(packet.stream_index, 0);
        assert!(!packet.data.is_empty());
    }

    #[test]
    fn ogg_end_of_stream() {
        let ogg = make_ogg_vorbis(44100, 2, 64);
        let cursor = Cursor::new(ogg);
        let mut demuxer = OggDemuxer::new(cursor);
        demuxer.probe().unwrap();

        // Read all packets until EOS
        let mut count = 0;
        loop {
            match demuxer.next_packet() {
                Ok(_) => count += 1,
                Err(TarangError::EndOfStream) => break,
                Err(TarangError::DemuxError(_)) => break, // EOF on read
                Err(e) => panic!("unexpected error: {e}"),
            }
        }
        assert!(count >= 1);
    }

    #[test]
    fn ogg_invalid_header() {
        let cursor = Cursor::new(vec![0u8; 100]);
        let mut demuxer = OggDemuxer::new(cursor);
        assert!(demuxer.probe().is_err());
    }
}
