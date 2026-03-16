//! Container muxing — writing audio data into container formats
//!
//! Provides muxers that write encoded audio packets into container files.
//! Currently supports WAV and OGG containers.

use std::io::{Seek, Write};
use tarang_core::{AudioCodec, Result, TarangError};

/// Trait for container muxers (writers)
pub trait Muxer {
    /// Write the container header / initialize the stream.
    fn write_header(&mut self) -> Result<()>;

    /// Write a packet of encoded audio data.
    fn write_packet(&mut self, data: &[u8]) -> Result<()>;

    /// Finalize the container (write trailing metadata, fix headers, etc.)
    fn finalize(&mut self) -> Result<()>;
}

/// Configuration for a mux stream
#[derive(Debug, Clone)]
pub struct MuxConfig {
    pub codec: AudioCodec,
    pub sample_rate: u32,
    pub channels: u16,
    pub bits_per_sample: u16,
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
        self.writer
            .write_all(&1u16.to_le_bytes())
            .map_err(io_err)?; // PCM format
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
            return Err(TarangError::Pipeline("header not written".to_string()));
        }
        self.writer.write_all(data).map_err(io_err)?;
        self.data_bytes_written += data.len() as u32;
        Ok(())
    }

    fn finalize(&mut self) -> Result<()> {
        // Patch RIFF size (file_size - 8)
        let riff_size = 36 + self.data_bytes_written;
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
    pub fn new(writer: W, config: MuxConfig) -> Self {
        // Use a fixed serial for now; could be randomized
        Self {
            writer,
            config,
            serial: 0x74617267, // "targ" in hex
            page_sequence: 0,
            granule_position: 0,
            header_written: false,
        }
    }

    /// Write a single OGG page containing the given packets.
    fn write_page(
        &mut self,
        header_type: u8,
        granule: i64,
        packets: &[&[u8]],
    ) -> Result<()> {
        // Build segment table
        let mut segment_table = Vec::new();
        for packet in packets {
            let len = packet.len();
            let full_segments = len / 255;
            let remainder = len % 255;
            for _ in 0..full_segments {
                segment_table.push(255u8);
            }
            segment_table.push(remainder as u8);
        }

        // Page header
        self.writer.write_all(b"OggS").map_err(io_err)?;
        self.writer.write_all(&[0u8]).map_err(io_err)?; // version
        self.writer.write_all(&[header_type]).map_err(io_err)?;
        self.writer
            .write_all(&granule.to_le_bytes())
            .map_err(io_err)?;
        self.writer
            .write_all(&self.serial.to_le_bytes())
            .map_err(io_err)?;
        self.writer
            .write_all(&self.page_sequence.to_le_bytes())
            .map_err(io_err)?;
        self.writer
            .write_all(&0u32.to_le_bytes())
            .map_err(io_err)?; // CRC (0 for now)
        self.writer
            .write_all(&[segment_table.len() as u8])
            .map_err(io_err)?;
        self.writer.write_all(&segment_table).map_err(io_err)?;

        // Page body
        for packet in packets {
            self.writer.write_all(packet).map_err(io_err)?;
        }

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
                return Err(TarangError::UnsupportedCodec(format!(
                    "OGG muxer does not support {}",
                    self.config.codec
                )));
            }
        }

        self.header_written = true;
        Ok(())
    }

    fn write_packet(&mut self, data: &[u8]) -> Result<()> {
        if !self.header_written {
            return Err(TarangError::Pipeline("header not written".to_string()));
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

/// MP4/M4A container muxer — writes ISOBMFF boxes for audio-only MP4 files.
///
/// Accumulates sample data and metadata, then writes the full file on `finalize()`
/// (moov-at-end strategy, simple and correct).
pub struct Mp4Muxer<W: Write + Seek> {
    writer: W,
    config: MuxConfig,
    /// Collected encoded sample data
    samples: Vec<Vec<u8>>,
    /// Per-sample sizes for stsz
    sample_sizes: Vec<u32>,
    /// Sample delta for stts (constant for audio)
    sample_delta: u32,
    header_written: bool,
}

impl<W: Write + Seek> Mp4Muxer<W> {
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
            samples: Vec::new(),
            sample_sizes: Vec::new(),
            sample_delta,
            header_written: false,
        }
    }

    fn write_box(&mut self, box_type: &[u8; 4], data: &[u8]) -> Result<()> {
        let size = (8 + data.len()) as u32;
        self.writer
            .write_all(&size.to_be_bytes())
            .map_err(io_err)?;
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

    fn build_moov(&self, mdat_offset: u64) -> Vec<u8> {
        let mut moov = Vec::new();

        // mvhd
        let mvhd = self.build_mvhd();
        write_sub_box(&mut moov, b"mvhd", &mvhd);

        // trak
        let trak = self.build_trak(mdat_offset);
        write_sub_box(&mut moov, b"trak", &trak);

        moov
    }

    fn build_mvhd(&self) -> Vec<u8> {
        let num_samples = self.samples.len() as u64;
        let timescale = self.config.sample_rate;
        let duration = num_samples * self.sample_delta as u64;

        let mut buf = Vec::new();
        buf.extend_from_slice(&0u32.to_be_bytes()); // version + flags
        buf.extend_from_slice(&0u32.to_be_bytes()); // creation_time
        buf.extend_from_slice(&0u32.to_be_bytes()); // modification_time
        buf.extend_from_slice(&timescale.to_be_bytes());
        buf.extend_from_slice(&(duration as u32).to_be_bytes());
        buf.extend_from_slice(&0x00010000u32.to_be_bytes()); // rate = 1.0
        buf.extend_from_slice(&0x0100u16.to_be_bytes()); // volume = 1.0
        buf.extend_from_slice(&[0u8; 10]); // reserved
        // Matrix (identity)
        for &v in &[
            0x00010000u32, 0, 0, 0, 0x00010000, 0, 0, 0, 0x40000000,
        ] {
            buf.extend_from_slice(&v.to_be_bytes());
        }
        buf.extend_from_slice(&[0u8; 24]); // pre_defined
        buf.extend_from_slice(&2u32.to_be_bytes()); // next_track_id
        buf
    }

    fn build_trak(&self, mdat_offset: u64) -> Vec<u8> {
        let mut trak = Vec::new();

        let tkhd = self.build_tkhd();
        write_sub_box(&mut trak, b"tkhd", &tkhd);

        let mdia = self.build_mdia(mdat_offset);
        write_sub_box(&mut trak, b"mdia", &mdia);

        trak
    }

    fn build_tkhd(&self) -> Vec<u8> {
        let num_samples = self.samples.len() as u64;
        let duration = num_samples * self.sample_delta as u64;

        let mut buf = Vec::new();
        buf.extend_from_slice(&0x00000003u32.to_be_bytes()); // version 0 + flags (enabled+in_movie)
        buf.extend_from_slice(&0u32.to_be_bytes()); // creation_time
        buf.extend_from_slice(&0u32.to_be_bytes()); // modification_time
        buf.extend_from_slice(&1u32.to_be_bytes()); // track_id
        buf.extend_from_slice(&0u32.to_be_bytes()); // reserved
        buf.extend_from_slice(&(duration as u32).to_be_bytes());
        buf.extend_from_slice(&[0u8; 8]); // reserved
        buf.extend_from_slice(&0u16.to_be_bytes()); // layer
        buf.extend_from_slice(&0u16.to_be_bytes()); // alternate_group
        buf.extend_from_slice(&0x0100u16.to_be_bytes()); // volume = 1.0
        buf.extend_from_slice(&0u16.to_be_bytes()); // reserved
        // Matrix (identity)
        for &v in &[
            0x00010000u32, 0, 0, 0, 0x00010000, 0, 0, 0, 0x40000000,
        ] {
            buf.extend_from_slice(&v.to_be_bytes());
        }
        buf.extend_from_slice(&0u32.to_be_bytes()); // width
        buf.extend_from_slice(&0u32.to_be_bytes()); // height
        buf
    }

    fn build_mdia(&self, mdat_offset: u64) -> Vec<u8> {
        let mut mdia = Vec::new();

        let mdhd = self.build_mdhd();
        write_sub_box(&mut mdia, b"mdhd", &mdhd);

        let hdlr = self.build_hdlr();
        write_sub_box(&mut mdia, b"hdlr", &hdlr);

        let minf = self.build_minf(mdat_offset);
        write_sub_box(&mut mdia, b"minf", &minf);

        mdia
    }

    fn build_mdhd(&self) -> Vec<u8> {
        let num_samples = self.samples.len() as u64;
        let timescale = self.config.sample_rate;
        let duration = num_samples * self.sample_delta as u64;

        let mut buf = Vec::new();
        buf.extend_from_slice(&0u32.to_be_bytes()); // version + flags
        buf.extend_from_slice(&0u32.to_be_bytes()); // creation_time
        buf.extend_from_slice(&0u32.to_be_bytes()); // modification_time
        buf.extend_from_slice(&timescale.to_be_bytes());
        buf.extend_from_slice(&(duration as u32).to_be_bytes());
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

    fn build_minf(&self, mdat_offset: u64) -> Vec<u8> {
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

        let stbl = self.build_stbl(mdat_offset);
        write_sub_box(&mut minf, b"stbl", &stbl);

        minf
    }

    fn build_stbl(&self, mdat_offset: u64) -> Vec<u8> {
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
        let stco = self.build_stco(data_start);
        write_sub_box(&mut stbl, b"stco", &stco);

        stbl
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
        buf.extend_from_slice(&(self.samples.len() as u32).to_be_bytes());
        buf.extend_from_slice(&self.sample_delta.to_be_bytes());
        buf
    }

    fn build_stsc(&self) -> Vec<u8> {
        let mut buf = Vec::new();
        buf.extend_from_slice(&0u32.to_be_bytes()); // version + flags
        buf.extend_from_slice(&1u32.to_be_bytes()); // entry_count
        buf.extend_from_slice(&1u32.to_be_bytes()); // first_chunk
        buf.extend_from_slice(&(self.samples.len() as u32).to_be_bytes()); // samples_per_chunk
        buf.extend_from_slice(&1u32.to_be_bytes()); // sample_description_index
        buf
    }

    fn build_stsz(&self) -> Vec<u8> {
        let mut buf = Vec::new();
        buf.extend_from_slice(&0u32.to_be_bytes()); // version + flags

        // Check if all samples are the same size
        let all_same = self
            .sample_sizes
            .windows(2)
            .all(|w| w[0] == w[1]);

        if all_same && !self.sample_sizes.is_empty() {
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
}

impl<W: Write + Seek> Muxer for Mp4Muxer<W> {
    fn write_header(&mut self) -> Result<()> {
        // Write ftyp immediately
        let ftyp = self.build_ftyp();
        self.write_box(b"ftyp", &ftyp)?;
        self.header_written = true;
        Ok(())
    }

    fn write_packet(&mut self, data: &[u8]) -> Result<()> {
        if !self.header_written {
            return Err(TarangError::Pipeline("header not written".to_string()));
        }
        self.sample_sizes.push(data.len() as u32);
        self.samples.push(data.to_vec());
        Ok(())
    }

    fn finalize(&mut self) -> Result<()> {
        // Write mdat box
        let total_data: usize = self.samples.iter().map(|s| s.len()).sum();
        let mdat_size = (8 + total_data) as u32;

        let mdat_offset = self
            .writer
            .stream_position()
            .map_err(io_err)?;

        self.writer
            .write_all(&mdat_size.to_be_bytes())
            .map_err(io_err)?;
        self.writer.write_all(b"mdat").map_err(io_err)?;
        for sample in &self.samples {
            self.writer.write_all(sample).map_err(io_err)?;
        }

        // Write moov box
        let moov_data = self.build_moov(mdat_offset);
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

// ---- MKV/WebM Muxer ----

/// MKV/WebM container muxer — writes EBML-encoded Matroska files.
///
/// Supports writing audio and video streams. Uses a simple
/// "header + clusters" layout.
pub struct MkvMuxer<W: Write> {
    writer: W,
    config: MuxConfig,
    is_webm: bool,
    timecode_scale: u64,
    cluster_timecode: u64,
    packets_in_cluster: u32,
    max_cluster_packets: u32,
    header_written: bool,
    total_packets: u64,
}

impl<W: Write> MkvMuxer<W> {
    pub fn new(writer: W, config: MuxConfig, webm: bool) -> Self {
        Self {
            writer,
            config,
            is_webm: webm,
            timecode_scale: 1_000_000, // 1ms
            cluster_timecode: 0,
            packets_in_cluster: 0,
            max_cluster_packets: 256,
            header_written: false,
            total_packets: 0,
        }
    }
}

impl<W: Write> Muxer for MkvMuxer<W> {
    fn write_header(&mut self) -> Result<()> {
        // EBML Header
        let mut ebml_header = Vec::new();
        ebml_write_uint(&mut ebml_header, 0x4286, 1); // EBMLVersion
        ebml_write_uint(&mut ebml_header, 0x42F7, 1); // EBMLReadVersion
        ebml_write_uint(&mut ebml_header, 0x42F2, 4); // EBMLMaxIDLength
        ebml_write_uint(&mut ebml_header, 0x42F3, 8); // EBMLMaxSizeLength
        let doc_type = if self.is_webm { "webm" } else { "matroska" };
        ebml_write_string(&mut ebml_header, 0x4282, doc_type);
        ebml_write_uint(&mut ebml_header, 0x4287, 4); // DocTypeVersion
        ebml_write_uint(&mut ebml_header, 0x4285, 2); // DocTypeReadVersion

        ebml_write_master(&mut self.writer, 0x1A45DFA3, &ebml_header).map_err(io_err)?;

        // Segment (unknown size — 0xFF... means "until EOF")
        ebml_write_id(&mut self.writer, 0x18538067).map_err(io_err)?;
        // Unknown size marker: 0x01FFFFFFFFFFFFFF (8 bytes)
        self.writer
            .write_all(&[0x01, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF])
            .map_err(io_err)?;

        // Info
        let mut info = Vec::new();
        ebml_write_uint(&mut info, 0x2AD7B1, self.timecode_scale);
        let mut info_buf = Vec::new();
        ebml_write_master_to_buf(&mut info_buf, 0x1549A966, &info);
        self.writer.write_all(&info_buf).map_err(io_err)?;

        // Tracks
        let mut tracks = Vec::new();
        let mut track_entry = Vec::new();
        ebml_write_uint(&mut track_entry, 0xD7, 1); // TrackNumber
        ebml_write_uint(&mut track_entry, 0x73C5, 1); // TrackUID
        ebml_write_uint(&mut track_entry, 0x83, 2); // TrackType = audio

        let codec_id = match self.config.codec {
            AudioCodec::Opus => "A_OPUS",
            AudioCodec::Vorbis => "A_VORBIS",
            AudioCodec::Flac => "A_FLAC",
            AudioCodec::Aac => "A_AAC",
            AudioCodec::Mp3 => "A_MPEG/L3",
            _ => "A_PCM/INT/LIT",
        };
        ebml_write_string(&mut track_entry, 0x86, codec_id);

        let mut audio = Vec::new();
        ebml_write_float(&mut audio, 0xB5, self.config.sample_rate as f64);
        ebml_write_uint(&mut audio, 0x9F, self.config.channels as u64);
        if self.config.bits_per_sample > 0 {
            ebml_write_uint(&mut audio, 0x6264, self.config.bits_per_sample as u64);
        }
        ebml_write_master_to_buf(&mut track_entry, 0xE1, &audio);

        ebml_write_master_to_buf(&mut tracks, 0xAE, &track_entry);
        let mut tracks_buf = Vec::new();
        ebml_write_master_to_buf(&mut tracks_buf, 0x1654AE6B, &tracks);
        self.writer.write_all(&tracks_buf).map_err(io_err)?;

        // Start first cluster
        self.start_cluster(0)?;

        self.header_written = true;
        Ok(())
    }

    fn write_packet(&mut self, data: &[u8]) -> Result<()> {
        if !self.header_written {
            return Err(TarangError::Pipeline("header not written".to_string()));
        }

        // Write SimpleBlock
        let mut block = Vec::new();
        ebml_write_vint(&mut block, 1); // track number
        block.extend_from_slice(&0i16.to_be_bytes()); // relative timecode
        block.push(0x80); // flags: keyframe
        block.extend_from_slice(data);

        let mut block_buf = Vec::new();
        ebml_write_id_to_buf(&mut block_buf, 0xA3); // SimpleBlock
        ebml_write_vint_to_buf(&mut block_buf, block.len() as u64);
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
        // Write Cluster element with unknown size
        ebml_write_id(&mut self.writer, 0x1F43B675).map_err(io_err)?;
        self.writer
            .write_all(&[0x01, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF])
            .map_err(io_err)?;

        // Timecode element
        let mut tc_buf = Vec::new();
        ebml_write_uint(&mut tc_buf, 0xE7, timecode);
        self.writer.write_all(&tc_buf).map_err(io_err)?;

        self.cluster_timecode = timecode;
        self.packets_in_cluster = 0;
        Ok(())
    }
}

// ---- EBML writing helpers ----

fn ebml_write_id(w: &mut dyn Write, id: u32) -> std::io::Result<()> {
    let mut buf = Vec::new();
    ebml_write_id_to_buf(&mut buf, id);
    w.write_all(&buf)
}

fn ebml_write_id_to_buf(buf: &mut Vec<u8>, id: u32) {
    if id >= 0x80 && id <= 0xFF {
        buf.push(id as u8);
    } else if id >= 0x4000 && id <= 0x7FFF {
        buf.push((id >> 8) as u8);
        buf.push(id as u8);
    } else if id >= 0x20_0000 && id <= 0x3F_FFFF {
        buf.push((id >> 16) as u8);
        buf.push((id >> 8) as u8);
        buf.push(id as u8);
    } else {
        buf.push((id >> 24) as u8);
        buf.push((id >> 16) as u8);
        buf.push((id >> 8) as u8);
        buf.push(id as u8);
    }
}

fn ebml_write_vint(buf: &mut Vec<u8>, value: u64) {
    ebml_write_vint_to_buf(buf, value);
}

fn ebml_write_vint_to_buf(buf: &mut Vec<u8>, value: u64) {
    if value < 0x7F {
        buf.push(0x80 | value as u8);
    } else if value < 0x3FFF {
        buf.push(0x40 | (value >> 8) as u8);
        buf.push(value as u8);
    } else if value < 0x1F_FFFF {
        buf.push(0x20 | (value >> 16) as u8);
        buf.push((value >> 8) as u8);
        buf.push(value as u8);
    } else {
        buf.push(0x10 | (value >> 24) as u8);
        buf.push((value >> 16) as u8);
        buf.push((value >> 8) as u8);
        buf.push(value as u8);
    }
}

fn ebml_write_uint(buf: &mut Vec<u8>, id: u32, value: u64) {
    ebml_write_id_to_buf(buf, id);
    if value <= 0xFF {
        ebml_write_vint_to_buf(buf, 1);
        buf.push(value as u8);
    } else if value <= 0xFFFF {
        ebml_write_vint_to_buf(buf, 2);
        buf.push((value >> 8) as u8);
        buf.push(value as u8);
    } else if value <= 0xFFFFFF {
        ebml_write_vint_to_buf(buf, 3);
        buf.push((value >> 16) as u8);
        buf.push((value >> 8) as u8);
        buf.push(value as u8);
    } else {
        ebml_write_vint_to_buf(buf, 4);
        buf.extend_from_slice(&(value as u32).to_be_bytes());
    }
}

fn ebml_write_float(buf: &mut Vec<u8>, id: u32, value: f64) {
    ebml_write_id_to_buf(buf, id);
    ebml_write_vint_to_buf(buf, 8);
    buf.extend_from_slice(&value.to_be_bytes());
}

fn ebml_write_string(buf: &mut Vec<u8>, id: u32, value: &str) {
    ebml_write_id_to_buf(buf, id);
    ebml_write_vint_to_buf(buf, value.len() as u64);
    buf.extend_from_slice(value.as_bytes());
}

fn ebml_write_master(w: &mut dyn Write, id: u32, data: &[u8]) -> std::io::Result<()> {
    let mut buf = Vec::new();
    ebml_write_master_to_buf(&mut buf, id, data);
    w.write_all(&buf)
}

fn ebml_write_master_to_buf(buf: &mut Vec<u8>, id: u32, data: &[u8]) {
    ebml_write_id_to_buf(buf, id);
    ebml_write_vint_to_buf(buf, data.len() as u64);
    buf.extend_from_slice(data);
}

fn io_err(e: std::io::Error) -> TarangError {
    TarangError::DemuxError(format!("mux write error: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Demuxer;
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
        let mut demuxer = crate::WavDemuxer::new(cursor);
        let info = demuxer.probe().unwrap();

        assert_eq!(info.format, tarang_core::ContainerFormat::Wav);
        let audio = info.audio_streams();
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

        let mut mux = OggMuxer::new(&mut buf, config);
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

        let mut mux = OggMuxer::new(&mut buf, config);
        mux.write_header().unwrap();

        let packet = vec![0xFCu8; 64];
        mux.write_packet(&packet).unwrap();
        mux.write_packet(&packet).unwrap();
        mux.finalize().unwrap();

        // Read it back with OggDemuxer
        let data = buf.into_inner();
        let cursor = Cursor::new(data);
        let mut demuxer = crate::OggDemuxer::new(cursor);
        let info = demuxer.probe().unwrap();

        assert_eq!(info.format, tarang_core::ContainerFormat::Ogg);
        let audio = info.audio_streams();
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

        let mut mux = OggMuxer::new(&mut buf, config);
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
        let mut mux = OggMuxer::new(&mut buf, config);
        assert!(mux.write_header().is_err());
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
        let mut demuxer = crate::Mp4Demuxer::new(cursor);
        let info = demuxer.probe().unwrap();

        assert_eq!(info.format, tarang_core::ContainerFormat::Mp4);
        let audio = info.audio_streams();
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
        let mut demuxer = crate::MkvDemuxer::new(cursor);
        let info = demuxer.probe().unwrap();

        assert_eq!(info.format, tarang_core::ContainerFormat::Mkv);
        let audio = info.audio_streams();
        assert_eq!(audio[0].codec, AudioCodec::Opus);
        assert_eq!(audio[0].sample_rate, 48000);
        assert_eq!(audio[0].channels, 2);

        let packet = demuxer.next_packet().unwrap();
        assert_eq!(packet.data.len(), 64);
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
        let mut demuxer = crate::MkvDemuxer::new(cursor);
        let info = demuxer.probe().unwrap();

        assert_eq!(info.format, tarang_core::ContainerFormat::WebM);
    }
}
