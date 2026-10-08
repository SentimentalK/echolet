plugins {
    id("com.android.application")
}

android {
    namespace = "com.mainstayx.echolet"
    compileSdk = 37
    buildToolsVersion = "36.0.0"

    defaultConfig {
        applicationId = "com.mainstayx.echolet"
        minSdk = 26
        targetSdk = 36
        versionCode = 1
        versionName = "0.1.0"

        // Phase 0-A feasibility: the native slice is built and validated for
        // arm64-v8a only (libecholet_android.so, libsherpa-onnx-c-api.so,
        // libonnxruntime.so under build/generated/jniLibs/arm64-v8a).
        ndk {
            abiFilters += listOf("arm64-v8a")
        }
    }

    compileOptions {
        sourceCompatibility = JavaVersion.VERSION_17
        targetCompatibility = JavaVersion.VERSION_17
    }
}

// Native .so binaries are staged by android/scripts/build-native-arm64.sh
// (cargo-ndk + pinned sherpa-onnx C API); they are build outputs, never
// committed sources. AGP 9 requires the Sources variant API for generated
// source directories.
androidComponents {
    onVariants { variant ->
        variant.sources.jniLibs?.addStaticSourceDirectory("build/generated/jniLibs")
    }
}
