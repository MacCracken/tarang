//! Container muxing — writing audio data into container formats
//!
//! Provides muxers that write encoded audio packets into container files.
//! Currently supports WAV and OGG containers.

use crate::core::{AudioCodec, Result, TarangError};
use std::io::{Seek, Write};

/// Trait for container muxers (writers).
///
/// Muxers follow a strict state machine:
/// 1. `write_header()` — initialize the container (must be called first)
/// 2. `write_packet()` — write encoded data (call N times)
/// 3. `finalize()` — close the container (fix headers, write indices)
///
/// Calling methods out of order returns a `Pipeline` error.
pub trait Muxer {
    /// Write the container header / initialize the stream.
    /// Must be called before any `write_packet()` calls.
    fn write_header(&mut self) -> Result<()>;

    /// Write a packet of encoded audio data.
    /// Must be called after `write_header()` and before `finalize()`.
    fn write_packet(&mut self, data: &[u8]) -> Result<()>;

    /// Finalize the container (write trailing metadata, fix headers, etc.)
    /// After this call, no more packets can be written.
    fn finalize(&mut self) -> Result<()>;
}

/// Configuration for a mux stream.
#[derive(Debug, Clone)]
pub struct MuxConfig {
    /// Audio codec to write.
    pub codec: AudioCodec,
    /// Sample rate in Hz.
    pub sample_rate: u32,
    /// Number of channels.
    pub channels: u16,
    /// Bits per sample (for PCM/FLAC containers).
    pub bits_per_sample: u16,
}

/// Video track configuration for MKV/WebM muxing.
#[derive(Debug, Clone)]
pub struct VideoMuxConfig {
    /// Video codec.
    pub codec: crate::core::VideoCodec,
    /// Frame width in pixels.
    pub width: u32,
    /// Frame height in pixels.
    pub height: u32,
}

// ---- WAV Muxer ----

/// WAV container muxer — writes PCM data into a RIFF/WAVE file
pub struct WavMuxer<W: Write + Seek> {
    writer: W,
    config: MuxConfig,
    data_bytes_written: u32,
    header_written: bool,
}

impl<W: Write + Seek> WavMuxer<W> {
    pub fn new(writer: W, config: MuxConfig) -> Self {
        Self {
            writer,
            config,
            data_bytes_written: 0,
            header_written: false,
        }
    }
}

impl<W: Write + Seek> Muxer for WavMuxer<W> {
    fn write_header(&mut self) -> Result<()> {
        let byte_rate = self.config.sample_rate
            * self.config.channels as u32
            * (self.config.bits_per_sample as u32 / 8);
        let block_align = self.config.channels * (self.config.bits_per_sample / 8);

        // RIFF header (size placeholder — will be patched in finalize)
        self.writer.write_all(b"RIFF").map_err(io_err)?;
        self.writer.write_all(&0u32.to_le_bytes()).map_err(io_err)?; // placeholder
        self.writer.write_all(b"WAVE").map_err(io_err)?;

        // fmt chunk
        self.writer.write_all(b"fmt ").map_err(io_err)?;
        self.writer
            .write_all(&16u32.to_le_bytes())
            .map_err(io_err)?;
        self.writer.write_all(&1u16.to_le_bytes()).map_err(io_err)?; // PCM format
        self.writer
            .write_all(&self.config.channels.to_le_bytes())
            .map_err(io_err)?;
        self.writer
            .write_all(&self.config.sample_rate.to_le_bytes())
            .map_err(io_err)?;
        self.writer
            .write_all(&byte_rate.to_le_bytes())
            .map_err(io_err)?;
        self.writer
            .write_all(&block_align.to_le_bytes())
            .map_err(io_err)?;
        self.writer
            .write_all(&self.config.bits_per_sample.to_le_bytes())
            .map_err(io_err)?;

        // data chunk (size placeholder — will be patched in finalize)
        self.writer.write_all(b"data").map_err(io_err)?;
        self.writer.write_all(&0u32.to_le_bytes()).map_err(io_err)?; // placeholder

        self.header_written = true;
        Ok(())
    }

    fn write_packet(&mut self, data: &[u8]) -> Result<()> {
        if !self.header_written {
            return Err(TarangError::MuxError("header not written".into()));
        }
        self.writer.write_all(data).map_err(io_err)?;
        self.data_bytes_written += data.len() as u32;
        Ok(())
    }

    fn finalize(&mut self) -> Result<()> {
        // Patch RIFF size (file_size - 8)
        let riff_size = 36u32.saturating_add(self.data_bytes_written);
        self.writer
            .seek(std::io::SeekFrom::Start(4))
            .map_err(io_err)?;
        self.writer
            .write_all(&riff_size.to_le_bytes())
            .map_err(io_err)?;

        // Patch data chunk size
        self.writer
            .seek(std::io::SeekFrom::Start(40))
            .map_err(io_err)?;
        self.writer
            .write_all(&self.data_bytes_written.to_le_bytes())
            .map_err(io_err)?;

        self.writer.flush().map_err(io_err)?;
        Ok(())
    }
}

// ---- OGG Muxer ----

/// OGG container muxer — assembles OGG pages from codec packets
pub struct OggMuxer<W: Write> {
    writer: W,
    config: MuxConfig,
    serial: u32,
    page_sequence: u32,
    granule_position: i64,
    header_written: bool,
}

impl<W: Write> OggMuxer<W> {
    pub fn new(writer: W, config: MuxConfig) -> Result<Self> {
        // Validate codec upfront instead of deferring to write_header
        match config.codec {
            crate::core::AudioCodec::Opus | crate::core::AudioCodec::Vorbis => {}
            other => {
                return Err(TarangError::UnsupportedCodec(
                    format!("OGG muxer does not support {other}").into(),
                ));
            }
        }
        // Randomize serial to support concurrent streams
        let serial = {
            let mut buf = [0u8; 4];
            buf[0] = (std::process::id() & 0xFF) as u8;
            buf[1] = (std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .subsec_nanos()
                & 0xFF) as u8;
            buf[2] = ((std::process::id() >> 8) & 0xFF) as u8;
            buf[3] = 0x74; // 't' — tarang signature byte
            u32::from_le_bytes(buf)
        };
        Ok(Self {
            writer,
            config,
            serial,
            page_sequence: 0,
            granule_position: 0,
            header_written: false,
        })
    }

    /// Write a single OGG page containing the given packets.
    /// Computes and embeds the CRC-32 checksum per the OGG spec.
    fn write_page(&mut self, header_type: u8, granule: i64, packets: &[&[u8]]) -> Result<()> {
        // Build segment table
        let mut segment_table = Vec::new();
        for packet in packets {
            let len = packet.len();
            let full_segments = len / 255;
            let remainder = len % 255;
            segment_table.extend(std::iter::repeat_n(255u8, full_segments));
            segment_table.push(remainder as u8);
        }

        // Build complete page in memory so we can compute the CRC
        let body_size: usize = packets.iter().map(|p| p.len()).sum();
        let page_size = 27 + segment_table.len() + body_size;
        let mut page = Vec::with_capacity(page_size);

        // Header (27 bytes)
        page.extend_from_slice(b"OggS");
        page.push(0); // version
        page.push(header_type);
        page.extend_from_slice(&granule.to_le_bytes());
        page.extend_from_slice(&self.serial.to_le_bytes());
        page.extend_from_slice(&self.page_sequence.to_le_bytes());
        page.extend_from_slice(&0u32.to_le_bytes()); // CRC placeholder (bytes 22..26)
        page.push(segment_table.len() as u8);
        page.extend_from_slice(&segment_table);

        // Body
        for packet in packets {
            page.extend_from_slice(packet);
        }

        // Compute CRC with checksum field zeroed (already is)
        let crc = crate::demux::ogg::ogg_crc32(&page);
        page[22..26].copy_from_slice(&crc.to_le_bytes());

        self.writer.write_all(&page).map_err(io_err)?;

        self.page_sequence += 1;
        Ok(())
    }
}

impl<W: Write> Muxer for OggMuxer<W> {
    fn write_header(&mut self) -> Result<()> {
        match self.config.codec {
            AudioCodec::Opus => {
                // OpusHead identification header
                let mut head = Vec::new();
                head.extend_from_slice(b"OpusHead");
                head.push(1); // version
                head.push(self.config.channels as u8);
                head.extend_from_slice(&312u16.to_le_bytes()); // pre-skip
                head.extend_from_slice(&self.config.sample_rate.to_le_bytes());
                head.extend_from_slice(&0u16.to_le_bytes()); // output gain
                head.push(0); // channel mapping family

                self.write_page(0x02, 0, &[&head])?; // BOS

                // OpusTags
                let mut tags = Vec::new();
                tags.extend_from_slice(b"OpusTags");
                let vendor = b"tarang";
                tags.extend_from_slice(&(vendor.len() as u32).to_le_bytes());
                tags.extend_from_slice(vendor);
                tags.extend_from_slice(&0u32.to_le_bytes()); // no user comments

                self.write_page(0x00, 0, &[&tags])?;
            }
            AudioCodec::Vorbis => {
                // Minimal Vorbis identification header
                let mut id_header = Vec::new();
                id_header.push(0x01); // type: identification
                id_header.extend_from_slice(b"vorbis");
                id_header.extend_from_slice(&0u32.to_le_bytes()); // version
                id_header.push(self.config.channels as u8);
                id_header.extend_from_slice(&self.config.sample_rate.to_le_bytes());
                id_header.extend_from_slice(&0i32.to_le_bytes()); // bitrate max
                id_header.extend_from_slice(&128000i32.to_le_bytes()); // bitrate nominal
                id_header.extend_from_slice(&0i32.to_le_bytes()); // bitrate min
                id_header.push(0x08); // blocksize
                id_header.push(0x01); // framing

                self.write_page(0x02, 0, &[&id_header])?; // BOS
            }
            _ => {
                return Err(TarangError::UnsupportedCodec(
                    format!("OGG muxer does not support {}", self.config.codec).into(),
                ));
            }
        }

        self.header_written = true;
        Ok(())
    }

    fn write_packet(&mut self, data: &[u8]) -> Result<()> {
        if !self.header_written {
            return Err(TarangError::MuxError("header not written".into()));
        }

        // For Opus, granule position is at 48kHz
        let samples_per_packet = if self.config.codec == AudioCodec::Opus {
            // Opus typically uses 960 samples (20ms at 48kHz)
            960i64
        } else {
            // For Vorbis, use the configured sample rate
            1024i64
        };

        self.granule_position += samples_per_packet;
        self.write_page(0x00, self.granule_position, &[data])?;
        Ok(())
    }

    fn finalize(&mut self) -> Result<()> {
        // Write an EOS page
        self.write_page(0x04, self.granule_position, &[&[]])?;
        self.writer.flush().map_err(io_err)?;
        Ok(())
    }
}

// ---- MP4/M4A Muxer ----

/// MP4 container muxer — writes ISOBMFF boxes for MP4 files.
///
/// Supports audio-only or audio+video. Streams sample data directly to the
/// writer and patches the mdat size on `finalize()` (moov-at-end strategy).
///
/// For audio+video, use [`Mp4Muxer::new_with_video`].
pub struct Mp4Muxer<W: Write + Seek> {
    writer: W,
    config: MuxConfig,
    video_config: Option<VideoMuxConfig>,
    /// Per-sample sizes for audio stsz
    sample_sizes: Vec<u32>,
    /// Per-sample sizes for video stsz
    video_sample_sizes: Vec<u32>,
    /// Sample delta for stts (constant for audio)
    sample_delta: u32,
    /// Offset where the mdat box starts (for patching size in finalize)
    mdat_offset: u64,
    /// Running total of sample data written into mdat
    mdat_data_size: u64,
    header_written: bool,
}

