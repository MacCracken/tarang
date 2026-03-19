//! tarang — AI-native Rust media framework for AGNOS
//!
//! Container parsing, audio/video decoding, media pipeline, and AI-powered
//! content analysis. Pure Rust audio decoding via symphonia, thin FFI wrappers
//! for video codecs (dav1d, openh264, libvpx, rav1e), and LLM integration
//! through hoosh.

pub mod core;
pub mod demux;
pub mod audio;
pub mod video;
pub mod ai;
