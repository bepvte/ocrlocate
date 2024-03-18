extern crate bindgen;

use std::env;
use std::fs;
use std::path::{Path, PathBuf};
#[cfg(windows)]
use vcpkg;

#[cfg(windows)]
fn find_tesseract_system_lib() -> Vec<String> {
    println!("cargo:rerun-if-env-changed=TESSERACT_INCLUDE_PATHS");
    println!("cargo:rerun-if-env-changed=TESSERACT_LINK_PATHS");
    println!("cargo:rerun-if-env-changed=TESSERACT_LINK_LIBS");

    let vcpkg = || {
        let lib = vcpkg::Config::new().find_package("tesseract").unwrap();

        vec![lib
            .include_paths
            .iter()
            .map(|x| x.to_string_lossy())
            .collect::<String>()]
    };

    let include_paths = env::var("TESSERACT_INCLUDE_PATHS").ok();
    let include_paths = include_paths.as_deref().map(|x| x.split(','));
    let link_paths = env::var("TESSERACT_LINK_PATHS").ok();
    let link_paths = link_paths.as_deref().map(|x| x.split(','));
    let link_libs = env::var("TESSERACT_LINK_LIBS").ok();
    let link_libs = link_libs.as_deref().map(|x| x.split(','));
    if let (Some(include_paths), Some(link_paths), Some(link_libs)) =
        (include_paths, link_paths, link_libs)
    {
        for link_path in link_paths {
            println!("cargo:rustc-link-search={}", link_path)
        }

        for link_lib in link_libs {
            println!("cargo:rustc-link-lib={}", link_lib)
        }

        include_paths.map(|x| x.to_string()).collect::<Vec<_>>()
    } else {
        vcpkg()
    }
}

// we sometimes need additional search paths, which we get using pkg-config
// we can use tesseract installed anywhere on Linux.
// if you change install path(--prefix) to `configure` script.
// set `export PKG_CONFIG_PATH=/path-to-lib/pkgconfig` before.
#[cfg(any(target_os = "macos", target_os = "linux"))]
#[cfg(not(feature = "bundled"))]
fn find_tesseract_system_lib() -> Vec<String> {
    let pk = pkg_config::Config::new()
        .atleast_version("4.1")
        .probe("tesseract")
        .unwrap();
    // Tell cargo to tell rustc to link the system proj shared library.
    println!("cargo:rustc-link-search=native={:?}", pk.link_paths[0]);
    println!("cargo:rustc-link-lib=tesseract");

    let mut include_paths = pk.include_paths.clone();
    include_paths
        .iter_mut()
        .map(|x| {
            if !x.ends_with("include") {
                x.pop();
            }
            x
        })
        .map(|x| x.to_string_lossy())
        .map(|x| x.to_string())
        .collect::<Vec<String>>()
}

#[cfg(all(not(windows), not(target_os = "macos"), not(target_os = "linux")))]
fn find_tesseract_system_lib() -> Vec<String> {
    println!("cargo:rustc-link-lib=tesseract");
    vec![]
}

fn capi_bindings(clang_extra_include: &[String]) -> bindgen::Bindings {
    let mut capi_bindings = bindgen::Builder::default()
        .header("wrapper_capi.h")
        .allowlist_function("^Tess.*")
        .blocklist_type("Boxa")
        .blocklist_type("Pix")
        .blocklist_type("Pixa")
        .blocklist_type("_IO_FILE")
        .blocklist_type("_IO_codecvt")
        .blocklist_type("_IO_marker")
        .blocklist_type("_IO_wide_data");

    for inc in clang_extra_include {
        capi_bindings = capi_bindings.clang_arg(format!("-I{}", *inc));
    }

    capi_bindings
        .generate()
        .expect("Unable to generate capi bindings")
}

#[cfg(not(target_os = "macos"))]
fn public_types_bindings(clang_extra_include: &[String]) -> String {
    let mut public_types_bindings = bindgen::Builder::default()
        .header("wrapper_public_types.hpp")
        .allowlist_var("^k.*")
        .allowlist_var("^tesseract::k.*")
        .blocklist_item("^kPolyBlockNames")
        .blocklist_item("^tesseract::kPolyBlockNames");

    for inc in clang_extra_include {
        public_types_bindings = public_types_bindings.clang_arg(format!("-I{}", *inc));
    }

    public_types_bindings
        .generate()
        .expect("Unable to generate public types bindings")
        .to_string()
        .replace("tesseract_k", "k")
}

