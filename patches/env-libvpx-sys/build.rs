use std::env;
use std::path::Path;

pub fn main() {
    println!("cargo:rerun-if-env-changed=VPX_VERSION");
    println!("cargo:rerun-if-env-changed=VPX_LIB_DIR");
    println!("cargo:rerun-if-env-changed=VPX_INCLUDE_DIR");
    println!("cargo:rerun-if-env-changed=VPX_STATIC");
    println!("cargo:rerun-if-env-changed=VPX_DYNAMIC");

    let requested_version = env::var("VPX_VERSION").ok();

    let src_dir = env::var_os("CARGO_MANIFEST_DIR").unwrap();
    let src_dir = Path::new(&src_dir);

    let ffi_header = src_dir.join("ffi.h");
    let ffi_rs = {
        let out_dir = env::var_os("OUT_DIR").unwrap();
        let out_dir = Path::new(&out_dir);
        out_dir.join("ffi.rs")
    };

    #[allow(unused_variables)]
    let (found_version, include_paths) = match env::var_os("VPX_LIB_DIR") {
        None => {
            // use VPX config from pkg-config
            let lib = pkg_config::probe_library("vpx").unwrap();

            if let Some(v) = requested_version {
                if lib.version != v {
                    panic!(
                        "version mismatch. pkg-config returns version {}, but VPX_VERSION \
                    environment variable is {}.",
                        lib.version, v
                    );
                }
            }
            (lib.version, lib.include_paths)
        }
        Some(vpx_libdir) => {
            // use VPX config from environment variable
            let libdir = std::path::Path::new(&vpx_libdir);

            // Set lib search path.
            println!("cargo:rustc-link-search=native={}", libdir.display());

            // Get static using pkg-config-rs rules.
            let statik = infer_static("VPX");

            // Set libname.
            if statik {
                #[cfg(target_os = "windows")]
                println!("cargo:rustc-link-lib=static=libvpx");
                #[cfg(not(target_os = "windows"))]
                println!("cargo:rustc-link-lib=static=vpx");
            } else {
                println!("cargo:rustc-link-lib=vpx");
            }

            let mut include_paths = vec![];
            if let Some(include_dir) = env::var_os("VPX_INCLUDE_DIR") {
                include_paths.push(include_dir.into());
            }
            let version = requested_version.unwrap_or_else(|| {
                panic!("If VPX_LIB_DIR is set, VPX_VERSION must also be defined.")
            });
            (version, include_paths)
        }
    };

    println!("rerun-if-changed={}", ffi_header.display());
    for dir in &include_paths {
        println!("rerun-if-changed={}", dir.display());
    }

    #[cfg(feature = "generate")]
    generate_bindings(&ffi_header, &include_paths, &ffi_rs);

    #[cfg(not(feature = "generate"))]
    {
        let full_src = find_pregenerated_bindings(&found_version);
        std::fs::copy(&full_src, &ffi_rs).unwrap();
    }
}

// This function was modified from pkg-config-rs and should have same behavior.
fn infer_static(name: &str) -> bool {
    if env::var_os(&format!("{}_STATIC", name)).is_some() {
        true
    } else if env::var_os(&format!("{}_DYNAMIC", name)).is_some() {
        false
    } else if env::var_os("PKG_CONFIG_ALL_STATIC").is_some() {
        true
    } else if env::var_os("PKG_CONFIG_ALL_DYNAMIC").is_some() {
        false
    } else {
        false
    }
}

/// Find pre-generated bindings for the given vpx version.
/// Tries exact match first, then falls back to the highest available version
/// that doesn't exceed the detected version (ABI is stable within major versions).
#[cfg(not(feature = "generate"))]
fn find_pregenerated_bindings(version: &str) -> std::path::PathBuf {
    let exact = std::path::PathBuf::from("generated").join(format!("vpx-ffi-{}.rs", version));
    if exact.exists() {
        return exact;
    }

    // Parse detected version for comparison
    let detected_parts: Vec<u32> = version
        .split('.')
        .filter_map(|s| s.parse().ok())
        .collect();

    // Scan generated/ for the best fallback
    let mut best: Option<(Vec<u32>, std::path::PathBuf)> = None;
    if let Ok(entries) = std::fs::read_dir("generated") {
        for entry in entries.flatten() {
            let name = entry.file_name();
            let name = name.to_string_lossy();
            if let Some(ver_str) = name.strip_prefix("vpx-ffi-").and_then(|s| s.strip_suffix(".rs"))
            {
                let parts: Vec<u32> = ver_str
                    .split('.')
                    .filter_map(|s| s.parse().ok())
                    .collect();
                if parts <= detected_parts {
                    if best.as_ref().map_or(true, |(b, _)| &parts > b) {
                        best = Some((parts, entry.path()));
                    }
                }
            }
        }
    }

    match best {
        Some((_, path)) => {
            eprintln!(
                "cargo:warning=No exact vpx bindings for {}; using {}",
                version,
                path.display()
            );
            path
        }
        None => panic!(
            "No pre-generated vpx bindings found for version {} or any earlier version. \
             Enable the 'generate' cargo feature or add bindings to generated/.",
            version
        ),
    }
}

#[cfg(feature = "generate")]
fn generate_bindings(ffi_header: &Path, include_paths: &[std::path::PathBuf], ffi_rs: &Path) {
    let mut b = bindgen::builder()
        .header(ffi_header.to_str().unwrap())
        .allowlist_type("^[vV].*")
        .allowlist_var("^[vV].*")
        .allowlist_function("^[vV].*")
        .rustified_enum("^v.*")
        .trust_clang_mangling(false)
        .layout_tests(false) // breaks 32/64-bit compat
        .generate_comments(false); // vpx comments have prefix /*!\

    for dir in include_paths {
        b = b.clang_arg(format!("-I{}", dir.display()));
    }

    b.generate().unwrap().write_to_file(ffi_rs).unwrap();
}
