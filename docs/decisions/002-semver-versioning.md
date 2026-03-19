# ADR 002: Semantic versioning for crates.io

## Status

Accepted (supersedes calendar versioning)

## Date

2026-03-19

## Context

Tarang was originally versioned with calendar versioning (`YYYY.M.D`). This
worked well during rapid early development within the AGNOS ecosystem where
downstream consumers pinned exact versions.

With the move to crates.io publishing, calver has significant drawbacks:

- crates.io treats all versions as semver. A calver version like `2026.3.19`
  is interpreted as major version 2026, making dependency resolution and
  compatibility ranges meaningless.
- External consumers (outside AGNOS) cannot express compatible version ranges
  in `Cargo.toml` — `^2026.3` would accept any 2026.3.x but reject 2026.4.x,
  which is not the intended semantics.
- The Rust ecosystem universally expects semver. Non-semver versions create
  friction for adoption.

## Decision

Tarang uses semantic versioning (`MAJOR.MINOR.PATCH`) starting at `0.19.3`.

- `0.x.y` signals pre-1.0 status: the public API may change between minor
  versions.
- Patch bumps (`0.19.3` → `0.19.4`) for bug fixes and security patches.
- Minor bumps (`0.19.x` → `0.20.0`) for new features or non-trivial API
  changes.
- Major bump to `1.0.0` when the public API stabilizes.

The canonical version lives in the `VERSION` file at the repository root.
`scripts/version-bump.sh` updates `VERSION` and `Cargo.toml` atomically.
Contributors do not bump versions — maintainers handle it during release.

CI verifies that `VERSION`, `Cargo.toml`, and the git tag all agree before
publishing.

## Consequences

**Positive**:

- Standard crates.io compatibility: consumers can use `tarang = "0.19"` and
  get patch updates automatically.
- Clear signal about API stability (pre-1.0).
- Aligns with Rust ecosystem conventions.

**Negative**:

- Requires judgment calls about breaking vs. non-breaking changes. Mitigated
  by `#[non_exhaustive]` on public enums and staying at 0.x pre-1.0.
- Version numbers no longer encode release dates. Mitigated by CHANGELOG.md
  and git tags.
