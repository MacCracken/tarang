# Security Policy

## Scope

Tarang is an AI-native media framework that parses untrusted media files,
calls C libraries through FFI, and communicates with AI services. Its threat
surface includes:

- **Media file parsing** -- container demuxing (MP4, MKV, OGG, WAV) processes
  untrusted input including malformed headers, oversized allocations, and
  crafted bitstreams.
- **FFI boundaries** -- video codec backends (dav1d, openh264, libvpx) are C
  libraries. Memory corruption in these libraries could be exploitable.
- **AI/ML API calls** -- the hoosh (Whisper) and daimon (LLM) clients make
  outbound HTTP requests with media-derived content.
- **MCP server** -- JSON-RPC over stdio. No network listening, but processes
  tool invocations from the connected agent.

## Supported versions

Only the latest released version receives security fixes.

| Version | Supported |
|---|---|
| Latest | Yes |
| Older | No |

## Reporting a vulnerability

**Do not open a public issue for security vulnerabilities.**

Instead, please report vulnerabilities privately via
[GitHub Security Advisories](https://github.com/AskCortal/tarang/security/advisories/new)
or by emailing the maintainer directly.

Include:

- A description of the vulnerability.
- Steps to reproduce or a proof of concept.
- The potential impact.

You should receive an acknowledgement within 72 hours. We aim to release a fix
within 14 days of confirmation.

## Security considerations

- **Container parsing**: All demuxers validate sizes before allocation. MP4 box
  sizes are capped at 4 GB to prevent OOM. MKV SimpleBlock parsing includes
  bounds checks for malformed input. OGG pages are CRC-32 validated.
- **FFI safety**: All C library calls include bounds checks on input data before
  crossing the FFI boundary. RAII guards manage C resource lifetimes. Pixel
  format and dimension validation occurs before unsafe operations. Plane stride
  values from C libraries are validated before use.
- **Network**: AI API clients (hoosh, daimon) use HTTPS via rustls. The MCP
  server communicates only over stdio -- it does not bind any network port.
- **Serialization**: Types that derive `Serialize`/`Deserialize` should be
  validated after deserialization if the source is untrusted.
- **Supply chain**: `cargo audit` and `cargo deny` run in CI to catch known
  vulnerabilities and license violations.