impl<W: Write + Seek> Mp4Muxer<W> {
    /// Create an audio-only MP4 muxer.
    pub fn new(writer: W, config: MuxConfig) -> Self {
        // Default sample delta: 1024 for AAC, 960 for Opus, 1 for PCM
        let sample_delta = match config.codec {
            AudioCodec::Aac => 1024,
            AudioCodec::Opus => 960,
            _ => 1024,
        };
        Self {
            writer,
            config,
            video_config: None,
            sample_sizes: Vec::new(),
            video_sample_sizes: Vec::new(),
            sample_delta,
            mdat_offset: 0,
            mdat_data_size: 0,
            header_written: false,
        }
    }

    /// Create an audio+video MP4 muxer.
    pub fn new_with_video(writer: W, audio: MuxConfig, video: VideoMuxConfig) -> Self {
        let sample_delta = match audio.codec {
            AudioCodec::Aac => 1024,
            AudioCodec::Opus => 960,
            _ => 1024,
        };
        Self {
            writer,
            config: audio,
            video_config: Some(video),
            sample_sizes: Vec::new(),
            video_sample_sizes: Vec::new(),
            sample_delta,
            mdat_offset: 0,
            mdat_data_size: 0,
            header_written: false,
        }
    }

    /// Write a video sample to the mdat. Must be called after `write_header()`.
    pub fn write_video_packet(&mut self, data: &[u8]) -> Result<()> {
        if !self.header_written {
            return Err(TarangError::MuxError("header not written".into()));
        }
        if self.video_config.is_none() {
            return Err(TarangError::MuxError(
                "no video track configured — use new_with_video()".into(),
            ));
        }
        self.writer.write_all(data).map_err(io_err)?;
        self.video_sample_sizes.push(data.len() as u32);
        self.mdat_data_size += data.len() as u64;
        Ok(())
    }

    fn write_box(&mut self, box_type: &[u8; 4], data: &[u8]) -> Result<()> {
        let total = 8usize.checked_add(data.len()).ok_or_else(|| {
            TarangError::MuxError("box size overflow: data too large for u32".into())
        })?;
        if total > u32::MAX as usize {
            return Err(TarangError::MuxError(
                "box size overflow: total size exceeds u32::MAX".into(),
            ));
        }
        let size = total as u32;
        self.writer.write_all(&size.to_be_bytes()).map_err(io_err)?;
        self.writer.write_all(box_type).map_err(io_err)?;
        self.writer.write_all(data).map_err(io_err)?;
        Ok(())
    }

    fn build_ftyp(&self) -> Vec<u8> {
        let mut buf = Vec::new();
        buf.extend_from_slice(b"isom"); // major brand
        buf.extend_from_slice(&0u32.to_be_bytes()); // minor version
        buf.extend_from_slice(b"isom"); // compatible
        buf.extend_from_slice(b"mp41"); // compatible
        buf
    }

    fn build_moov(&self, mdat_offset: u64) -> Result<Vec<u8>> {
        let mut moov = Vec::new();

        // mvhd
        let mvhd = self.build_mvhd();
        write_sub_box(&mut moov, b"mvhd", &mvhd);

        // Audio trak (track 1)
        let trak = self.build_audio_trak(mdat_offset)?;
        write_sub_box(&mut moov, b"trak", &trak);

        // Video trak (track 2) if configured
        if let Some(ref video) = self.video_config {
            let vtrak = self.build_video_trak(mdat_offset, video)?;
            write_sub_box(&mut moov, b"trak", &vtrak);
        }

        Ok(moov)
    }

    fn build_mvhd(&self) -> Vec<u8> {
        let num_samples = self.sample_sizes.len() as u64;
        let timescale = self.config.sample_rate;
        let duration = num_samples
            .saturating_mul(self.sample_delta as u64)
            .min(u32::MAX as u64) as u32;

        let mut buf = Vec::new();
        buf.extend_from_slice(&0u32.to_be_bytes()); // version + flags
        buf.extend_from_slice(&0u32.to_be_bytes()); // creation_time
        buf.extend_from_slice(&0u32.to_be_bytes()); // modification_time
        buf.extend_from_slice(&timescale.to_be_bytes());
        buf.extend_from_slice(&duration.to_be_bytes());
        buf.extend_from_slice(&0x00010000u32.to_be_bytes()); // rate = 1.0
        buf.extend_from_slice(&0x0100u16.to_be_bytes()); // volume = 1.0
        buf.extend_from_slice(&[0u8; 10]); // reserved
        // Matrix (identity)
        for &v in &[0x00010000u32, 0, 0, 0, 0x00010000, 0, 0, 0, 0x40000000] {
            buf.extend_from_slice(&v.to_be_bytes());
        }
        buf.extend_from_slice(&[0u8; 24]); // pre_defined
        let next_id = if self.video_config.is_some() { 3u32 } else { 2u32 };
        buf.extend_from_slice(&next_id.to_be_bytes()); // next_track_id
        buf
    }

    fn build_audio_trak(&self, mdat_offset: u64) -> Result<Vec<u8>> {
        let mut trak = Vec::new();

        let tkhd = self.build_tkhd();
        write_sub_box(&mut trak, b"tkhd", &tkhd);

        let mdia = self.build_mdia(mdat_offset)?;
        write_sub_box(&mut trak, b"mdia", &mdia);

        Ok(trak)
    }

    fn build_tkhd(&self) -> Vec<u8> {
        let num_samples = self.sample_sizes.len() as u64;
        let duration = num_samples
            .saturating_mul(self.sample_delta as u64)
            .min(u32::MAX as u64) as u32;

        let mut buf = Vec::new();
        buf.extend_from_slice(&0x00000003u32.to_be_bytes()); // version 0 + flags (enabled+in_movie)
        buf.extend_from_slice(&0u32.to_be_bytes()); // creation_time
        buf.extend_from_slice(&0u32.to_be_bytes()); // modification_time
        buf.extend_from_slice(&1u32.to_be_bytes()); // track_id
        buf.extend_from_slice(&0u32.to_be_bytes()); // reserved
        buf.extend_from_slice(&duration.to_be_bytes());
        buf.extend_from_slice(&[0u8; 8]); // reserved
        buf.extend_from_slice(&0u16.to_be_bytes()); // layer
        buf.extend_from_slice(&0u16.to_be_bytes()); // alternate_group
        buf.extend_from_slice(&0x0100u16.to_be_bytes()); // volume = 1.0
        buf.extend_from_slice(&0u16.to_be_bytes()); // reserved
        // Matrix (identity)
        for &v in &[0x00010000u32, 0, 0, 0, 0x00010000, 0, 0, 0, 0x40000000] {
            buf.extend_from_slice(&v.to_be_bytes());
        }
        buf.extend_from_slice(&0u32.to_be_bytes()); // width
        buf.extend_from_slice(&0u32.to_be_bytes()); // height
        buf
    }

    fn build_mdia(&self, mdat_offset: u64) -> Result<Vec<u8>> {
        let mut mdia = Vec::new();

        let mdhd = self.build_mdhd();
        write_sub_box(&mut mdia, b"mdhd", &mdhd);

        let hdlr = self.build_hdlr();
        write_sub_box(&mut mdia, b"hdlr", &hdlr);

        let minf = self.build_minf(mdat_offset)?;
        write_sub_box(&mut mdia, b"minf", &minf);

        Ok(mdia)
    }

    fn build_mdhd(&self) -> Vec<u8> {
        let num_samples = self.sample_sizes.len() as u64;
        let timescale = self.config.sample_rate;
        let duration = num_samples
            .saturating_mul(self.sample_delta as u64)
            .min(u32::MAX as u64) as u32;

        let mut buf = Vec::new();
        buf.extend_from_slice(&0u32.to_be_bytes()); // version + flags
        buf.extend_from_slice(&0u32.to_be_bytes()); // creation_time
        buf.extend_from_slice(&0u32.to_be_bytes()); // modification_time
        buf.extend_from_slice(&timescale.to_be_bytes());
        buf.extend_from_slice(&duration.to_be_bytes());
        buf.extend_from_slice(&0x55C40000u32.to_be_bytes()); // language 'und' + pre_defined
        buf
    }

    fn build_hdlr(&self) -> Vec<u8> {
        let mut buf = Vec::new();
        buf.extend_from_slice(&0u32.to_be_bytes()); // version + flags
        buf.extend_from_slice(&0u32.to_be_bytes()); // pre_defined
        buf.extend_from_slice(b"soun"); // handler_type
        buf.extend_from_slice(&[0u8; 12]); // reserved
        buf.extend_from_slice(b"tarang\0"); // name
        buf
    }

    fn build_minf(&self, mdat_offset: u64) -> Result<Vec<u8>> {
        let mut minf = Vec::new();

        // smhd (sound media header)
        let mut smhd = Vec::new();
        smhd.extend_from_slice(&0u32.to_be_bytes()); // version + flags
        smhd.extend_from_slice(&0u16.to_be_bytes()); // balance
        smhd.extend_from_slice(&0u16.to_be_bytes()); // reserved
        write_sub_box(&mut minf, b"smhd", &smhd);

        // dinf + dref (data reference)
        let mut dinf = Vec::new();
        let mut dref = Vec::new();
        dref.extend_from_slice(&0u32.to_be_bytes()); // version + flags
        dref.extend_from_slice(&1u32.to_be_bytes()); // entry_count
        // url entry (self-contained)
        let mut url_entry = Vec::new();
        url_entry.extend_from_slice(&0x00000001u32.to_be_bytes()); // version + flags (self-contained)
        write_sub_box(&mut dref, b"url ", &url_entry);
        write_sub_box(&mut dinf, b"dref", &dref);
        write_sub_box(&mut minf, b"dinf", &dinf);

        let stbl = self.build_stbl(mdat_offset)?;
        write_sub_box(&mut minf, b"stbl", &stbl);

        Ok(minf)
    }

    fn build_stbl(&self, mdat_offset: u64) -> Result<Vec<u8>> {
        let mut stbl = Vec::new();

        let stsd = self.build_stsd();
        write_sub_box(&mut stbl, b"stsd", &stsd);

        let stts = self.build_stts();
        write_sub_box(&mut stbl, b"stts", &stts);

        let stsc = self.build_stsc();
        write_sub_box(&mut stbl, b"stsc", &stsc);

        let stsz = self.build_stsz();
        write_sub_box(&mut stbl, b"stsz", &stsz);

        // mdat data starts at mdat_offset + 8 (box header)
        let data_start = mdat_offset + 8;
        if data_start > u32::MAX as u64 {
            // 64-bit chunk offsets for files > 4GB
            let co64 = self.build_co64(data_start);
            write_sub_box(&mut stbl, b"co64", &co64);
        } else {
            let stco = self.build_stco(data_start);
            write_sub_box(&mut stbl, b"stco", &stco);
        }

        Ok(stbl)
    }

