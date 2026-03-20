#![no_main]
use libfuzzer_sys::fuzz_target;
use std::io::Cursor;
use tarang::demux::{Demuxer, OggDemuxer};

fuzz_target!(|data: &[u8]| {
    let cursor = Cursor::new(data.to_vec());
    let mut demuxer = OggDemuxer::new(cursor);

    if demuxer.probe().is_ok() {
        for _ in 0..1000 {
            if demuxer.next_packet().is_err() {
                break;
            }
        }
        let _ = demuxer.seek(std::time::Duration::from_secs(1));
    }
});
