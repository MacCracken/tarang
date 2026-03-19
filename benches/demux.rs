use criterion::{Criterion, black_box, criterion_group, criterion_main};
use std::io::Cursor;
use tarang::demux::{Demuxer, Mp4Demuxer, WavDemuxer};

/// Build a minimal valid WAV file with `num_samples` of silence.
fn make_wav(sample_rate: u32, channels: u16, num_samples: usize) -> Vec<u8> {
    let bits_per_sample: u16 = 16;
    let bytes_per_sample = bits_per_sample / 8;
    let block_align = channels * bytes_per_sample;
    let byte_rate = sample_rate * block_align as u32;
    let data_size = (num_samples * channels as usize * bytes_per_sample as usize) as u32;
    let file_size = 36 + data_size;

    let mut wav = Vec::with_capacity(44 + data_size as usize);
    wav.extend_from_slice(b"RIFF");
    wav.extend_from_slice(&file_size.to_le_bytes());
    wav.extend_from_slice(b"WAVE");
    wav.extend_from_slice(b"fmt ");
    wav.extend_from_slice(&16u32.to_le_bytes());
    wav.extend_from_slice(&1u16.to_le_bytes());
    wav.extend_from_slice(&channels.to_le_bytes());
    wav.extend_from_slice(&sample_rate.to_le_bytes());
    wav.extend_from_slice(&byte_rate.to_le_bytes());
    wav.extend_from_slice(&block_align.to_le_bytes());
    wav.extend_from_slice(&bits_per_sample.to_le_bytes());
    wav.extend_from_slice(b"data");
    wav.extend_from_slice(&data_size.to_le_bytes());
    wav.extend_from_slice(&vec![0u8; data_size as usize]);
    wav
}

fn bench_wav_probe(c: &mut Criterion) {
    let wav = make_wav(44100, 2, 44100);

    c.bench_function("wav_probe_1s_stereo", |b| {
        b.iter(|| {
            let cursor = Cursor::new(black_box(&wav));
            let mut demuxer = WavDemuxer::new(cursor);
            demuxer.probe().unwrap()
        })
    });
}

fn bench_wav_read_packets(c: &mut Criterion) {
    let wav = make_wav(44100, 2, 44100);

    c.bench_function("wav_read_all_packets_1s", |b| {
        b.iter(|| {
            let cursor = Cursor::new(black_box(&wav));
            let mut demuxer = WavDemuxer::new(cursor);
            demuxer.probe().unwrap();
            let mut count = 0;
            while demuxer.next_packet().is_ok() {
                count += 1;
            }
            count
        })
    });
}

