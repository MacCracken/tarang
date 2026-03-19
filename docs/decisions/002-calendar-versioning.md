# ADR 002: Calendar versioning

## Status

Accepted

## Date

2026-03-16

## Context

Tarang is a media framework that evolves alongside the AGNOS ecosystem.
Traditional semver (MAJOR.MINOR.PATCH) requires judgment calls about what
constitutes a breaking change versus a new feature, and the version numbers
carry no temporal information.

For a project that:

- Ships as part of a larger OS distribution (AGNOS),
- Has downstream consumers (Jalwa, Tazama, Shruti) that track the latest
  version,
- Releases frequently with mixed changes (new codecs, security fixes,
  refactoring),

the semver ceremony adds friction without proportional benefit. Downstream
consumers already pin to exact versions via workspace dependencies.

## Decision

Tarang uses calendar versioning with the scheme `YYYY.M.D`. Patch releases
on the same day use a `-N` suffix (e.g., `2026.3.16-1`).

The canonical version lives in the `VERSION` file at the repository root.
`scripts/version-bump.sh` updates `VERSION` and all `Cargo.toml` files in
the workspace atomically. Contributors do not bump versions in their PRs --
maintainers handle version bumps as part of the release process.

CI verifies that `VERSION`, `Cargo.toml`, and the git tag (if present) all
agree before publishing to crates.io.

## Consequences

**Positive**:

- The version immediately communicates when a release was made.
- No debates about semver compliance for internal API changes.
- Simple, scriptable version bumping.
- Consistent with other AGNOS crates (e.g., ai-hwaccel).

**Negative**:

- Consumers cannot rely on semver to distinguish breaking changes. In
  practice this is acceptable because downstream crates pin exact versions
  and are maintained in the same ecosystem.
- Multiple releases on the same day require the `-N` suffix convention.
