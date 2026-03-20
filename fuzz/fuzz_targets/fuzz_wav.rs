#![no_main]
use libfuzzer_sys::fuzz_target;
use std::io::Cursor;
use tarang::demux::{Demuxer, WavDemuxer};

fuzz_target!(|data: &[u8]| {
    let cursor = Cursor::new(data.to_vec());
    let mut demuxer = WavDemuxer::new(cursor);

    // Probe should never panic — errors are fine
    if demuxer.probe().is_ok() {
        // Try reading packets until EOF or error
        for _ in 0..1000 {
            if demuxer.next_packet().is_err() {
                break;
            }
        }
    }
});
