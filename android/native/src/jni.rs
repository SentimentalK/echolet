//! Thin JNI translation layer: argument validation, exception mapping and
//! panic fencing only. All session/ASR logic lives in [`crate::runtime`].
//!
//! Extern conventions match the static native methods declared on
//! `com.mainstayx.echolet.NativeBridge`:
//! `Java_com_mainstayx_echolet_NativeBridge_nativeOpen` etc.

use jni::objects::{JClass, JFloatArray, JString};
use jni::sys::{jfloat, jint, jlong};
use jni::JNIEnv;

use crate::runtime::{AndroidRuntime, BridgeError};

/// Runs `f` with the process runtime MUTEX HELD and panics fenced, so no
/// unwind can cross the `extern "system"` boundary. `f` receives the locked
/// runtime directly (it must NOT lock again); a panic or failure is reported
/// as a descriptive Java exception and `fallback` is returned.
fn with_locked_runtime<'env, T, F>(
    env: &mut JNIEnv<'env>,
    operation: &'static str,
    fallback: T,
    f: F,
) -> T
where
    F: FnOnce(&mut AndroidRuntime, &mut JNIEnv<'env>) -> Result<T, BridgeError>,
{
    let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let mutex = crate::runtime::shared_runtime();
        let mut guard = mutex
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        f(&mut guard, env)
    }));
    match outcome {
        Ok(Ok(value)) => value,
        Ok(Err(error)) => {
            report_error(env, operation, &error);
            fallback
        }
        Err(_) => {
            env.throw_new(
                "java/lang/IllegalStateException",
                format!("{} abandoned: native runtime panicked; state re-locked unchanged", operation),
            )
            .ok();
            fallback
        }
    }
}

/// Maps bridge failures to the Kotlin exception contract.
fn report_error(env: &mut JNIEnv, operation: &'static str, error: &BridgeError) {
    let class = if error.is_input_error() {
        "java/lang/IllegalArgumentException"
    } else {
        "java/lang/IllegalStateException"
    };
    env.throw_new(class, format!("{} failed: {}", operation, error.message()))
        .ok();
}

#[no_mangle]
pub extern "system" fn Java_com_mainstayx_echolet_NativeBridge_nativeOpen<'local>(
    mut env: JNIEnv<'local>,
    _class: JClass<'local>,
    model_dir: JString<'local>,
) -> jlong {
    with_locked_runtime(&mut env, "nativeOpen", 0, |runtime, env| {
        // A zero handle is reserved as the failure fallback.
        let dir_path = env
            .get_string(&model_dir)
            .map(|s| {
                let path: String = s.to_string_lossy().into_owned();
                std::path::PathBuf::from(path)
            })
            .map_err(|e| BridgeError::Model(format!("invalid modelDir argument: {}", e)))?;
        runtime.open(&dir_path).map(|h| h as jlong)
    })
}

#[no_mangle]
pub extern "system" fn Java_com_mainstayx_echolet_NativeBridge_nativeFeed<'local>(
    mut env: JNIEnv<'local>,
    _class: JClass<'local>,
    handle: jlong,
    samples: JFloatArray<'local>,
    sample_rate: jint,
) -> JString<'local> {
    // Pre-allocate the `[]` fallback string; on allocation failure itself we
    // return a null reference (a valid object return when the exception is
    // already thrown).
    let empty = env.new_string("[]").ok();
    let result = with_locked_runtime(&mut env, "nativeFeed", empty, |runtime, env| {
        let copied: Vec<jfloat> = unsafe {
            env.get_array_elements(&samples, jni::objects::ReleaseMode::NoCopyBack)
        }
        .map(|elements| elements.to_vec())
        .map_err(|e| BridgeError::InvalidSamples {
                reason: format!("cannot read samples array: {}", e),
            })?;
        let events = runtime.feed(handle as u64, &copied, sample_rate as u32)?;
        match env.new_string(&events) {
            Ok(js) => Ok(Some(js)),
            Err(e) => Err(BridgeError::Owned(format!(
                "cannot allocate result string: {}",
                e
            ))),
        }
    });
    match result {
        Some(component) => component,
        None => {
            // The exception is already thrown; return null to stay in contract.
            unsafe { JString::from_raw(std::ptr::null_mut()) }
        }
    }
}

#[no_mangle]
pub extern "system" fn Java_com_mainstayx_echolet_NativeBridge_nativeClose<'local>(
    mut env: JNIEnv<'local>,
    _class: JClass<'local>,
    handle: jlong,
) {
    with_locked_runtime(&mut env, "nativeClose", (), |runtime, _env| {
        runtime.close(handle as u64);
        Ok(())
    })
}
