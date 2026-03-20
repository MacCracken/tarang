#![no_main]
use libfuzzer_sys::fuzz_target;
use std::io::Cursor;
use tarang::demux::{Demuxer, MkvDemuxer};

fuzz_target!(|data: &[u8]| {
    let cursor = Cursor::new(data.to_vec());
    let mut demuxer = MkvDemuxer::new(cursor);

    if demuxer.probe().is_ok() {
        // Read chapters (should never panic)
        let _ = demuxer.chapters();

        for _ in 0..1000 {
            if demuxer.next_packet().is_err() {
                break;
            }
        }
        let _ = demuxer.seek(std::time::Duration::from_secs(1));
    }
});