    fn build_stsd(&self) -> Vec<u8> {
        let mut buf = Vec::new();
        buf.extend_from_slice(&0u32.to_be_bytes()); // version + flags
        buf.extend_from_slice(&1u32.to_be_bytes()); // entry_count

        // Audio sample entry
        let box_type = match self.config.codec {
            AudioCodec::Aac => b"mp4a",
            AudioCodec::Alac => b"alac",
            AudioCodec::Opus => b"Opus",
            AudioCodec::Flac => b"fLaC",
            _ => b"mp4a",
        };

        let mut entry = Vec::new();
        entry.extend_from_slice(&[0u8; 6]); // reserved
        entry.extend_from_slice(&1u16.to_be_bytes()); // data_ref_index
        entry.extend_from_slice(&[0u8; 8]); // reserved
        entry.extend_from_slice(&self.config.channels.to_be_bytes());
        entry.extend_from_slice(&self.config.bits_per_sample.to_be_bytes());
        entry.extend_from_slice(&0u16.to_be_bytes()); // pre_defined
        entry.extend_from_slice(&0u16.to_be_bytes()); // reserved
        entry.extend_from_slice(&(self.config.sample_rate << 16).to_be_bytes()); // 16.16 fixed

        write_sub_box(&mut buf, box_type, &entry);
        buf
    }

    fn build_stts(&self) -> Vec<u8> {
        let mut buf = Vec::new();
        buf.extend_from_slice(&0u32.to_be_bytes()); // version + flags
        buf.extend_from_slice(&1u32.to_be_bytes()); // entry_count
        buf.extend_from_slice(&(self.sample_sizes.len() as u32).to_be_bytes());
        buf.extend_from_slice(&self.sample_delta.to_be_bytes());
        buf
    }

    fn build_stsc(&self) -> Vec<u8> {
        let mut buf = Vec::new();
        buf.extend_from_slice(&0u32.to_be_bytes()); // version + flags
        buf.extend_from_slice(&1u32.to_be_bytes()); // entry_count
        buf.extend_from_slice(&1u32.to_be_bytes()); // first_chunk
        buf.extend_from_slice(&(self.sample_sizes.len() as u32).to_be_bytes()); // samples_per_chunk
        buf.extend_from_slice(&1u32.to_be_bytes()); // sample_description_index
        buf
    }

    fn build_stsz(&self) -> Vec<u8> {
        let mut buf = Vec::new();
        buf.extend_from_slice(&0u32.to_be_bytes()); // version + flags

        // Guard: no samples — emit an empty variable-size stsz (default_sample_size=0, count=0)
        if self.sample_sizes.is_empty() {
            buf.extend_from_slice(&0u32.to_be_bytes()); // default_sample_size = 0
            buf.extend_from_slice(&0u32.to_be_bytes()); // sample_count = 0
            return buf;
        }

        // Check if all samples are the same size
        let all_same = self.sample_sizes.windows(2).all(|w| w[0] == w[1]);

        if all_same {
            buf.extend_from_slice(&self.sample_sizes[0].to_be_bytes());
            buf.extend_from_slice(&(self.sample_sizes.len() as u32).to_be_bytes());
        } else {
            buf.extend_from_slice(&0u32.to_be_bytes()); // default_sample_size = 0 (variable)
            buf.extend_from_slice(&(self.sample_sizes.len() as u32).to_be_bytes());
            for &size in &self.sample_sizes {
                buf.extend_from_slice(&size.to_be_bytes());
            }
        }
        buf
    }

    fn build_stco(&self, data_start: u64) -> Vec<u8> {
        let mut buf = Vec::new();
        buf.extend_from_slice(&0u32.to_be_bytes()); // version + flags
        buf.extend_from_slice(&1u32.to_be_bytes()); // entry_count (single chunk)
        buf.extend_from_slice(&(data_start as u32).to_be_bytes());
        buf
    }

    /// Build a co64 box (64-bit chunk offset table) for files > 4GB.
    fn build_co64(&self, data_start: u64) -> Vec<u8> {
        let mut buf = Vec::new();
        buf.extend_from_slice(&0u32.to_be_bytes()); // version + flags
        buf.extend_from_slice(&1u32.to_be_bytes()); // entry_count (single chunk)
        buf.extend_from_slice(&data_start.to_be_bytes()); // 64-bit offset
        buf
    }

    // ---- Video track support ----

    fn build_video_trak(
        &self,
        mdat_offset: u64,
        video: &VideoMuxConfig,
    ) -> Result<Vec<u8>> {
        let mut trak = Vec::new();

        let tkhd = self.build_video_tkhd(video);
        write_sub_box(&mut trak, b"tkhd", &tkhd);

        let mdia = self.build_video_mdia(mdat_offset, video)?;
        write_sub_box(&mut trak, b"mdia", &mdia);

        Ok(trak)
    }

    fn build_video_tkhd(&self, video: &VideoMuxConfig) -> Vec<u8> {
        // Use a fixed 30fps for duration estimation
        let duration = (self.video_sample_sizes.len() as u64)
            .saturating_mul(self.sample_delta as u64)
            .min(u32::MAX as u64) as u32;

        let mut buf = Vec::new();
        buf.extend_from_slice(&0x00000003u32.to_be_bytes()); // version 0 + flags
        buf.extend_from_slice(&0u32.to_be_bytes()); // creation_time
        buf.extend_from_slice(&0u32.to_be_bytes()); // modification_time
        buf.extend_from_slice(&2u32.to_be_bytes()); // track_id = 2
        buf.extend_from_slice(&0u32.to_be_bytes()); // reserved
        buf.extend_from_slice(&duration.to_be_bytes());
        buf.extend_from_slice(&[0u8; 8]); // reserved
        buf.extend_from_slice(&0u16.to_be_bytes()); // layer
        buf.extend_from_slice(&0u16.to_be_bytes()); // alternate_group
        buf.extend_from_slice(&0u16.to_be_bytes()); // volume = 0 (video)
        buf.extend_from_slice(&0u16.to_be_bytes()); // reserved
        for &v in &[0x00010000u32, 0, 0, 0, 0x00010000, 0, 0, 0, 0x40000000] {
            buf.extend_from_slice(&v.to_be_bytes());
        }
        // width and height as 16.16 fixed-point
        buf.extend_from_slice(&((video.width as u32) << 16).to_be_bytes());
        buf.extend_from_slice(&((video.height as u32) << 16).to_be_bytes());
        buf
    }

    fn build_video_mdia(
        &self,
        mdat_offset: u64,
        video: &VideoMuxConfig,
    ) -> Result<Vec<u8>> {
        let mut mdia = Vec::new();

        // mdhd — use 90kHz timescale (standard for video)
        let video_timescale = 90000u32;
        let num_video = self.video_sample_sizes.len() as u64;
        let duration = num_video
            .saturating_mul(3000) // 90000/30 = 3000 ticks per frame at 30fps
            .min(u32::MAX as u64) as u32;

        let mut mdhd = Vec::new();
        mdhd.extend_from_slice(&0u32.to_be_bytes());
        mdhd.extend_from_slice(&0u32.to_be_bytes());
        mdhd.extend_from_slice(&0u32.to_be_bytes());
        mdhd.extend_from_slice(&video_timescale.to_be_bytes());
        mdhd.extend_from_slice(&duration.to_be_bytes());
        mdhd.extend_from_slice(&0x55C40000u32.to_be_bytes()); // language 'und'
        write_sub_box(&mut mdia, b"mdhd", &mdhd);

        // hdlr — video handler
        let mut hdlr = Vec::new();
        hdlr.extend_from_slice(&0u32.to_be_bytes());
        hdlr.extend_from_slice(&0u32.to_be_bytes());
        hdlr.extend_from_slice(b"vide"); // handler_type
        hdlr.extend_from_slice(&[0u8; 12]);
        hdlr.extend_from_slice(b"tarang\0");
        write_sub_box(&mut mdia, b"hdlr", &hdlr);

        // minf
        let mut minf = Vec::new();

        // vmhd (video media header)
        let mut vmhd = Vec::new();
        vmhd.extend_from_slice(&0x00000001u32.to_be_bytes()); // version 0 + flags=1
        vmhd.extend_from_slice(&0u16.to_be_bytes()); // graphicsmode
        vmhd.extend_from_slice(&[0u8; 6]); // opcolor
        write_sub_box(&mut minf, b"vmhd", &vmhd);

        // dinf + dref
        let mut dinf = Vec::new();
        let mut dref = Vec::new();
        dref.extend_from_slice(&0u32.to_be_bytes());
        dref.extend_from_slice(&1u32.to_be_bytes());
        let mut url_entry = Vec::new();
        url_entry.extend_from_slice(&0x00000001u32.to_be_bytes());
        write_sub_box(&mut dref, b"url ", &url_entry);
        write_sub_box(&mut dinf, b"dref", &dref);
        write_sub_box(&mut minf, b"dinf", &dinf);

        // stbl
        let stbl = self.build_video_stbl(mdat_offset, video)?;
        write_sub_box(&mut minf, b"stbl", &stbl);

        write_sub_box(&mut mdia, b"minf", &minf);
        Ok(mdia)
    }

    fn build_video_stbl(
        &self,
        mdat_offset: u64,
        video: &VideoMuxConfig,
    ) -> Result<Vec<u8>> {
        let mut stbl = Vec::new();

        // stsd — video sample description
        let stsd = self.build_video_stsd(video);
        write_sub_box(&mut stbl, b"stsd", &stsd);

        // stts — 3000 ticks per sample (30fps at 90kHz)
        let mut stts = Vec::new();
        stts.extend_from_slice(&0u32.to_be_bytes());
        stts.extend_from_slice(&1u32.to_be_bytes());
        stts.extend_from_slice(&(self.video_sample_sizes.len() as u32).to_be_bytes());
        stts.extend_from_slice(&3000u32.to_be_bytes()); // 90000/30
        write_sub_box(&mut stbl, b"stts", &stts);

        // stss — sync samples (all keyframes for now)
        let mut stss = Vec::new();
        stss.extend_from_slice(&0u32.to_be_bytes());
        stss.extend_from_slice(&(self.video_sample_sizes.len() as u32).to_be_bytes());
        for i in 1..=self.video_sample_sizes.len() as u32 {
            stss.extend_from_slice(&i.to_be_bytes());
        }
        write_sub_box(&mut stbl, b"stss", &stss);

        // stsc
        let mut stsc = Vec::new();
        stsc.extend_from_slice(&0u32.to_be_bytes());
        stsc.extend_from_slice(&1u32.to_be_bytes());
        stsc.extend_from_slice(&1u32.to_be_bytes());
        stsc.extend_from_slice(&(self.video_sample_sizes.len() as u32).to_be_bytes());
        stsc.extend_from_slice(&1u32.to_be_bytes());
        write_sub_box(&mut stbl, b"stsc", &stsc);

        // stsz
        let mut stsz = Vec::new();
        stsz.extend_from_slice(&0u32.to_be_bytes());
        stsz.extend_from_slice(&0u32.to_be_bytes()); // variable size
        stsz.extend_from_slice(&(self.video_sample_sizes.len() as u32).to_be_bytes());
        for &sz in &self.video_sample_sizes {
            stsz.extend_from_slice(&sz.to_be_bytes());
        }
        write_sub_box(&mut stbl, b"stsz", &stsz);

        // stco — video data offset (after all audio data in mdat)
        let audio_data_size: u64 = self.sample_sizes.iter().map(|&s| s as u64).sum();
        let video_data_start = mdat_offset + 8 + audio_data_size;
        if video_data_start > u32::MAX as u64 {
            let co64 = self.build_co64(video_data_start);
            write_sub_box(&mut stbl, b"co64", &co64);
        } else {
            let stco = self.build_stco(video_data_start);
            write_sub_box(&mut stbl, b"stco", &stco);
        }

        Ok(stbl)
    }

