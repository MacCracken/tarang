use std::env;
use std::process::Command;

fn main() {
    let need_vpx = env::var("CARGO_FEATURE_VPX").is_ok()
        || env::var("CARGO_FEATURE_VPX_ENC").is_ok();

    if need_vpx {
        // Extract VPX_DECODER_ABI_VERSION and VPX_ENCODER_ABI_VERSION from system headers.
        // These are C macros that the -sys crate doesn't export.
        let cflags = Command::new("pkg-config")
            .args(["--cflags", "vpx"])
            .output()
            .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
            .unwrap_or_default();

        let src = r#"
#include <stdio.h>
#include <vpx/vpx_decoder.h>
#include <vpx/vpx_encoder.h>
int main() { printf("%d %d\n", VPX_DECODER_ABI_VERSION, VPX_ENCODER_ABI_VERSION); }
"#;

        let tmp_dir = env::var("OUT_DIR").unwrap();
        let src_path = format!("{tmp_dir}/vpx_abi_check.c");
        let bin_path = format!("{tmp_dir}/vpx_abi_check");

        std::fs::write(&src_path, src).expect("write vpx_abi_check.c");

        let mut cc_args: Vec<&str> = vec![&src_path, "-o", &bin_path];
        let cflags_split: Vec<&str> = cflags.split_whitespace().collect();
        cc_args.extend_from_slice(&cflags_split);

        let status = Command::new("cc")
            .args(&cc_args)
            .status()
            .expect("failed to compile vpx ABI check");
        assert!(status.success(), "vpx ABI version check compilation failed");

        let output = Command::new(&bin_path)
            .output()
            .expect("failed to run vpx ABI check");
        let versions = String::from_utf8_lossy(&output.stdout);
        let mut parts = versions.trim().split_whitespace();
        let decoder_abi: i32 = parts.next().unwrap().parse().unwrap();
        let encoder_abi: i32 = parts.next().unwrap().parse().unwrap();

        println!("cargo::rustc-cfg=vpx_decoder_abi=\"{decoder_abi}\"");
        println!("cargo::rustc-cfg=vpx_encoder_abi=\"{encoder_abi}\"");
        println!("cargo::rustc-env=VPX_DECODER_ABI_VERSION={decoder_abi}");
        println!("cargo::rustc-env=VPX_ENCODER_ABI_VERSION={encoder_abi}");
    }
}
