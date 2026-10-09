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
        // libonnxruntime.so under .native-jniLibs/arm64-v8a, outside build/).
        ndk {
            abiFilters += listOf("arm64-v8a")
        }
    }

    compileOptions {
        sourceCompatibility = JavaVersion.VERSION_17
        targetCompatibility = JavaVersion.VERSION_17
    }

    // JVM mediation tests exercise the real controller with fake main/lane;
    // they need android.util.Log calls to return defaults instead of throwing.
    testOptions.unitTests.isReturnDefaultValues = true
}

// Pure-JVM projection tests (CodepointDiffBuffer), runnable without Android:
//   (cd android && ./gradlew :app:testDebugUnitTest)
dependencies {
    testImplementation("junit:junit:4.13.2")
    // Real org.json for JVM tests: the android.jar stub throws "not mocked"
    // when ProjectionReducer parses nativeFeed wire payloads.
    testImplementation("org.json:json:20240303")
}

// Native .so binaries are staged by android/scripts/build-native-arm64.sh
// (cargo-ndk + pinned sherpa-onnx C API) into app/.native-jniLibs/<abi>/;
// they are build outputs, never committed sources. The directory deliberately
// lives OUTSIDE app/build so `./gradlew clean :app:assembleDebug` preserves
// the staged set, and android/.gitignore ignores it. AGP 9 requires the
// Sources variant API for additional static source directories; the path is
// resolved relative to the app module root and contains ABI subdirectories.
androidComponents {
    onVariants { variant ->
        variant.sources.jniLibs?.addStaticSourceDirectory(".native-jniLibs")
    }
}
