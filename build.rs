use std::env;
use std::path::PathBuf;

fn main() {
    println!("cargo:rerun-if-env-changed=ECHOLET_NATIVE_LIB_DIR");
    println!("cargo:rerun-if-env-changed=ECHOLET_IOS_LIB_DIR");
    println!("cargo:rerun-if-env-changed=ECHOLET_BUNDLE_BUILD");

    let manifest_dir = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap());
    let target_os = env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();

    // 1. Determine native library directory (iOS uses its own override because
    // the iPhoneOS staticlibs live in a separate ignored cache directory).
    let native_lib_dir = if target_os == "ios" {
        if let Ok(custom_dir) = env::var("ECHOLET_IOS_LIB_DIR") {
            PathBuf::from(custom_dir)
        } else {
            manifest_dir.join(".local-runtime/ios-native/lib")
        }
    } else if let Ok(custom_dir) = env::var("ECHOLET_NATIVE_LIB_DIR") {
        PathBuf::from(custom_dir)
    } else {
        manifest_dir.join(".local-runtime/runtime/lib")
    };

    // 2. Validate that required native shared libraries / import libraries exist
    let (lib_name, prep_script) = if target_os == "macos" {
        (
            "libsherpa-onnx-c-api.dylib",
            "./scripts/macos/prepare-assets.sh",
        )
    } else if target_os == "windows" {
        (
            "sherpa-onnx-c-api.lib",
            ".\\scripts\\windows\\prepare-assets.ps1",
        )
    } else if target_os == "android" {
        (
            "libsherpa-onnx-c-api.so",
            "./android/scripts/build-native-arm64.sh",
        )
    } else if target_os == "ios" {
        (
            "libsherpa-onnx-c-api.a",
            "./ios/scripts/build-ios-native.sh",
        )
    } else {
        (
            "libsherpa-onnx-c-api.so",
            "./scripts/prepare-local-assets.sh",
        )
    };

    let sherpa_c_api = native_lib_dir.join(lib_name);
    if !sherpa_c_api.exists() {
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
    println!(
        "cargo:rustc-link-search=native={}",
        native_lib_dir.display()
    );
    if target_os == "ios" {
        // iPhoneOS device: fully static C-API archive containing the pinned
        // Sherpa-onnx runtime plus its statically-linked onnxruntime, so the
        // app/extension targets need no extra dynamic library bundling.
        println!("cargo:rustc-link-lib=static=sherpa-onnx-c-api");
    } else {
        println!("cargo:rustc-link-lib=dylib=sherpa-onnx-c-api");
        println!("cargo:rustc-link-lib=dylib=onnxruntime");
    }

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
            println!(
                "cargo:rustc-link-arg=-Wl,-rpath,{}",
                native_lib_dir.display()
            );
            println!(
                "cargo:rustc-link-arg=-Wl,-rpath,@loader_path/../../.local-runtime/runtime/lib"
            );
            println!(
                "cargo:rustc-link-arg=-Wl,-rpath,@loader_path/../../../.local-runtime/runtime/lib"
            );
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
    } else if target_os == "android" {
        // Android: the shared sherpa/onnxruntime libraries ship beside
        // libecholet_android.so in the APK's jniLibs directory, so no RPATH is
        // needed. Slint compilation and Windows assets are skipped: the Slint
        // UI is desktop-only and the shared library is built with cargo-ndk.
    } else if target_os == "ios" {
        // iPhoneOS device: the static sherpa-onnx C-API archive embeds
        // onnxruntime; C++ objects inside the archive need the C++ runtime and
        // the iOS frameworks reach the Foundation symbols pulled by the
        // pinned build.
        println!("cargo:rustc-link-lib=c++");
        println!("cargo:rustc-link-lib=framework=Foundation");
        println!("cargo:rustc-link-lib=framework=CoreFoundation");
        println!("cargo:rustc-link-lib=framework=Accelerate");
    } else if target_os == "windows" {
        println!("cargo:rerun-if-changed=assets/windows/echolet.ico");
        let mut res = winresource::WindowsResource::new();
        res.set_icon("assets/windows/echolet.ico");
        res.compile()
            .expect("Failed to compile Windows application icon resource");
    }

    if target_os == "android" {
        // Slint is desktop-only; the Android build must not compile the
        // desktop panel UI definition.
    } else if target_os == "macos" || target_os == "linux" || target_os == "windows" {
        slint_build::compile("ui/desktop/EcholetPanel.slint")
            .expect("Failed to compile EcholetPanel.slint");
    }
}
