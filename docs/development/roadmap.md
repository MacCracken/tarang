# Tarang Roadmap

> **Principle**: FOSS codecs first, proprietary codecs next, wide coverage always in scope.

## Waiting on Upstream

- [ ] **VA-API encode pipeline completion** — surface upload, parameter buffers, bitstream readback. Blocked on `cros-codecs` updating its `cros-libva` dependency from `^0.0.12` to `^0.0.13`. Upstream repo (chromeos/cros-codecs) has had no commits since March 2025; no PRs tracking this. Workarounds: fork cros-codecs, use `[patch]`, or downgrade to cros-libva 0.0.12. *(added 2026-03-16, audited 2026-03-19)*

- [ ] **rav1e `paste` dependency** — PR #3442 merged upstream (paste → pastey), but no release since 0.8.1 (September 2025). Our lockfile still pulls `paste 1.0.15` (RUSTSEC-2024-0436, informational only). Resolves automatically when rav1e cuts 0.8.2 or 0.9.0. *(added 2026-03-16, audited 2026-03-19)*
