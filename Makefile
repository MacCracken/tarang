.PHONY: check fmt clippy test bench audit deny build doc clean

# System dependencies (Arch/AGNOS):
#   sudo pacman -S nasm libvpx dav1d opus libfdk-aac pipewire libva clang
# Debian/Ubuntu:
#   sudo apt install nasm libvpx-dev libdav1d-dev libva-dev libopus-dev \
#       libfdk-aac-dev libpipewire-0.3-dev clang

# Run all CI checks locally
check: fmt clippy test audit

# Format check
fmt:
	cargo fmt --all -- --check

# Lint (zero warnings)
clippy:
	cargo clippy --all-targets -- -D warnings

# Run test suite
test:
	cargo test
	cargo test --features openh264,openh264-enc,vpx,vpx-enc,dav1d,vaapi

# Run benchmarks (criterion)
bench:
	cargo bench

# Security audit
audit:
	cargo audit

# Supply-chain checks (license + advisory + source)
deny:
	cargo deny check

# Build release
build:
	cargo build --release

# Generate documentation
doc:
	cargo doc --no-deps

# Clean build artifacts
clean:
	cargo clean
