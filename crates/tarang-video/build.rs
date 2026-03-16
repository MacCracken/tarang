use std::env;
use std::process::Command;

fn main() {
    println!("cargo::rerun-if-changed=build.rs");
    println!("cargo::rerun-if-env-changed=VPX_DECODER_ABI_VERSION");
    println!("cargo::rerun-if-env-changed=VPX_ENCODER_ABI_VERSION");
    println!("cargo::rerun-if-env-changed=CC");

    let need_vpx =
        env::var("CARGO_FEATURE_VPX").is_ok() || env::var("CARGO_FEATURE_VPX_ENC").is_ok();

    if need_vpx {
        // Allow manual override for cross-compilation
        let dec_override = env::var("VPX_DECODER_ABI_VERSION");
        let enc_override = env::var("VPX_ENCODER_ABI_VERSION");
        match (&dec_override, &enc_override) {
            (Ok(dec), Ok(enc)) => {
                println!("cargo::rustc-env=VPX_DECODER_ABI_VERSION={dec}");
                println!("cargo::rustc-env=VPX_ENCODER_ABI_VERSION={enc}");
                return;
            }
            (Ok(_), Err(_)) | (Err(_), Ok(_)) => {
                panic!(
                    "Both VPX_DECODER_ABI_VERSION and VPX_ENCODER_ABI_VERSION must be set together \
                     (for cross-compilation), or neither."
                );
            }
            _ => {} // Neither set — fall through to probe
        }

        // Extract ABI versions from system headers (C macros not exported by the -sys crate)
        let cflags = Command::new("pkg-config")
            .args(["--cflags", "vpx"])
            .output()
            .ok()
            .filter(|o| o.status.success())
            .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
            .unwrap_or_default();

        if cflags.is_empty() {
            // pkg-config may not be available but headers could still be in default paths
            eprintln!("cargo::warning=pkg-config did not find 'vpx'; trying default include paths");
        }

        let src = r#"
#include <stdio.h>
#include <vpx/vpx_decoder.h>
#include <vpx/vpx_encoder.h>
int main() { printf("%d %d\n", VPX_DECODER_ABI_VERSION, VPX_ENCODER_ABI_VERSION); return 0; }
"#;

        let out_dir = env::var("OUT_DIR").expect("OUT_DIR not set");
        let src_path = format!("{out_dir}/vpx_abi_check.c");
        let bin_path = format!("{out_dir}/vpx_abi_check");

        std::fs::write(&src_path, src).expect("failed to write vpx_abi_check.c");

        // Determine C compiler: respect $CC, fall back to "cc"
        let cc = env::var("CC").unwrap_or_else(|_| "cc".to_string());

        let mut cc_args: Vec<String> = vec![src_path.clone(), "-o".to_string(), bin_path.clone()];
        for flag in cflags.split_whitespace() {
            cc_args.push(flag.to_string());
        }

        let status = Command::new(&cc)
            .args(&cc_args)
            .status()
            .unwrap_or_else(|e| panic!(
                "failed to run C compiler '{cc}' for vpx ABI probe: {e}. \
                 Install a C compiler or set VPX_DECODER_ABI_VERSION and VPX_ENCODER_ABI_VERSION env vars."
            ));

        if !status.success() {
            panic!(
                "vpx ABI probe compilation failed. Ensure libvpx headers are installed \
                 (e.g. libvpx-dev) or set VPX_DECODER_ABI_VERSION and VPX_ENCODER_ABI_VERSION \
                 env vars manually."
            );
        }

        let output = Command::new(&bin_path)
            .output()
            .unwrap_or_else(|e| panic!(
                "failed to execute vpx ABI probe binary: {e}. \
                 For cross-compilation, set VPX_DECODER_ABI_VERSION and VPX_ENCODER_ABI_VERSION env vars."
            ));

        let versions = String::from_utf8_lossy(&output.stdout);
        let mut parts = versions.trim().split_whitespace();
        let decoder_abi: i32 = parts
            .next()
            .expect("vpx ABI probe produced no output")
            .parse()
            .expect("failed to parse VPX_DECODER_ABI_VERSION from probe");
        let encoder_abi: i32 = parts
            .next()
            .expect("vpx ABI probe missing encoder version")
            .parse()
            .expect("failed to parse VPX_ENCODER_ABI_VERSION from probe");

        println!("cargo::rustc-env=VPX_DECODER_ABI_VERSION={decoder_abi}");
        println!("cargo::rustc-env=VPX_ENCODER_ABI_VERSION={encoder_abi}");
    }
}
