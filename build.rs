use std::env;
use std::path::PathBuf;

fn main() {
    println!("cargo:rerun-if-env-changed=ECHOLET_NATIVE_LIB_DIR");
    println!("cargo:rerun-if-env-changed=ECHOLET_BUNDLE_BUILD");

    let manifest_dir = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap());
    let target_os = env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();

    // 1. Determine native library directory
    let native_lib_dir = if let Ok(custom_dir) = env::var("ECHOLET_NATIVE_LIB_DIR") {
        PathBuf::from(custom_dir)
    } else {
        manifest_dir.join(".local-runtime/runtime/lib")
    };

    // 2. Validate that required native shared libraries exist
    let lib_name = if target_os == "macos" {
        "libsherpa-onnx-c-api.dylib"
    } else {
        "libsherpa-onnx-c-api.so"
    };

    let sherpa_c_api = native_lib_dir.join(lib_name);
    if !sherpa_c_api.exists() {
        let prep_script = if target_os == "macos" {
            "./scripts/macos/prepare-assets.sh"
        } else {
            "./scripts/prepare-local-assets.sh"
        };
        panic!(
            "\n========================================================================\n\
             [Build Error] Echolet native runtime not found at:\n  {:?}\n\n\
             Please run the local asset preparation script first:\n\
               {}\n\n\
             Or specify a custom native library directory:\n\
               export ECHOLET_NATIVE_LIB_DIR=/path/to/runtime/lib\n\
             ========================================================================\n",
            native_lib_dir, prep_script
        );
    }

    // 3. Link against shared libraries
    println!("cargo:rustc-link-search=native={}", native_lib_dir.display());
    println!("cargo:rustc-link-lib=dylib=sherpa-onnx-c-api");
    println!("cargo:rustc-link-lib=dylib=onnxruntime");

    // 4. Inject RPATH & OS-specific link flags:
    let is_bundle_build = env::var("ECHOLET_BUNDLE_BUILD")
        .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
        .unwrap_or(false);

    if target_os == "macos" {
        println!("cargo:rustc-link-lib=framework=Carbon");
        println!("cargo:rustc-link-lib=framework=Cocoa");
        println!("cargo:rustc-link-lib=framework=CoreGraphics");
        println!("cargo:rustc-link-lib=framework=CoreFoundation");
        println!("cargo:rustc-link-lib=framework=ApplicationServices");

        if is_bundle_build {
            println!("cargo:rustc-link-arg=-Wl,-rpath,@executable_path/../Frameworks");
        } else {
            println!("cargo:rustc-link-arg=-Wl,-rpath,@executable_path/../Frameworks");
            println!("cargo:rustc-link-arg=-Wl,-rpath,{}", native_lib_dir.display());
            println!("cargo:rustc-link-arg=-Wl,-rpath,@loader_path/../../.local-runtime/runtime/lib");
            println!("cargo:rustc-link-arg=-Wl,-rpath,@loader_path/../../../.local-runtime/runtime/lib");
        }
    } else if target_os == "linux" {
        if is_bundle_build {
            println!("cargo:rustc-link-arg=-Wl,-rpath,$ORIGIN/runtime/lib");
        } else {
            println!("cargo:rustc-link-arg=-Wl,-rpath,$ORIGIN/runtime/lib");
            println!("cargo:rustc-link-arg=-Wl,-rpath,$ORIGIN/../runtime/lib");
            println!("cargo:rustc-link-arg=-Wl,-rpath,$ORIGIN/../../.local-runtime/runtime/lib");
            println!("cargo:rustc-link-arg=-Wl,-rpath,$ORIGIN/../../../.local-runtime/runtime/lib");
        }
        println!("cargo:rustc-link-arg=-Wl,-z,origin");
    }
}