/// Build a minimal MP4 with `num_samples` of silence.
fn make_mp4(sample_rate: u32, channels: u16, num_samples: u32) -> Vec<u8> {
    let sample_size = 64u32;
    let mut buf = Vec::new();

    // ftyp
    let ftyp_start = buf.len();
    buf.extend_from_slice(&0u32.to_be_bytes());
    buf.extend_from_slice(b"ftyp");
    buf.extend_from_slice(b"isom");
    buf.extend_from_slice(&0u32.to_be_bytes());
    buf.extend_from_slice(b"isom");
    let ftyp_size = (buf.len() - ftyp_start) as u32;
    buf[ftyp_start..ftyp_start + 4].copy_from_slice(&ftyp_size.to_be_bytes());

    // moov
    let moov_start = buf.len();
    buf.extend_from_slice(&0u32.to_be_bytes());
    buf.extend_from_slice(b"moov");

    // mvhd
    let mvhd_start = buf.len();
    buf.extend_from_slice(&0u32.to_be_bytes());
    buf.extend_from_slice(b"mvhd");
    buf.extend_from_slice(&[0u8; 12]); // version, creation, modification
    buf.extend_from_slice(&sample_rate.to_be_bytes());
    buf.extend_from_slice(&(num_samples * 1024).to_be_bytes());
    buf.extend_from_slice(&[0u8; 80]);
    let mvhd_size = (buf.len() - mvhd_start) as u32;
    buf[mvhd_start..mvhd_start + 4].copy_from_slice(&mvhd_size.to_be_bytes());

    // trak
    let trak_start = buf.len();
    buf.extend_from_slice(&0u32.to_be_bytes());
    buf.extend_from_slice(b"trak");

    // tkhd
    let tkhd_start = buf.len();
    buf.extend_from_slice(&0u32.to_be_bytes());
    buf.extend_from_slice(b"tkhd");
    buf.extend_from_slice(&[0u8; 12]);
    buf.extend_from_slice(&1u32.to_be_bytes());
    buf.extend_from_slice(&[0u8; 68]);
    let tkhd_size = (buf.len() - tkhd_start) as u32;
    buf[tkhd_start..tkhd_start + 4].copy_from_slice(&tkhd_size.to_be_bytes());

    // mdia
    let mdia_start = buf.len();
    buf.extend_from_slice(&0u32.to_be_bytes());
    buf.extend_from_slice(b"mdia");

    // mdhd
    let mdhd_start = buf.len();
    buf.extend_from_slice(&0u32.to_be_bytes());
    buf.extend_from_slice(b"mdhd");
    buf.extend_from_slice(&[0u8; 12]);
    buf.extend_from_slice(&sample_rate.to_be_bytes());
    buf.extend_from_slice(&(num_samples * 1024).to_be_bytes());
    buf.extend_from_slice(&0u32.to_be_bytes());
    let mdhd_size = (buf.len() - mdhd_start) as u32;
    buf[mdhd_start..mdhd_start + 4].copy_from_slice(&mdhd_size.to_be_bytes());

    // hdlr
    let hdlr_start = buf.len();
    buf.extend_from_slice(&0u32.to_be_bytes());
    buf.extend_from_slice(b"hdlr");
    buf.extend_from_slice(&[0u8; 8]);
    buf.extend_from_slice(b"soun");
    buf.extend_from_slice(&[0u8; 12]);
    buf.push(0);
    let hdlr_size = (buf.len() - hdlr_start) as u32;
    buf[hdlr_start..hdlr_start + 4].copy_from_slice(&hdlr_size.to_be_bytes());

    // minf > stbl
    let minf_start = buf.len();
    buf.extend_from_slice(&0u32.to_be_bytes());
    buf.extend_from_slice(b"minf");
    let stbl_start = buf.len();
    buf.extend_from_slice(&0u32.to_be_bytes());
    buf.extend_from_slice(b"stbl");

    // stsd
    let stsd_start = buf.len();
    buf.extend_from_slice(&0u32.to_be_bytes());
    buf.extend_from_slice(b"stsd");
    buf.extend_from_slice(&0u32.to_be_bytes());
    buf.extend_from_slice(&1u32.to_be_bytes());
    let mp4a_start = buf.len();
    buf.extend_from_slice(&0u32.to_be_bytes());
    buf.extend_from_slice(b"mp4a");
    buf.extend_from_slice(&[0u8; 6]);
    buf.extend_from_slice(&1u16.to_be_bytes());
    buf.extend_from_slice(&[0u8; 8]);
    buf.extend_from_slice(&channels.to_be_bytes());
    buf.extend_from_slice(&16u16.to_be_bytes());
    buf.extend_from_slice(&[0u8; 4]);
    buf.extend_from_slice(&(sample_rate << 16).to_be_bytes());
    let mp4a_size = (buf.len() - mp4a_start) as u32;
    buf[mp4a_start..mp4a_start + 4].copy_from_slice(&mp4a_size.to_be_bytes());
    let stsd_size = (buf.len() - stsd_start) as u32;
    buf[stsd_start..stsd_start + 4].copy_from_slice(&stsd_size.to_be_bytes());

    // stts
    let stts_start = buf.len();
    buf.extend_from_slice(&0u32.to_be_bytes());
    buf.extend_from_slice(b"stts");
    buf.extend_from_slice(&0u32.to_be_bytes());
    buf.extend_from_slice(&1u32.to_be_bytes());
    buf.extend_from_slice(&num_samples.to_be_bytes());
    buf.extend_from_slice(&1024u32.to_be_bytes());
    let stts_size = (buf.len() - stts_start) as u32;
    buf[stts_start..stts_start + 4].copy_from_slice(&stts_size.to_be_bytes());

    // stsc
    let stsc_start = buf.len();
    buf.extend_from_slice(&0u32.to_be_bytes());
    buf.extend_from_slice(b"stsc");
    buf.extend_from_slice(&0u32.to_be_bytes());
    buf.extend_from_slice(&1u32.to_be_bytes());
    buf.extend_from_slice(&1u32.to_be_bytes());
    buf.extend_from_slice(&num_samples.to_be_bytes());
    buf.extend_from_slice(&1u32.to_be_bytes());
    let stsc_size = (buf.len() - stsc_start) as u32;
    buf[stsc_start..stsc_start + 4].copy_from_slice(&stsc_size.to_be_bytes());

    // stsz
    let stsz_start = buf.len();
    buf.extend_from_slice(&0u32.to_be_bytes());
    buf.extend_from_slice(b"stsz");
    buf.extend_from_slice(&0u32.to_be_bytes());
    buf.extend_from_slice(&sample_size.to_be_bytes());
    buf.extend_from_slice(&num_samples.to_be_bytes());
    let stsz_size = (buf.len() - stsz_start) as u32;
    buf[stsz_start..stsz_start + 4].copy_from_slice(&stsz_size.to_be_bytes());

    // stco
    let stco_start = buf.len();
    buf.extend_from_slice(&0u32.to_be_bytes());
    buf.extend_from_slice(b"stco");
    buf.extend_from_slice(&0u32.to_be_bytes());
    buf.extend_from_slice(&1u32.to_be_bytes());
    let stco_offset_pos = buf.len();
    buf.extend_from_slice(&0u32.to_be_bytes());
    let stco_size = (buf.len() - stco_start) as u32;
    buf[stco_start..stco_start + 4].copy_from_slice(&stco_size.to_be_bytes());

    // Close boxes
    let stbl_size = (buf.len() - stbl_start) as u32;
    buf[stbl_start..stbl_start + 4].copy_from_slice(&stbl_size.to_be_bytes());
    let minf_size = (buf.len() - minf_start) as u32;
    buf[minf_start..minf_start + 4].copy_from_slice(&minf_size.to_be_bytes());
    let mdia_size = (buf.len() - mdia_start) as u32;
    buf[mdia_start..mdia_start + 4].copy_from_slice(&mdia_size.to_be_bytes());
    let trak_size = (buf.len() - trak_start) as u32;
    buf[trak_start..trak_start + 4].copy_from_slice(&trak_size.to_be_bytes());
    let moov_size = (buf.len() - moov_start) as u32;
    buf[moov_start..moov_start + 4].copy_from_slice(&moov_size.to_be_bytes());

    // mdat
    let mdat_data_offset = buf.len() + 8;
    buf[stco_offset_pos..stco_offset_pos + 4]
        .copy_from_slice(&(mdat_data_offset as u32).to_be_bytes());
    let total_data = num_samples * sample_size;
    let mdat_start = buf.len();
    buf.extend_from_slice(&0u32.to_be_bytes());
    buf.extend_from_slice(b"mdat");
    buf.extend_from_slice(&vec![0xAA; total_data as usize]);
    let mdat_size = (buf.len() - mdat_start) as u32;
    buf[mdat_start..mdat_start + 4].copy_from_slice(&mdat_size.to_be_bytes());

    buf
}

fn bench_mp4_probe(c: &mut Criterion) {
    let mp4 = make_mp4(44100, 2, 100);

    c.bench_function("mp4_probe_100_samples", |b| {
        b.iter(|| {
            let cursor = Cursor::new(black_box(&mp4));
            let mut demuxer = Mp4Demuxer::new(cursor);
            demuxer.probe().unwrap()
        })
    });
}

fn bench_mp4_read_packets(c: &mut Criterion) {
    let mp4 = make_mp4(44100, 2, 1000);

    c.bench_function("mp4_read_1000_packets", |b| {
        b.iter(|| {
            let cursor = Cursor::new(black_box(&mp4));
            let mut demuxer = Mp4Demuxer::new(cursor);
            demuxer.probe().unwrap();
            let mut count = 0;
            while demuxer.next_packet().is_ok() {
                count += 1;
            }
            count
        })
    });
}

criterion_group!(
    benches,
    bench_wav_probe,
    bench_wav_read_packets,
    bench_mp4_probe,
    bench_mp4_read_packets
);
criterion_main!(benches);
