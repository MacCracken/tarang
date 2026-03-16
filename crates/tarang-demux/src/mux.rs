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
}
