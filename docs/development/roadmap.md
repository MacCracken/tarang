# Tarang Roadmap

> **Principle**: FOSS codecs first, proprietary codecs next, wide coverage always in scope.

## Waiting on Upstream
- [ ] **VA-API encode pipeline completion** — surface upload, parameter buffers, bitstream readback. Blocked on `cros-codecs` releasing a version compatible with `cros-libva` 0.0.13 (current cros-codecs 0.0.6 depends on cros-libva 0.0.12). *(added 2026-03-16)*
- [ ] **rav1e `paste` dependency** — rav1e 0.8.1 depends on `paste` 1.0.15 which is unmaintained (RUSTSEC-2024-0436). No security vulnerability, but watch for rav1e release that drops or replaces it. *(added 2026-03-16)*
