fn main() {
    let sherpa_lib_dir = "/home/sentimentalk/sherpa-onnx/build-shared/lib";
    let ort_lib_dir = "/home/sentimentalk/sherpa-onnx/build-shared/_deps/onnxruntime-src/lib";

    println!("cargo:rustc-link-search=native={}", sherpa_lib_dir);
    println!("cargo:rustc-link-search=native={}", ort_lib_dir);
    println!("cargo:rustc-link-lib=dylib=sherpa-onnx-c-api");
    println!("cargo:rustc-link-lib=dylib=onnxruntime");

    // Set rpath so runtime execution does not require LD_LIBRARY_PATH
    println!("cargo:rustc-link-arg=-Wl,-rpath,{}", sherpa_lib_dir);
    println!("cargo:rustc-link-arg=-Wl,-rpath,{}", ort_lib_dir);
}
