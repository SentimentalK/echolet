plugins {
    id("com.android.application")
}

// Required signing secrets are supplied exclusively through gradle
// properties (see android.yml and android/README.md); a missing secret is
// a hard configuration error, never a silent fall-back.
fun echoletReleaseProperty(name: String): String {
    val value = property(name) as? String
    return value ?: error("Missing required signing gradle property '$name'")
}

android {
    namespace = "com.mainstayx.echolet"
    compileSdk = 37
    buildToolsVersion = "36.0.0"

    defaultConfig {
        applicationId = "com.mainstayx.echolet"
        minSdk = 26
        targetSdk = 36
        versionCode = 2
        versionName = "2.0.0"

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

    val releaseStorePath = findProperty("echolet.release.storePath") as String?
    signingConfigs {
        if (releaseStorePath != null) {
            create("echoletRelease") {
                storeFile = file(releaseStorePath)
                storePassword = echoletReleaseProperty("echolet.release.storePassword")
                keyAlias = echoletReleaseProperty("echolet.release.keyAlias")
                val keyPass = findProperty("echolet.release.keyPassword") as? String
                keyPassword = if (!keyPass.isNullOrBlank()) keyPass else storePassword
            }
        }
    }

    buildTypes {
        release {
            // STABLE user-owned signing identity only: configured via the
            // gradle properties above when the CI/release caller supplies
            // them. There is deliberately NO debug-signing fallback for
            // release packages; without signing properties an unsigned APK
            // is produced and the release pipeline's verification gate
            // fails it (never published).
            if (releaseStorePath != null) {
                signingConfig = signingConfigs.getByName("echoletRelease")
            }
        }
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