    fn build_video_stsd(&self, video: &VideoMuxConfig) -> Vec<u8> {
        let mut buf = Vec::new();
        buf.extend_from_slice(&0u32.to_be_bytes()); // version + flags
        buf.extend_from_slice(&1u32.to_be_bytes()); // entry_count

        let box_type: &[u8; 4] = match video.codec {
            crate::core::VideoCodec::H264 => b"avc1",
            crate::core::VideoCodec::H265 => b"hev1",
            crate::core::VideoCodec::Vp9 => b"vp09",
            crate::core::VideoCodec::Av1 => b"av01",
            crate::core::VideoCodec::Vp8 => b"vp08",
            _ => b"avc1",
        };

        // Visual sample entry (ISO 14496-12 section 12.1.3)
        let mut entry = Vec::new();
        entry.extend_from_slice(&[0u8; 6]); // reserved
        entry.extend_from_slice(&1u16.to_be_bytes()); // data_reference_index
        entry.extend_from_slice(&0u16.to_be_bytes()); // pre_defined
        entry.extend_from_slice(&0u16.to_be_bytes()); // reserved
        entry.extend_from_slice(&[0u8; 12]); // pre_defined
        entry.extend_from_slice(&(video.width as u16).to_be_bytes());
        entry.extend_from_slice(&(video.height as u16).to_be_bytes());
        entry.extend_from_slice(&0x00480000u32.to_be_bytes()); // horiz_resolution 72dpi
        entry.extend_from_slice(&0x00480000u32.to_be_bytes()); // vert_resolution 72dpi
        entry.extend_from_slice(&0u32.to_be_bytes()); // reserved
        entry.extend_from_slice(&1u16.to_be_bytes()); // frame_count
        entry.extend_from_slice(&[0u8; 32]); // compressorname
        entry.extend_from_slice(&0x0018u16.to_be_bytes()); // depth = 24
        entry.extend_from_slice(&(-1i16).to_be_bytes()); // pre_defined = -1

        write_sub_box(&mut buf, box_type, &entry);
        buf
    }
}

impl<W: Write + Seek> Muxer for Mp4Muxer<W> {
    fn write_header(&mut self) -> Result<()> {
        // Write ftyp immediately
        let ftyp = self.build_ftyp();
        self.write_box(b"ftyp", &ftyp)?;

        // Remember where mdat starts and write placeholder header
        self.mdat_offset = self.writer.stream_position().map_err(io_err)?;
        self.writer.write_all(&0u32.to_be_bytes()).map_err(io_err)?; // placeholder size
        self.writer.write_all(b"mdat").map_err(io_err)?;

        self.header_written = true;
        Ok(())
    }

    fn write_packet(&mut self, data: &[u8]) -> Result<()> {
        if !self.header_written {
            return Err(TarangError::MuxError("header not written".into()));
        }
        self.writer.write_all(data).map_err(io_err)?;
        self.sample_sizes.push(data.len() as u32);
        self.mdat_data_size += data.len() as u64;
        Ok(())
    }

    fn finalize(&mut self) -> Result<()> {
        // Patch mdat box size
        let mdat_size = (8 + self.mdat_data_size) as u32;
        let current_pos = self.writer.stream_position().map_err(io_err)?;
        self.writer
            .seek(std::io::SeekFrom::Start(self.mdat_offset))
            .map_err(io_err)?;
        self.writer
            .write_all(&mdat_size.to_be_bytes())
            .map_err(io_err)?;
        self.writer
            .seek(std::io::SeekFrom::Start(current_pos))
            .map_err(io_err)?;

        // Write moov box
        let moov_data = self.build_moov(self.mdat_offset)?;
        self.write_box(b"moov", &moov_data)?;

        self.writer.flush().map_err(io_err)?;
        Ok(())
    }
}

fn write_sub_box(buf: &mut Vec<u8>, box_type: &[u8; 4], data: &[u8]) {
    let size = (8 + data.len()) as u32;
    buf.extend_from_slice(&size.to_be_bytes());
    buf.extend_from_slice(box_type);
    buf.extend_from_slice(data);
}

// ---- Fragmented MP4 / DASH Muxer ----

/// Fragmented MP4 muxer for DASH/HLS streaming.
///
/// Generates fMP4 segments: an init segment (`ftyp` + `moov`) followed by
/// media segments (`moof` + `mdat`). Each media segment is self-contained
/// and can be served independently.
///
/// ```text
/// Init:    [ftyp][moov]
/// Seg 1:   [moof][mdat]
/// Seg 2:   [moof][mdat]
/// ...
/// ```
pub struct FragmentedMp4Muxer<W: Write> {
    writer: W,
    config: MuxConfig,
    video_config: Option<VideoMuxConfig>,
    sequence_number: u32,
    sample_delta: u32,
    /// Samples accumulated for the current fragment.
    fragment_audio_sizes: Vec<u32>,
    fragment_video_sizes: Vec<u32>,
    /// Raw data for the current fragment's mdat.
    fragment_data: Vec<u8>,
    init_written: bool,
}

impl<W: Write> FragmentedMp4Muxer<W> {
    /// Create a fragmented MP4 muxer for audio-only DASH segments.
    pub fn new(writer: W, config: MuxConfig) -> Self {
        let sample_delta = match config.codec {
            AudioCodec::Aac => 1024,
            AudioCodec::Opus => 960,
            _ => 1024,
        };
        Self {
            writer,
            config,
            video_config: None,
            sequence_number: 1,
            sample_delta,
            fragment_audio_sizes: Vec::new(),
            fragment_video_sizes: Vec::new(),
            fragment_data: Vec::new(),
            init_written: false,
        }
    }

    /// Create a fragmented MP4 muxer for audio+video DASH segments.
    pub fn new_with_video(writer: W, audio: MuxConfig, video: VideoMuxConfig) -> Self {
        let sample_delta = match audio.codec {
            AudioCodec::Aac => 1024,
            AudioCodec::Opus => 960,
            _ => 1024,
        };
        Self {
            writer,
            config: audio,
            video_config: Some(video),
            sequence_number: 1,
            sample_delta,
            fragment_audio_sizes: Vec::new(),
            fragment_video_sizes: Vec::new(),
            fragment_data: Vec::new(),
            init_written: false,
        }
    }

    /// Write the init segment (`ftyp` + `moov`). Call once before any fragments.
    pub fn write_init_segment(&mut self) -> Result<()> {
        // ftyp
        let mut ftyp = Vec::new();
        ftyp.extend_from_slice(b"isom");
        ftyp.extend_from_slice(&0u32.to_be_bytes());
        ftyp.extend_from_slice(b"isom");
        ftyp.extend_from_slice(b"iso5"); // fMP4 compatible brand
        ftyp.extend_from_slice(b"dash");
        self.write_box(b"ftyp", &ftyp)?;

        // moov (empty sample tables — data is in fragments)
        let mut moov = Vec::new();

        // mvhd
        let mut mvhd = Vec::new();
        mvhd.extend_from_slice(&0u32.to_be_bytes()); // version + flags
        mvhd.extend_from_slice(&0u32.to_be_bytes()); // creation_time
        mvhd.extend_from_slice(&0u32.to_be_bytes()); // modification_time
        mvhd.extend_from_slice(&self.config.sample_rate.to_be_bytes());
        mvhd.extend_from_slice(&0u32.to_be_bytes()); // duration = 0 (unknown)
        mvhd.extend_from_slice(&0x00010000u32.to_be_bytes()); // rate
        mvhd.extend_from_slice(&0x0100u16.to_be_bytes()); // volume
        mvhd.extend_from_slice(&[0u8; 10]);
        for &v in &[0x00010000u32, 0, 0, 0, 0x00010000, 0, 0, 0, 0x40000000] {
            mvhd.extend_from_slice(&v.to_be_bytes());
        }
        mvhd.extend_from_slice(&[0u8; 24]);
        let next_id = if self.video_config.is_some() { 3u32 } else { 2u32 };
        mvhd.extend_from_slice(&next_id.to_be_bytes());
        write_sub_box(&mut moov, b"mvhd", &mvhd);

        // Audio trak (track 1) — empty stbl for fMP4
        let audio_trak = self.build_fmp4_audio_trak();
        write_sub_box(&mut moov, b"trak", &audio_trak);

        // mvex (movie extends — signals fragmented structure)
        let mut mvex = Vec::new();
        let mut trex = Vec::new();
        trex.extend_from_slice(&0u32.to_be_bytes()); // version + flags
        trex.extend_from_slice(&1u32.to_be_bytes()); // track_id
        trex.extend_from_slice(&1u32.to_be_bytes()); // default_sample_description_index
        trex.extend_from_slice(&self.sample_delta.to_be_bytes()); // default_sample_duration
        trex.extend_from_slice(&0u32.to_be_bytes()); // default_sample_size
        trex.extend_from_slice(&0u32.to_be_bytes()); // default_sample_flags
        write_sub_box(&mut mvex, b"trex", &trex);
        write_sub_box(&mut moov, b"mvex", &mvex);

        self.write_box(b"moov", &moov)?;
        self.init_written = true;
        Ok(())
    }

    /// Add an audio sample to the current fragment.
    pub fn add_audio_sample(&mut self, data: &[u8]) -> Result<()> {
        if !self.init_written {
            return Err(TarangError::MuxError("init segment not written".into()));
        }
        self.fragment_audio_sizes.push(data.len() as u32);
        self.fragment_data.extend_from_slice(data);
        Ok(())
    }

    /// Add a video sample to the current fragment.
    pub fn add_video_sample(&mut self, data: &[u8]) -> Result<()> {
        if !self.init_written {
            return Err(TarangError::MuxError("init segment not written".into()));
        }
        if self.video_config.is_none() {
            return Err(TarangError::MuxError("no video track configured".into()));
        }
        self.fragment_video_sizes.push(data.len() as u32);
        self.fragment_data.extend_from_slice(data);
        Ok(())
    }

    /// Flush the current fragment as a `moof` + `mdat` pair.
    ///
    /// Returns the number of samples written. Call this periodically
    /// (e.g. every 2-4 seconds) to produce DASH-compatible segments.
    pub fn flush_fragment(&mut self) -> Result<usize> {
        if self.fragment_audio_sizes.is_empty() && self.fragment_video_sizes.is_empty() {
            return Ok(0);
        }

        let total_samples =
            self.fragment_audio_sizes.len() + self.fragment_video_sizes.len();

        // Build moof
        let mut moof = Vec::new();

        // mfhd (movie fragment header)
        let mut mfhd = Vec::new();
        mfhd.extend_from_slice(&0u32.to_be_bytes()); // version + flags
        mfhd.extend_from_slice(&self.sequence_number.to_be_bytes());
        write_sub_box(&mut moof, b"mfhd", &mfhd);

        // traf for audio (track 1)
        if !self.fragment_audio_sizes.is_empty() {
            let traf = self.build_traf(1, &self.fragment_audio_sizes, self.sample_delta);
            write_sub_box(&mut moof, b"traf", &traf);
        }

        // traf for video (track 2)
        if !self.fragment_video_sizes.is_empty() {
            let traf = self.build_traf(2, &self.fragment_video_sizes, 3000); // 90kHz/30fps
            write_sub_box(&mut moof, b"traf", &traf);
        }

        self.write_box(b"moof", &moof)?;

        // Write mdat with accumulated data
        let frag_data = std::mem::take(&mut self.fragment_data);
        self.write_box(b"mdat", &frag_data)?;

        // Reset for next fragment
        self.fragment_audio_sizes.clear();
        self.fragment_video_sizes.clear();
        self.fragment_data.clear();
        self.sequence_number += 1;

        Ok(total_samples)
    }

