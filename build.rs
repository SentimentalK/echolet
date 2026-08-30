use std::env;
use std::path::PathBuf;

fn main() {
    println!("cargo:rerun-if-env-changed=ECHOLET_NATIVE_LIB_DIR");

    let manifest_dir = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap());

    // 1. Determine native library directory
    let native_lib_dir = if let Ok(custom_dir) = env::var("ECHOLET_NATIVE_LIB_DIR") {
        PathBuf::from(custom_dir)
    } else {
        manifest_dir.join(".local-runtime/runtime/lib")
    };

    // 2. Validate that required native shared libraries exist
    let sherpa_c_api = native_lib_dir.join("libsherpa-onnx-c-api.so");
    if !sherpa_c_api.exists() {
        panic!(
            "\n========================================================================\n\
             [Build Error] Echolet native runtime not found at:\n  {:?}\n\n\
             Please run the local asset preparation script first:\n\
               ./scripts/prepare-local-assets.sh\n\n\
             Or specify a custom native library directory:\n\
               export ECHOLET_NATIVE_LIB_DIR=/path/to/runtime/lib\n\
             ========================================================================\n",
            native_lib_dir
        );
    }

    // 3. Link against shared libraries
    println!("cargo:rustc-link-search=native={}", native_lib_dir.display());
    println!("cargo:rustc-link-lib=dylib=sherpa-onnx-c-api");
    println!("cargo:rustc-link-lib=dylib=onnxruntime");

    // 4. Inject relative RPATH ($ORIGIN-based) so the binary finds runtime/lib portably:
    // - Production Bundle: $ORIGIN/runtime/lib
    // - Dev Binary (target/release/echolet): $ORIGIN/../../.local-runtime/runtime/lib
    // - Dev Tests (target/release/deps/test_*): $ORIGIN/../../../.local-runtime/runtime/lib
    println!("cargo:rustc-link-arg=-Wl,-rpath,$ORIGIN/runtime/lib");
    println!("cargo:rustc-link-arg=-Wl,-rpath,$ORIGIN/../runtime/lib");
    println!("cargo:rustc-link-arg=-Wl,-rpath,$ORIGIN/../../.local-runtime/runtime/lib");
    println!("cargo:rustc-link-arg=-Wl,-rpath,$ORIGIN/../../../.local-runtime/runtime/lib");
    println!("cargo:rustc-link-arg=-Wl,-z,origin");
}
