# Tarang

**AI-native Rust media framework for AGNOS**

Tarang (Sanskrit: wave) replaces ffmpeg as the media decode pipeline. Pure Rust audio decoding via symphonia, thin C FFI wrappers for video codecs (dav1d, openh264, libvpx), and AI-powered content analysis.

## Architecture

```
tarang-core     — codecs, formats, buffers, pipeline primitives, magic bytes
tarang-demux    — container parsing (WAV pure Rust, MP4/MKV/WebM planned)
tarang-audio    — audio decoding via symphonia (MP3, FLAC, WAV, OGG, AAC, ALAC, Opus)
tarang-video    — video decoding (dav1d/openh264/libvpx backend stubs)
tarang-ai       — content classification, quality scoring, codec recommendations, transcription routing
```

## Why not ffmpeg?

ffmpeg is a 500K+ line C monolith. Tarang takes the opposite approach:

- **Smaller attack surface** — only the codecs you need
- **Memory safety** — pipeline is Rust; only codec math is C
- **Modularity** — each codec is a separate dep, not a monolith
- **AGNOS-native** — fits the Rust-first, security-conscious OS

## Usage

```bash
# Probe a media file
tarang probe song.flac

# AI content analysis
tarang analyze movie.mp4

# List supported codecs
tarang codecs

# Run as MCP server
tarang mcp
```

## MCP Tools

| Tool | Description |
|------|-------------|
| `tarang_probe` | Probe media file for format, codec, stream info |
| `tarang_analyze` | AI content classification and quality scoring |
| `tarang_codecs` | List supported audio/video codecs |
| `tarang_transcribe` | Prepare transcription request (routes to hoosh) |
| `tarang_formats` | Detect container format from magic bytes |

## Downstream Consumers

- **Jalwa** — AI-native media player (primary consumer)
- **Tazama** — video editor (planned migration from GStreamer)
- **Shruti** — DAW (planned unification under tarang-audio)

## License

GPL-3.0