    fn build_traf(&self, track_id: u32, sample_sizes: &[u32], _sample_duration: u32) -> Vec<u8> {
        let mut traf = Vec::new();

        // tfhd (track fragment header)
        // flags: 0x020000 = default-base-is-moof
        let mut tfhd = Vec::new();
        tfhd.extend_from_slice(&0x00020000u32.to_be_bytes()); // version 0 + flags
        tfhd.extend_from_slice(&track_id.to_be_bytes());
        write_sub_box(&mut traf, b"tfhd", &tfhd);

        // trun (track run)
        // flags: 0x000201 = data-offset-present + sample-size-present
        let mut trun = Vec::new();
        trun.extend_from_slice(&0x00000201u32.to_be_bytes()); // version 0 + flags
        trun.extend_from_slice(&(sample_sizes.len() as u32).to_be_bytes());
        // data_offset: placeholder (offset from moof start to mdat payload)
        // We don't know the exact offset yet, but for single-track fragments
        // this is typically the size of moof + 8 (mdat header). Use 0 for now —
        // players compute from moof position.
        trun.extend_from_slice(&0u32.to_be_bytes()); // data_offset (relative)

        for &size in sample_sizes {
            trun.extend_from_slice(&size.to_be_bytes());
        }
        write_sub_box(&mut traf, b"trun", &trun);

        traf
    }

    fn build_fmp4_audio_trak(&self) -> Vec<u8> {
        let mut trak = Vec::new();

        // tkhd
        let mut tkhd = Vec::new();
        tkhd.extend_from_slice(&0x00000003u32.to_be_bytes());
        tkhd.extend_from_slice(&0u32.to_be_bytes()); // creation
        tkhd.extend_from_slice(&0u32.to_be_bytes()); // modification
        tkhd.extend_from_slice(&1u32.to_be_bytes()); // track_id
        tkhd.extend_from_slice(&0u32.to_be_bytes()); // reserved
        tkhd.extend_from_slice(&0u32.to_be_bytes()); // duration = 0
        tkhd.extend_from_slice(&[0u8; 8]);
        tkhd.extend_from_slice(&0u16.to_be_bytes());
        tkhd.extend_from_slice(&0u16.to_be_bytes());
        tkhd.extend_from_slice(&0x0100u16.to_be_bytes()); // volume
        tkhd.extend_from_slice(&0u16.to_be_bytes());
        for &v in &[0x00010000u32, 0, 0, 0, 0x00010000, 0, 0, 0, 0x40000000] {
            tkhd.extend_from_slice(&v.to_be_bytes());
        }
        tkhd.extend_from_slice(&0u32.to_be_bytes()); // width
        tkhd.extend_from_slice(&0u32.to_be_bytes()); // height
        write_sub_box(&mut trak, b"tkhd", &tkhd);

        // mdia
        let mut mdia = Vec::new();

        // mdhd
        let mut mdhd = Vec::new();
        mdhd.extend_from_slice(&0u32.to_be_bytes());
        mdhd.extend_from_slice(&0u32.to_be_bytes());
        mdhd.extend_from_slice(&0u32.to_be_bytes());
        mdhd.extend_from_slice(&self.config.sample_rate.to_be_bytes());
        mdhd.extend_from_slice(&0u32.to_be_bytes()); // duration = 0
        mdhd.extend_from_slice(&0x55C40000u32.to_be_bytes());
        write_sub_box(&mut mdia, b"mdhd", &mdhd);

        // hdlr
        let mut hdlr = Vec::new();
        hdlr.extend_from_slice(&0u32.to_be_bytes());
        hdlr.extend_from_slice(&0u32.to_be_bytes());
        hdlr.extend_from_slice(b"soun");
        hdlr.extend_from_slice(&[0u8; 12]);
        hdlr.extend_from_slice(b"tarang\0");
        write_sub_box(&mut mdia, b"hdlr", &hdlr);

        // minf with empty stbl
        let mut minf = Vec::new();
        let mut smhd = Vec::new();
        smhd.extend_from_slice(&[0u8; 8]);
        write_sub_box(&mut minf, b"smhd", &smhd);

        let mut dinf = Vec::new();
        let mut dref = Vec::new();
        dref.extend_from_slice(&0u32.to_be_bytes());
        dref.extend_from_slice(&1u32.to_be_bytes());
        let mut url_entry = Vec::new();
        url_entry.extend_from_slice(&0x00000001u32.to_be_bytes());
        write_sub_box(&mut dref, b"url ", &url_entry);
        write_sub_box(&mut dinf, b"dref", &dref);
        write_sub_box(&mut minf, b"dinf", &dinf);

        // stbl — must exist but empty for fMP4
        let mut stbl = Vec::new();

        // stsd — audio sample description (same as regular muxer)
        let mut stsd = Vec::new();
        stsd.extend_from_slice(&0u32.to_be_bytes());
        stsd.extend_from_slice(&1u32.to_be_bytes());
        let box_type = match self.config.codec {
            AudioCodec::Aac => b"mp4a",
            AudioCodec::Opus => b"Opus",
            _ => b"mp4a",
        };
        let mut entry = Vec::new();
        entry.extend_from_slice(&[0u8; 6]);
        entry.extend_from_slice(&1u16.to_be_bytes());
        entry.extend_from_slice(&[0u8; 8]);
        entry.extend_from_slice(&self.config.channels.to_be_bytes());
        entry.extend_from_slice(&self.config.bits_per_sample.to_be_bytes());
        entry.extend_from_slice(&0u16.to_be_bytes());
        entry.extend_from_slice(&0u16.to_be_bytes());
        entry.extend_from_slice(&(self.config.sample_rate << 16).to_be_bytes());
        write_sub_box(&mut stsd, box_type, &entry);
        write_sub_box(&mut stbl, b"stsd", &stsd);

        // Empty tables (required by spec even for fMP4)
        let empty_table = [0u8; 8]; // version+flags + 0 entries
        write_sub_box(&mut stbl, b"stts", &empty_table);
        write_sub_box(&mut stbl, b"stsc", &empty_table);
        let mut stsz = Vec::new();
        stsz.extend_from_slice(&0u32.to_be_bytes());
        stsz.extend_from_slice(&0u32.to_be_bytes());
        stsz.extend_from_slice(&0u32.to_be_bytes());
        write_sub_box(&mut stbl, b"stsz", &stsz);
        write_sub_box(&mut stbl, b"stco", &empty_table);

        write_sub_box(&mut minf, b"stbl", &stbl);
        write_sub_box(&mut mdia, b"minf", &minf);
        write_sub_box(&mut trak, b"mdia", &mdia);

        trak
    }

    fn write_box(&mut self, box_type: &[u8; 4], data: &[u8]) -> Result<()> {
        let size = (8 + data.len()) as u32;
        self.writer.write_all(&size.to_be_bytes()).map_err(io_err)?;
        self.writer.write_all(box_type).map_err(io_err)?;
        self.writer.write_all(data).map_err(io_err)?;
        Ok(())
    }
}

// ---- MKV/WebM Muxer ----

/// MKV/WebM container muxer — writes EBML-encoded Matroska files.
///
/// Supports writing audio and video streams. Uses a simple
/// "header + clusters" layout. For WebM, use `new_webm()`.
pub struct MkvMuxer<W: Write> {
    writer: W,
    config: MuxConfig,
    video_config: Option<VideoMuxConfig>,
    is_webm: bool,
    timecode_scale: u64,
    cluster_timecode: u64,
    packets_in_cluster: u32,
    header_written: bool,
    total_packets: u64,
}

impl<W: Write> MkvMuxer<W> {
    /// Create a new MKV muxer for audio-only streams.
    pub fn new(writer: W, config: MuxConfig, webm: bool) -> Self {
        Self {
            writer,
            config,
            video_config: None,
            is_webm: webm,
            timecode_scale: 1_000_000, // 1ms
            cluster_timecode: 0,
            packets_in_cluster: 0,
            header_written: false,
            total_packets: 0,
        }
    }

    /// Create a new WebM muxer for audio+video streams (Opus/VP9).
    pub fn new_webm(writer: W, audio: MuxConfig, video: VideoMuxConfig) -> Self {
        Self {
            writer,
            config: audio,
            video_config: Some(video),
            is_webm: true,
            timecode_scale: 1_000_000,
            cluster_timecode: 0,
            packets_in_cluster: 0,
            header_written: false,
            total_packets: 0,
        }
    }

    /// Write a video packet (track 2). Must be called after `write_header()`.
    pub fn write_video_packet(&mut self, data: &[u8]) -> Result<()> {
        use crate::demux::ebml;

        if !self.header_written {
            return Err(TarangError::MuxError("header not written".into()));
        }
        if self.video_config.is_none() {
            return Err(TarangError::MuxError(
                "no video track configured — use new_webm()".into(),
            ));
        }

        // Write SimpleBlock for track 2 (video)
        let mut block = Vec::new();
        ebml::write_vint(&mut block, 2); // track number 2
        block.extend_from_slice(&0i16.to_be_bytes()); // relative timecode
        block.push(0x80); // flags: keyframe
        block.extend_from_slice(data);

        let mut block_buf = Vec::new();
        ebml::write_id(&mut block_buf, 0xA3); // SimpleBlock
        ebml::write_vint(&mut block_buf, block.len() as u64);
        block_buf.extend_from_slice(&block);
        self.writer.write_all(&block_buf).map_err(io_err)?;

        self.packets_in_cluster += 1;
        self.total_packets += 1;
        Ok(())
    }
}