#[cfg(feature = "bundled")]
fn find_tesseract_system_lib() -> Vec<String> {
    use cmake::Config;
    use std::process::Command;

    let tesseract_ver = "2b07505e0e86026ae7c10767b334c337ccf06576";
    let tesseract_url =
        format!("https://github.com/tesseract-ocr/tesseract/archive/{tesseract_ver}.tar.gz");

    let out_dir = PathBuf::from(env::var("OUT_DIR").unwrap());
    let tess_tgz = out_dir
        .join(format!("tesseract-{}.tar.gz", tesseract_ver))
        .into_os_string()
        .into_string()
        .unwrap();

    let curl_status = Command::new("curl")
        .current_dir(&out_dir)
        .args([
            "-z",
            &tess_tgz,
            "-RsSfL",
            "--tlsv1.2",
            &tesseract_url,
            "-o",
            &tess_tgz,
        ])
        .status()
        .expect("failed to execute curl to download tesseract");
    if !curl_status.success() {
        panic!("failed to download tesseract");
    }

    let tar_status = Command::new("tar")
        .current_dir(&out_dir)
        .args(["-xzf", &tess_tgz])
        .status()
        .expect("failed to execute tar to unarchive tesseract");
    if !tar_status.success() {
        panic!("failed to unarchive tesseract");
    }

    let src_dir = out_dir
        .join(format!("tesseract-{tesseract_ver}"))
        .to_owned();

    let mut cm = Config::new(&src_dir);
    cm.define("TESSDATA_PREFIX", find_tessdata_path())
        .define(
            "ENABLE_LTO",
            if cfg!(not(debug_assertions)) {
                "ON"
            } else {
                "OFF"
            },
        )
        .define("DISABLED_LEGACY_ENGINE", "ON")
        .define("BUILD_TRAINING_TOOLS", "OFF")
        .define("BUILD_SHARED_LIBS", "OFF")
        .define("OPENMP_BUILD", "OFF")
        .define("GRAPHICS_DISABLED", "ON")
        .define("DISABLE_ARCHIVE", "ON")
        .define("DISABLE_CURL", "ON")
        // this flag disables tesseract recompressing every image as a png
        // no idea why anyone would do that
        .cflag("-DTESSERACT_IMAGEDATA_AS_PIX")
        .cxxflag("-DTESSERACT_IMAGEDATA_AS_PIX");

    if env::var("CI").is_err()
        && env::var("CARGO_ENCODED_RUSTFLAGS").is_ok_and(|x| x.contains("target-cpu=native"))
    {
        cm.define("ENABLE_NATIVE", "ON");
    } else {
        println!("cargo:warning=disabling native architecture optimizaton, put -Ctarget-cpu=native in rustflags to enable it");
    }

    let dst = cm.build();

    println!(
        "cargo:rustc-link-search=native={}",
        dst.join("lib").to_str().unwrap()
    );
    println!("cargo:rustc-link-lib=static=tesseract");
    println!("cargo:rustc-link-lib=stdc++");

    vec![src_dir
        .join("include")
        .into_os_string()
        .into_string()
        .unwrap()]
}

#[allow(dead_code)]
fn find_tessdata_path() -> String {
    println!("cargo:rerun-if-env-changed=TESSDATA_PREFIX");
    if let Ok(envvar) = env::var("TESSDATA_PREFIX") {
        return envvar;
    }
    for p in [
        "/usr/share/tessdata",
        "/usr/share/tesseract/tessdata",
        "/usr/share/tesseract-ocr/4.00/tessdata",
        "/usr/share/tesseract-ocr/5.00/tessdata",
        "/usr/share/tesseract-ocr/4/tessdata",
        "/usr/share/tesseract-ocr/5/tessdata",
    ] {
        let path = Path::new(p);
        if path.exists() {
            return path.parent().unwrap().to_str().unwrap().to_owned();
        };
    }
    panic!("Could not find tessdata directory, set the TESSDATA_PREFIX environment variable");
}

// MacOS clang is incompatible with Bindgen and constexpr
// https://github.com/rust-lang/rust-bindgen/issues/1948
// Hardcode the constants rather than reading them dynamically
#[cfg(target_os = "macos")]
fn public_types_bindings(_clang_extra_include: &[String]) -> &'static str {
    include_str!("src/public_types_bindings_mac.rs")
}

fn main() {
    // Tell cargo to tell rustc to link the system tesseract
    // and leptonica shared libraries.
    let clang_extra_include = find_tesseract_system_lib();

    // Write the bindings to the $OUT_DIR/bindings.rs file.
    let out_path = PathBuf::from(env::var("OUT_DIR").unwrap());
    capi_bindings(&clang_extra_include)
        .write_to_file(out_path.join("capi_bindings.rs"))
        .expect("Couldn't write capi bindings!");
    fs::write(
        out_path.join("public_types_bindings.rs"),
        public_types_bindings(&clang_extra_include),
    )
    .expect("Couldn't write public types bindings!");
}