impl<W: Write> Muxer for MkvMuxer<W> {
    fn write_header(&mut self) -> Result<()> {
        use crate::demux::ebml;

        // EBML Header
        let mut ebml_header = Vec::new();
        ebml::write_uint(&mut ebml_header, 0x4286, 1); // EBMLVersion
        ebml::write_uint(&mut ebml_header, 0x42F7, 1); // EBMLReadVersion
        ebml::write_uint(&mut ebml_header, 0x42F2, 4); // EBMLMaxIDLength
        ebml::write_uint(&mut ebml_header, 0x42F3, 8); // EBMLMaxSizeLength
        let doc_type = if self.is_webm { "webm" } else { "matroska" };
        ebml::write_string(&mut ebml_header, 0x4282, doc_type);
        ebml::write_uint(&mut ebml_header, 0x4287, 4); // DocTypeVersion
        ebml::write_uint(&mut ebml_header, 0x4285, 2); // DocTypeReadVersion

        ebml::write_master_to_writer(&mut self.writer, 0x1A45DFA3, &ebml_header).map_err(io_err)?;

        // Segment (unknown size — 0xFF... means "until EOF")
        ebml::write_id_to_writer(&mut self.writer, 0x18538067).map_err(io_err)?;
        // Unknown size marker: 0x01FFFFFFFFFFFFFF (8 bytes)
        self.writer
            .write_all(&[0x01, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF])
            .map_err(io_err)?;

        // Info
        let mut info = Vec::new();
        ebml::write_uint(&mut info, 0x2AD7B1, self.timecode_scale);
        let mut info_buf = Vec::new();
        ebml::write_master(&mut info_buf, 0x1549A966, &info);
        self.writer.write_all(&info_buf).map_err(io_err)?;

        // Tracks
        let mut tracks = Vec::new();
        let mut track_entry = Vec::new();
        ebml::write_uint(&mut track_entry, 0xD7, 1); // TrackNumber
        ebml::write_uint(&mut track_entry, 0x73C5, 1); // TrackUID
        ebml::write_uint(&mut track_entry, 0x83, 2); // TrackType = audio

        let codec_id = match self.config.codec {
            AudioCodec::Opus => "A_OPUS",
            AudioCodec::Vorbis => "A_VORBIS",
            AudioCodec::Flac => "A_FLAC",
            AudioCodec::Aac => "A_AAC",
            AudioCodec::Mp3 => "A_MPEG/L3",
            _ => "A_PCM/INT/LIT",
        };
        ebml::write_string(&mut track_entry, 0x86, codec_id);

        let mut audio = Vec::new();
        ebml::write_float(&mut audio, 0xB5, self.config.sample_rate as f64);
        ebml::write_uint(&mut audio, 0x9F, self.config.channels as u64);
        if self.config.bits_per_sample > 0 {
            ebml::write_uint(&mut audio, 0x6264, self.config.bits_per_sample as u64);
        }
        ebml::write_master(&mut track_entry, 0xE1, &audio);

        ebml::write_master(&mut tracks, 0xAE, &track_entry);

        // Video track (track 2) if configured
        if let Some(ref video) = self.video_config {
            let mut vtrack = Vec::new();
            ebml::write_uint(&mut vtrack, 0xD7, 2); // TrackNumber
            ebml::write_uint(&mut vtrack, 0x73C5, 2); // TrackUID
            ebml::write_uint(&mut vtrack, 0x83, 1); // TrackType = video

            let vid_codec_id = match video.codec {
                crate::core::VideoCodec::Vp8 => "V_VP8",
                crate::core::VideoCodec::Vp9 => "V_VP9",
                crate::core::VideoCodec::Av1 => "V_AV1",
                crate::core::VideoCodec::H264 => "V_MPEG4/ISO/AVC",
                crate::core::VideoCodec::H265 => "V_MPEGH/ISO/HEVC",
                _ => "V_UNCOMPRESSED",
            };
            ebml::write_string(&mut vtrack, 0x86, vid_codec_id);

            let mut video_elem = Vec::new();
            ebml::write_uint(&mut video_elem, 0xB0, video.width as u64); // PixelWidth
            ebml::write_uint(&mut video_elem, 0xBA, video.height as u64); // PixelHeight
            ebml::write_master(&mut vtrack, 0xE0, &video_elem); // Video element

            ebml::write_master(&mut tracks, 0xAE, &vtrack);
        }

        let mut tracks_buf = Vec::new();
        ebml::write_master(&mut tracks_buf, 0x1654AE6B, &tracks);
        self.writer.write_all(&tracks_buf).map_err(io_err)?;

        // Start first cluster
        self.start_cluster(0)?;

        self.header_written = true;
        Ok(())
    }

    fn write_packet(&mut self, data: &[u8]) -> Result<()> {
        use crate::demux::ebml;

        if !self.header_written {
            return Err(TarangError::MuxError("header not written".into()));
        }

        // Write SimpleBlock
        let mut block = Vec::new();
        ebml::write_vint(&mut block, 1); // track number
        block.extend_from_slice(&0i16.to_be_bytes()); // relative timecode
        block.push(0x80); // flags: keyframe
        block.extend_from_slice(data);

        let mut block_buf = Vec::new();
        ebml::write_id(&mut block_buf, 0xA3); // SimpleBlock
        ebml::write_vint(&mut block_buf, block.len() as u64);
        block_buf.extend_from_slice(&block);
        self.writer.write_all(&block_buf).map_err(io_err)?;

        self.packets_in_cluster += 1;
        self.total_packets += 1;

        Ok(())
    }

    fn finalize(&mut self) -> Result<()> {
        self.writer.flush().map_err(io_err)?;
        Ok(())
    }
}

impl<W: Write> MkvMuxer<W> {
    fn start_cluster(&mut self, timecode: u64) -> Result<()> {
        use crate::demux::ebml;

        // Write Cluster element with unknown size
        ebml::write_id_to_writer(&mut self.writer, 0x1F43B675).map_err(io_err)?;
        self.writer
            .write_all(&[0x01, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF])
            .map_err(io_err)?;

        // Timecode element
        let mut tc_buf = Vec::new();
        ebml::write_uint(&mut tc_buf, 0xE7, timecode);
        self.writer.write_all(&tc_buf).map_err(io_err)?;

        self.cluster_timecode = timecode;
        self.packets_in_cluster = 0;
        Ok(())
    }
}

fn io_err(e: std::io::Error) -> TarangError {
    TarangError::DemuxError(format!("mux write error: {e}").into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::demux::Demuxer;
    use std::io::Cursor;

    #[test]
    fn wav_muxer_roundtrip() {
        let mut buf = Cursor::new(Vec::new());
        let config = MuxConfig {
            codec: AudioCodec::Pcm,
            sample_rate: 44100,
            channels: 2,
            bits_per_sample: 16,
        };

        let mut mux = WavMuxer::new(&mut buf, config);
        mux.write_header().unwrap();

        // Write 100 samples of silence (2 channels, 16-bit = 400 bytes)
        let pcm_data = vec![0u8; 400];
        mux.write_packet(&pcm_data).unwrap();
        mux.finalize().unwrap();

        let output = buf.into_inner();

        // Verify RIFF header
        assert_eq!(&output[0..4], b"RIFF");
        assert_eq!(&output[8..12], b"WAVE");
        assert_eq!(&output[12..16], b"fmt ");
        assert_eq!(&output[36..40], b"data");

        // Verify data size was patched
        let data_size = u32::from_le_bytes(output[40..44].try_into().unwrap());
        assert_eq!(data_size, 400);

        // Verify RIFF size was patched
        let riff_size = u32::from_le_bytes(output[4..8].try_into().unwrap());
        assert_eq!(riff_size, 36 + 400);
    }

    #[test]
    fn wav_muxer_demuxer_roundtrip() {
        // Write a WAV, then read it back with WavDemuxer
        let mut buf = Cursor::new(Vec::new());
        let config = MuxConfig {
            codec: AudioCodec::Pcm,
            sample_rate: 48000,
            channels: 1,
            bits_per_sample: 16,
        };

        let mut mux = WavMuxer::new(&mut buf, config);
        mux.write_header().unwrap();

        let pcm_data = vec![0x42u8; 960]; // 480 samples mono 16-bit
        mux.write_packet(&pcm_data).unwrap();
        mux.finalize().unwrap();

        // Now demux it
        let data = buf.into_inner();
        let cursor = Cursor::new(data);
        let mut demuxer = crate::demux::WavDemuxer::new(cursor);
        let info = demuxer.probe().unwrap();

        assert_eq!(info.format, crate::core::ContainerFormat::Wav);
        let audio = info.audio_streams().collect::<Vec<_>>();
        assert_eq!(audio[0].sample_rate, 48000);
        assert_eq!(audio[0].channels, 1);

        let packet = demuxer.next_packet().unwrap();
        assert_eq!(packet.data.len(), 960);
    }

    #[test]
    fn wav_muxer_write_before_header() {
        let mut buf = Cursor::new(Vec::new());
        let config = MuxConfig {
            codec: AudioCodec::Pcm,
            sample_rate: 44100,
            channels: 2,
            bits_per_sample: 16,
        };
        let mut mux = WavMuxer::new(&mut buf, config);
        assert!(mux.write_packet(&[0u8; 100]).is_err());
    }

    #[test]
    fn ogg_opus_muxer_basic() {
        let mut buf = Cursor::new(Vec::new());
        let config = MuxConfig {
            codec: AudioCodec::Opus,
            sample_rate: 48000,
            channels: 2,
            bits_per_sample: 16,
        };

        let mut mux = OggMuxer::new(&mut buf, config).unwrap();
        mux.write_header().unwrap();

        // Write a fake Opus packet
        let packet = vec![0xFCu8; 64];
        mux.write_packet(&packet).unwrap();
        mux.finalize().unwrap();

        let output = buf.into_inner();

        // Should start with OggS
        assert_eq!(&output[0..4], b"OggS");
        // Should have BOS flag on first page
        assert_eq!(output[5], 0x02);
    }

    #[test]
    fn ogg_opus_muxer_demuxer_roundtrip() {
        let mut buf = Cursor::new(Vec::new());
        let config = MuxConfig {
            codec: AudioCodec::Opus,
            sample_rate: 48000,
            channels: 2,
            bits_per_sample: 16,
        };

        let mut mux = OggMuxer::new(&mut buf, config).unwrap();
        mux.write_header().unwrap();

        let packet = vec![0xFCu8; 64];
        mux.write_packet(&packet).unwrap();
        mux.write_packet(&packet).unwrap();
        mux.finalize().unwrap();

        // Read it back with OggDemuxer
        let data = buf.into_inner();
        let cursor = Cursor::new(data);
        let mut demuxer = crate::demux::OggDemuxer::new(cursor);
        let info = demuxer.probe().unwrap();

        assert_eq!(info.format, crate::core::ContainerFormat::Ogg);
        let audio = info.audio_streams().collect::<Vec<_>>();
        assert_eq!(audio[0].codec, AudioCodec::Opus);
        assert_eq!(audio[0].channels, 2);
    }

    #[test]
    fn ogg_vorbis_muxer_basic() {
        let mut buf = Cursor::new(Vec::new());
        let config = MuxConfig {
            codec: AudioCodec::Vorbis,
            sample_rate: 44100,
            channels: 2,
            bits_per_sample: 16,
        };

        let mut mux = OggMuxer::new(&mut buf, config).unwrap();
        mux.write_header().unwrap();

        let packet = vec![0x42u8; 128];
        mux.write_packet(&packet).unwrap();
        mux.finalize().unwrap();

        let output = buf.into_inner();
        assert_eq!(&output[0..4], b"OggS");
    }

    #[test]
    fn ogg_unsupported_codec() {
        let mut buf = Cursor::new(Vec::new());
        let config = MuxConfig {
            codec: AudioCodec::Mp3,
            sample_rate: 44100,
            channels: 2,
            bits_per_sample: 16,
        };
        // Unsupported codec should fail at construction
        assert!(OggMuxer::new(&mut buf, config).is_err());
    }

    #[test]
    fn mp4_muxer_basic() {
        let mut buf = Cursor::new(Vec::new());
        let config = MuxConfig {
            codec: AudioCodec::Aac,
            sample_rate: 44100,
            channels: 2,
            bits_per_sample: 16,
        };

        let mut mux = Mp4Muxer::new(&mut buf, config);
        mux.write_header().unwrap();

        let packet = vec![0xAAu8; 512];
        mux.write_packet(&packet).unwrap();
        mux.write_packet(&packet).unwrap();
        mux.finalize().unwrap();

        let output = buf.into_inner();

        // Should start with ftyp
        assert_eq!(&output[4..8], b"ftyp");
        assert_eq!(&output[8..12], b"isom");
    }

    #[test]
    fn mp4_muxer_has_moov_and_mdat() {
        let mut buf = Cursor::new(Vec::new());
        let config = MuxConfig {
            codec: AudioCodec::Aac,
            sample_rate: 44100,
            channels: 2,
            bits_per_sample: 16,
        };

        let mut mux = Mp4Muxer::new(&mut buf, config);
        mux.write_header().unwrap();
        mux.write_packet(&[0xBBu8; 256]).unwrap();
        mux.finalize().unwrap();

        let output = buf.into_inner();

        // Scan for mdat and moov boxes
        let mut found_mdat = false;
        let mut found_moov = false;
        let mut pos = 0;
        while pos + 8 <= output.len() {
            let size = u32::from_be_bytes(output[pos..pos + 4].try_into().unwrap()) as usize;
            let btype = &output[pos + 4..pos + 8];
            if btype == b"mdat" {
                found_mdat = true;
            }
            if btype == b"moov" {
                found_moov = true;
            }
            if size == 0 {
                break;
            }
            pos += size;
        }
        assert!(found_mdat, "MP4 should contain mdat box");
        assert!(found_moov, "MP4 should contain moov box");
    }

    #[test]
    fn mp4_muxer_demuxer_roundtrip() {
        let mut buf = Cursor::new(Vec::new());
        let config = MuxConfig {
            codec: AudioCodec::Aac,
            sample_rate: 44100,
            channels: 2,
            bits_per_sample: 16,
        };

        let mut mux = Mp4Muxer::new(&mut buf, config);
        mux.write_header().unwrap();

        for _ in 0..5 {
            mux.write_packet(&[0xCCu8; 128]).unwrap();
        }
        mux.finalize().unwrap();

        // Read it back with Mp4Demuxer
        let data = buf.into_inner();
        let cursor = Cursor::new(data);
        let mut demuxer = crate::demux::Mp4Demuxer::new(cursor);
        let info = demuxer.probe().unwrap();

        assert_eq!(info.format, crate::core::ContainerFormat::Mp4);
        let audio = info.audio_streams().collect::<Vec<_>>();
        assert_eq!(audio.len(), 1);
        assert_eq!(audio[0].codec, AudioCodec::Aac);
        assert_eq!(audio[0].sample_rate, 44100);
        assert_eq!(audio[0].channels, 2);

        // Read all packets back
        let mut count = 0;
        loop {
            match demuxer.next_packet() {
                Ok(p) => {
                    assert_eq!(p.data.len(), 128);
                    count += 1;
                }
                Err(TarangError::EndOfStream) => break,
                Err(e) => panic!("unexpected: {e}"),
            }
        }
        assert_eq!(count, 5);
    }

    #[test]
    fn mp4_muxer_write_before_header() {
        let mut buf = Cursor::new(Vec::new());
        let config = MuxConfig {
            codec: AudioCodec::Aac,
            sample_rate: 44100,
            channels: 2,
            bits_per_sample: 16,
        };
        let mut mux = Mp4Muxer::new(&mut buf, config);
        assert!(mux.write_packet(&[0u8; 100]).is_err());
    }

    #[test]
    fn mkv_muxer_basic() {
        let mut buf = Cursor::new(Vec::new());
        let config = MuxConfig {
            codec: AudioCodec::Opus,
            sample_rate: 48000,
            channels: 2,
            bits_per_sample: 16,
        };

        let mut mux = MkvMuxer::new(&mut buf, config, false);
        mux.write_header().unwrap();
        mux.write_packet(&[0xFFu8; 64]).unwrap();
        mux.finalize().unwrap();

        let output = buf.into_inner();
        // Should start with EBML header ID
        assert_eq!(output[0], 0x1A);
        assert_eq!(output[1], 0x45);
        assert_eq!(output[2], 0xDF);
        assert_eq!(output[3], 0xA3);
    }

    #[test]
    fn mkv_muxer_demuxer_roundtrip() {
        let mut buf = Cursor::new(Vec::new());
        let config = MuxConfig {
            codec: AudioCodec::Opus,
            sample_rate: 48000,
            channels: 2,
            bits_per_sample: 16,
        };

        let mut mux = MkvMuxer::new(&mut buf, config, false);
        mux.write_header().unwrap();
        mux.write_packet(&[0xABu8; 64]).unwrap();
        mux.finalize().unwrap();

        let data = buf.into_inner();
        let cursor = Cursor::new(data);
        let mut demuxer = crate::demux::MkvDemuxer::new(cursor);
        let info = demuxer.probe().unwrap();

        assert_eq!(info.format, crate::core::ContainerFormat::Mkv);
        let audio = info.audio_streams().collect::<Vec<_>>();
        assert_eq!(audio[0].codec, AudioCodec::Opus);
        assert_eq!(audio[0].sample_rate, 48000);
        assert_eq!(audio[0].channels, 2);

        let packet = demuxer.next_packet().unwrap();
        assert_eq!(packet.data.len(), 64);
    }

    // ---- MP4 Muxer regression tests ----

    /// Helper: scan top-level ISOBMFF boxes and return a vec of (offset, size, type).
    fn scan_top_level_boxes(data: &[u8]) -> Vec<(usize, u32, [u8; 4])> {
        let mut boxes = Vec::new();
        let mut pos = 0;
        while pos + 8 <= data.len() {
            let size = u32::from_be_bytes(data[pos..pos + 4].try_into().unwrap());
            let mut btype = [0u8; 4];
            btype.copy_from_slice(&data[pos + 4..pos + 8]);
            boxes.push((pos, size, btype));
            if size == 0 {
                break;
            }
            pos += size as usize;
        }
        boxes
    }

    /// Helper: create a muxed MP4 from packets of given sizes, returning the
    /// raw output bytes.
    fn mux_mp4_packets(packet_sizes: &[usize]) -> Vec<u8> {
        let mut buf = Cursor::new(Vec::new());
        let config = MuxConfig {
            codec: AudioCodec::Aac,
            sample_rate: 44100,
            channels: 2,
            bits_per_sample: 16,
        };
        let mut mux = Mp4Muxer::new(&mut buf, config);
        mux.write_header().unwrap();
        for (i, &sz) in packet_sizes.iter().enumerate() {
            // Fill with a recognizable per-packet byte so we can verify offsets
            let data = vec![(i & 0xFF) as u8; sz];
            mux.write_packet(&data).unwrap();
        }
        mux.finalize().unwrap();
        buf.into_inner()
    }

    #[test]
    fn test_mp4_roundtrip_basic() {
        let packet_sizes: Vec<usize> = vec![128, 256, 64, 512, 100];
        let output = mux_mp4_packets(&packet_sizes);

        // Parse with the MP4 demuxer
        let cursor = Cursor::new(output.clone());
        let mut demuxer = crate::demux::Mp4Demuxer::new(cursor);
        let info = demuxer.probe().unwrap();

        // ftyp present — verified implicitly by successful probe (demuxer
        // checks for ftyp). Also check raw bytes.
        assert_eq!(&output[4..8], b"ftyp");

        // moov present — scan boxes
        let boxes = scan_top_level_boxes(&output);
        assert!(
            boxes.iter().any(|(_, _, t)| t == b"moov"),
            "moov box must be present"
        );

        // Verify track info
        assert_eq!(info.format, crate::core::ContainerFormat::Mp4);
        let audio: Vec<_> = info.audio_streams().collect();
        assert_eq!(audio.len(), 1);
        assert_eq!(audio[0].codec, AudioCodec::Aac);
        assert_eq!(audio[0].sample_rate, 44100);
        assert_eq!(audio[0].channels, 2);

        // Read all packets back and verify count + sizes
        let mut read_sizes = Vec::new();
        loop {
            match demuxer.next_packet() {
                Ok(p) => read_sizes.push(p.data.len()),
                Err(TarangError::EndOfStream) => break,
                Err(e) => panic!("unexpected error: {e}"),
            }
        }
        assert_eq!(
            read_sizes.len(),
            packet_sizes.len(),
            "sample count mismatch"
        );
        assert_eq!(read_sizes, packet_sizes, "sample sizes mismatch");
    }

    #[test]
    fn test_mp4_empty_track() {
        // Finalize without writing any packets — must not panic.
        let mut buf = Cursor::new(Vec::new());
        let config = MuxConfig {
            codec: AudioCodec::Aac,
            sample_rate: 44100,
            channels: 2,
            bits_per_sample: 16,
        };
        let mut mux = Mp4Muxer::new(&mut buf, config);
        mux.write_header().unwrap();
        mux.finalize().unwrap();

        let output = buf.into_inner();

        // Verify ftyp + mdat + moov are all present as top-level boxes
        let boxes = scan_top_level_boxes(&output);
        let types: Vec<[u8; 4]> = boxes.iter().map(|(_, _, t)| *t).collect();
        assert!(types.contains(b"ftyp"), "ftyp must be present");
        assert!(types.contains(b"mdat"), "mdat must be present");
        assert!(types.contains(b"moov"), "moov must be present");

        // mdat should be exactly 8 bytes (header only, no data)
        let mdat_box = boxes.iter().find(|(_, _, t)| t == b"mdat").unwrap();
        assert_eq!(
            mdat_box.1, 8,
            "mdat size should be 8 (header only) for empty track"
        );
    }

    #[test]
    fn test_mp4_single_sample() {
        let output = mux_mp4_packets(&[42]);

        // Roundtrip through the demuxer
        let cursor = Cursor::new(output);
        let mut demuxer = crate::demux::Mp4Demuxer::new(cursor);
        let info = demuxer.probe().unwrap();

        let audio: Vec<_> = info.audio_streams().collect();
        assert_eq!(audio.len(), 1);

        let packet = demuxer.next_packet().unwrap();
        assert_eq!(packet.data.len(), 42);

        // Should be end of stream after the single sample
        match demuxer.next_packet() {
            Err(TarangError::EndOfStream) => {}
            other => panic!("expected EndOfStream, got {other:?}"),
        }
    }

    #[test]
    fn test_mp4_seek_back_patching() {
        let packet_sizes: Vec<usize> = vec![100, 200, 300];
        let total_data: usize = packet_sizes.iter().sum();
        let output = mux_mp4_packets(&packet_sizes);

        // Find the mdat box
        let boxes = scan_top_level_boxes(&output);
        let mdat_box = boxes
            .iter()
            .find(|(_, _, t)| t == b"mdat")
            .expect("mdat must exist");
        let (mdat_offset, mdat_size, _) = *mdat_box;

        // The mdat box size must equal 8 (header) + total data written
        assert_eq!(
            mdat_size as usize,
            8 + total_data,
            "mdat size must be header(8) + data({total_data})"
        );

        // Read the raw mdat header bytes at the mdat offset to double-check
        let raw_size = u32::from_be_bytes(output[mdat_offset..mdat_offset + 4].try_into().unwrap());
        assert_eq!(raw_size as usize, 8 + total_data);
        assert_eq!(&output[mdat_offset + 4..mdat_offset + 8], b"mdat");
    }

    #[test]
    fn test_mp4_stco_offsets() {
        // Write packets of known, varying sizes
        let packet_sizes: Vec<usize> = vec![100, 200, 50, 300, 75];
        let output = mux_mp4_packets(&packet_sizes);

        // The muxer puts all samples in a single chunk. The stco offset
        // should point to mdat_offset + 8 (start of sample data).
        let boxes = scan_top_level_boxes(&output);
        let mdat_box = boxes.iter().find(|(_, _, t)| t == b"mdat").unwrap();
        let mdat_data_start = mdat_box.0 + 8; // past the 8-byte mdat header

        // Parse with the demuxer and verify each packet reads from the right
        // place by checking the actual data content.
        let cursor = Cursor::new(output.clone());
        let mut demuxer = crate::demux::Mp4Demuxer::new(cursor);
        demuxer.probe().unwrap();

        let mut offset_in_mdat = 0usize;
        for (i, &sz) in packet_sizes.iter().enumerate() {
            let packet = demuxer.next_packet().unwrap();
            assert_eq!(packet.data.len(), sz, "packet {i} size mismatch");

            // Verify the data matches what we wrote (each packet filled with
            // (i & 0xFF) by mux_mp4_packets)
            let expected_byte = (i & 0xFF) as u8;
            assert!(
                packet.data.iter().all(|&b| b == expected_byte),
                "packet {i} data content mismatch"
            );

            // Also verify directly in the raw output that the bytes at the
            // expected offset match.
            let abs_offset = mdat_data_start + offset_in_mdat;
            assert_eq!(
                output[abs_offset], expected_byte,
                "raw byte at offset {abs_offset} for packet {i} should be {expected_byte:#x}"
            );
            offset_in_mdat += sz;
        }
    }

    #[test]
    fn webm_muxer_format() {
        let mut buf = Cursor::new(Vec::new());
        let config = MuxConfig {
            codec: AudioCodec::Vorbis,
            sample_rate: 44100,
            channels: 2,
            bits_per_sample: 16,
        };

        let mut mux = MkvMuxer::new(&mut buf, config, true);
        mux.write_header().unwrap();
        mux.write_packet(&[0xCDu8; 32]).unwrap();
        mux.finalize().unwrap();

        let data = buf.into_inner();
        let cursor = Cursor::new(data);
        let mut demuxer = crate::demux::MkvDemuxer::new(cursor);
        let info = demuxer.probe().unwrap();

        assert_eq!(info.format, crate::core::ContainerFormat::WebM);
    }

    #[test]
    fn webm_muxer_with_video_track() {
        let mut buf = Cursor::new(Vec::new());
        let audio = MuxConfig {
            codec: AudioCodec::Opus,
            sample_rate: 48000,
            channels: 2,
            bits_per_sample: 0,
        };
        let video = VideoMuxConfig {
            codec: crate::core::VideoCodec::Vp9,
            width: 1920,
            height: 1080,
        };

        let mut mux = MkvMuxer::new_webm(&mut buf, audio, video);
        mux.write_header().unwrap();

        // Write an audio packet (track 1)
        mux.write_packet(&[0xAA; 100]).unwrap();
        // Write a video packet (track 2)
        mux.write_video_packet(&[0xBB; 500]).unwrap();
        mux.finalize().unwrap();

        let data = buf.into_inner();
        // Should start with EBML header
        assert_eq!(&data[0..4], &[0x1A, 0x45, 0xDF, 0xA3]);
        // Should contain "webm" DocType
        assert!(data.windows(4).any(|w| w == b"webm"));
        // Should contain VP9 codec ID
        assert!(data.windows(5).any(|w| w == b"V_VP9"));
        // Should contain Opus codec ID
        assert!(data.windows(6).any(|w| w == b"A_OPUS"));
    }

    #[test]
    fn webm_video_packet_without_config_errors() {
        let mut buf = Cursor::new(Vec::new());
        let config = MuxConfig {
            codec: AudioCodec::Opus,
            sample_rate: 48000,
            channels: 2,
            bits_per_sample: 0,
        };
        let mut mux = MkvMuxer::new(&mut buf, config, true);
        mux.write_header().unwrap();

        // Writing video to an audio-only muxer should error
        assert!(mux.write_video_packet(&[0x00; 100]).is_err());
    }

    // ---- MP4 video muxer tests ----

    #[test]
    fn mp4_muxer_with_video_creates_valid_mp4() {
        let mut buf = Cursor::new(Vec::new());
        let audio = MuxConfig {
            codec: AudioCodec::Aac,
            sample_rate: 44100,
            channels: 2,
            bits_per_sample: 16,
        };
        let video = VideoMuxConfig {
            codec: crate::core::VideoCodec::H264,
            width: 1920,
            height: 1080,
        };

        let mut mux = Mp4Muxer::new_with_video(&mut buf, audio, video);
        mux.write_header().unwrap();

        // Write audio samples
        mux.write_packet(&[0xAA; 256]).unwrap();
        mux.write_packet(&[0xAA; 256]).unwrap();

        // Write video samples
        mux.write_video_packet(&[0xBB; 1024]).unwrap();
        mux.write_video_packet(&[0xBB; 2048]).unwrap();

        mux.finalize().unwrap();

        let data = buf.into_inner();
        // Should start with ftyp box
        assert_eq!(&data[4..8], b"ftyp");
        // Should contain moov
        assert!(data.windows(4).any(|w| w == b"moov"));
        // Should contain two trak boxes (audio + video)
        let trak_count = data.windows(4).filter(|w| *w == b"trak").count();
        assert_eq!(trak_count, 2, "should have 2 trak boxes (audio + video)");
        // Should contain vide handler
        assert!(data.windows(4).any(|w| w == b"vide"));
        // Should contain avc1 sample entry
        assert!(data.windows(4).any(|w| w == b"avc1"));
        // Should contain vmhd (video media header)
        assert!(data.windows(4).any(|w| w == b"vmhd"));
    }

    #[test]
    fn mp4_video_packet_without_config_errors() {
        let mut buf = Cursor::new(Vec::new());
        let config = MuxConfig {
            codec: AudioCodec::Aac,
            sample_rate: 44100,
            channels: 2,
            bits_per_sample: 16,
        };
        let mut mux = Mp4Muxer::new(&mut buf, config);
        mux.write_header().unwrap();
        assert!(mux.write_video_packet(&[0x00; 100]).is_err());
    }

    #[test]
    fn mp4_video_codec_ids() {
        // Verify different codec types produce correct box types
        for (codec, expected_box) in [
            (crate::core::VideoCodec::H264, b"avc1"),
            (crate::core::VideoCodec::H265, b"hev1"),
            (crate::core::VideoCodec::Vp9, b"vp09"),
            (crate::core::VideoCodec::Av1, b"av01"),
        ] {
            let mut buf = Cursor::new(Vec::new());
            let audio = MuxConfig {
                codec: AudioCodec::Aac,
                sample_rate: 44100,
                channels: 2,
                bits_per_sample: 16,
            };
            let video = VideoMuxConfig {
                codec,
                width: 640,
                height: 480,
            };

            let mut mux = Mp4Muxer::new_with_video(&mut buf, audio, video);
            mux.write_header().unwrap();
            mux.write_video_packet(&[0x00; 100]).unwrap();
            mux.finalize().unwrap();

            let data = buf.into_inner();
            assert!(
                data.windows(4).any(|w| w == expected_box),
                "{codec} should produce {} box",
                std::str::from_utf8(expected_box).unwrap()
            );
        }
    }

    // ---- Fragmented MP4 / DASH tests ----

    #[test]
    fn fmp4_init_segment() {
        let mut buf = Cursor::new(Vec::new());
        let config = MuxConfig {
            codec: AudioCodec::Aac,
            sample_rate: 44100,
            channels: 2,
            bits_per_sample: 16,
        };
        let mut mux = FragmentedMp4Muxer::new(&mut buf, config);
        mux.write_init_segment().unwrap();

        let data = buf.into_inner();
        assert_eq!(&data[4..8], b"ftyp");
        assert!(data.windows(4).any(|w| w == b"moov"));
        assert!(data.windows(4).any(|w| w == b"mvex")); // movie extends
        assert!(data.windows(4).any(|w| w == b"trex")); // track extends
    }

    #[test]
    fn fmp4_fragment_produces_moof_mdat() {
        let mut buf = Cursor::new(Vec::new());
        let config = MuxConfig {
            codec: AudioCodec::Aac,
            sample_rate: 44100,
            channels: 2,
            bits_per_sample: 16,
        };
        let mut mux = FragmentedMp4Muxer::new(&mut buf, config);
        mux.write_init_segment().unwrap();

        // Add samples and flush a fragment
        mux.add_audio_sample(&[0xAA; 256]).unwrap();
        mux.add_audio_sample(&[0xBB; 128]).unwrap();
        let count = mux.flush_fragment().unwrap();
        assert_eq!(count, 2);

        let data = buf.into_inner();
        assert!(data.windows(4).any(|w| w == b"moof"));
        assert!(data.windows(4).any(|w| w == b"mfhd")); // fragment header
        assert!(data.windows(4).any(|w| w == b"traf")); // track fragment
        assert!(data.windows(4).any(|w| w == b"tfhd"));
        assert!(data.windows(4).any(|w| w == b"trun")); // track run
        assert!(data.windows(4).any(|w| w == b"mdat"));
    }

    #[test]
    fn fmp4_multiple_fragments() {
        let mut buf = Cursor::new(Vec::new());
        let config = MuxConfig {
            codec: AudioCodec::Opus,
            sample_rate: 48000,
            channels: 2,
            bits_per_sample: 0,
        };
        let mut mux = FragmentedMp4Muxer::new(&mut buf, config);
        mux.write_init_segment().unwrap();

        // Fragment 1
        mux.add_audio_sample(&[0x01; 100]).unwrap();
        mux.flush_fragment().unwrap();

        // Fragment 2
        mux.add_audio_sample(&[0x02; 200]).unwrap();
        mux.add_audio_sample(&[0x03; 150]).unwrap();
        mux.flush_fragment().unwrap();

        let data = buf.into_inner();
        // Should have 2 moof boxes
        let moof_count = data.windows(4).filter(|w| *w == b"moof").count();
        assert_eq!(moof_count, 2);
    }

    #[test]
    fn fmp4_empty_fragment_returns_zero() {
        let mut buf = Cursor::new(Vec::new());
        let config = MuxConfig {
            codec: AudioCodec::Aac,
            sample_rate: 44100,
            channels: 2,
            bits_per_sample: 16,
        };
        let mut mux = FragmentedMp4Muxer::new(&mut buf, config);
        mux.write_init_segment().unwrap();

        let count = mux.flush_fragment().unwrap();
        assert_eq!(count, 0);
    }

    #[test]
    fn fmp4_add_sample_before_init_errors() {
        let mut buf = Cursor::new(Vec::new());
        let config = MuxConfig {
            codec: AudioCodec::Aac,
            sample_rate: 44100,
            channels: 2,
            bits_per_sample: 16,
        };
        let mut mux = FragmentedMp4Muxer::new(&mut buf, config);
        assert!(mux.add_audio_sample(&[0x00; 100]).is_err());
    }
}
