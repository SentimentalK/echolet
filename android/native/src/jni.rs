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

fn with_locked_model_owner<'env, T, F>(
    env: &mut JNIEnv<'env>,
    operation: &'static str,
    fallback: T,
    f: F,
) -> T
where
    F: FnOnce(&mut crate::model_owner::AndroidModelOwner, &mut JNIEnv<'env>) -> Result<T, String>,
{
    let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let mutex = crate::model_owner::shared_model_owner();
        let mut guard = mutex
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        f(&mut guard, env)
    }));
    match outcome {
        Ok(Ok(value)) => value,
        Ok(Err(err)) => {
            env.throw_new(
                "java/lang/IllegalStateException",
                format!("{} failed: {}", operation, err),
            )
            .ok();
            fallback
        }
        Err(_) => {
            env.throw_new(
                "java/lang/IllegalStateException",
                format!("{} abandoned: native model owner panicked", operation),
            )
            .ok();
            fallback
        }
    }
}

#[no_mangle]
pub extern "system" fn Java_com_mainstayx_echolet_NativeBridge_nativeInitModelManager<'local>(
    mut env: JNIEnv<'local>,
    _class: JClass<'local>,
    models_dir: JString<'local>,
    files_dir: JString<'local>,
) {
    with_locked_model_owner(&mut env, "nativeInitModelManager", (), |owner, env| {
        let m_dir = env
            .get_string(&models_dir)
            .map(|s| std::path::PathBuf::from(s.to_string_lossy().into_owned()))
            .map_err(|e| format!("invalid modelsDir: {}", e))?;
        let f_dir = env
            .get_string(&files_dir)
            .map(|s| std::path::PathBuf::from(s.to_string_lossy().into_owned()))
            .map_err(|e| format!("invalid filesDir: {}", e))?;
        owner.initialize(m_dir, f_dir);
        Ok(())
    })
}

#[no_mangle]
pub extern "system" fn Java_com_mainstayx_echolet_NativeBridge_nativeModelSnapshot<'local>(
    mut env: JNIEnv<'local>,
    _class: JClass<'local>,
) -> JString<'local> {
    let empty = env.new_string("{}").ok();
    let result = with_locked_model_owner(&mut env, "nativeModelSnapshot", empty, |owner, env| {
        let is_session_active = {
            let runtime_mutex = crate::runtime::shared_runtime();
            let runtime_guard = runtime_mutex
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            runtime_guard.session_active()
        };

        let runtime_state = if is_session_active {
            echolet::ui::control_surface::RuntimeState::Listening
        } else if owner.current_model_dir().is_none() {
            echolet::ui::control_surface::RuntimeState::NoModel
        } else {
            echolet::ui::control_surface::RuntimeState::Ready
        };

        let json = owner.build_snapshot_json(runtime_state)?;
        env.new_string(json)
            .map(Some)
            .map_err(|e| format!("Failed to create snapshot JString: {}", e))
    });
    match result {
        Some(s) => s,
        None => unsafe { JString::from_raw(std::ptr::null_mut()) },
    }
}

#[no_mangle]
pub extern "system" fn Java_com_mainstayx_echolet_NativeBridge_nativeSelectModel<'local>(
    mut env: JNIEnv<'local>,
    _class: JClass<'local>,
    model_id: JString<'local>,
) {
    with_locked_model_owner(&mut env, "nativeSelectModel", (), |owner, env| {
        let id_str = env
            .get_string(&model_id)
            .map(|s| s.to_string_lossy().into_owned())
            .map_err(|e| format!("invalid modelId: {}", e))?;

        let is_session_active = {
            let runtime_mutex = crate::runtime::shared_runtime();
            let runtime_guard = runtime_mutex
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            runtime_guard.session_active()
        };

        owner.select_model(&id_str, is_session_active)
    })
}

#[no_mangle]
pub extern "system" fn Java_com_mainstayx_echolet_NativeBridge_nativeInstallModelFromArchive<'local>(
    mut env: JNIEnv<'local>,
    _class: JClass<'local>,
    model_id: JString<'local>,
    archive_path: JString<'local>,
) {
    with_locked_model_owner(&mut env, "nativeInstallModelFromArchive", (), |owner, env| {
        let id_str = env
            .get_string(&model_id)
            .map(|s| s.to_string_lossy().into_owned())
            .map_err(|e| format!("invalid modelId: {}", e))?;
        let path_str = env
            .get_string(&archive_path)
            .map(|s| s.to_string_lossy().into_owned())
            .map_err(|e| format!("invalid archivePath: {}", e))?;

        owner.install_from_archive(&id_str, std::path::Path::new(&path_str))
    })
}

#[no_mangle]
pub extern "system" fn Java_com_mainstayx_echolet_NativeBridge_nativeSetDownloadProgress<'local>(
    mut env: JNIEnv<'local>,
    _class: JClass<'local>,
    model_id: JString<'local>,
    downloaded_bytes: jlong,
    total_bytes: jlong,
    phase: JString<'local>,
) {
    with_locked_model_owner(&mut env, "nativeSetDownloadProgress", (), |owner, env| {
        let id_str = env
            .get_string(&model_id)
            .map(|s| s.to_string_lossy().into_owned())
            .map_err(|e| format!("invalid modelId: {}", e))?;
        let phase_str = env
            .get_string(&phase)
            .map(|s| s.to_string_lossy().into_owned())
            .map_err(|e| format!("invalid phase: {}", e))?;

        let total_opt = if total_bytes > 0 {
            Some(total_bytes as u64)
        } else {
            None
        };

        let status = match phase_str.as_str() {
            "Starting" => echolet::models::progress::DownloadStatus::Starting,
            "Downloading" => echolet::models::progress::DownloadStatus::Downloading {
                downloaded_bytes: downloaded_bytes as u64,
                total_bytes: total_opt,
            },
            "Verifying" => echolet::models::progress::DownloadStatus::Verifying,
            "Extracting" => echolet::models::progress::DownloadStatus::Extracting,
            "Installing" => echolet::models::progress::DownloadStatus::Installing,
            "Completed" => echolet::models::progress::DownloadStatus::Completed,
            "Failed" => echolet::models::progress::DownloadStatus::Failed,
            _ => echolet::models::progress::DownloadStatus::NotDownloading,
        };

        owner.set_download_progress(&id_str, status);
        Ok(())
    })
}

#[no_mangle]
pub extern "system" fn Java_com_mainstayx_echolet_NativeBridge_nativeGetSelectedModelDir<'local>(
    mut env: JNIEnv<'local>,
    _class: JClass<'local>,
) -> JString<'local> {
    let result = with_locked_model_owner(&mut env, "nativeGetSelectedModelDir", None, |owner, env| {
        match owner.current_model_dir() {
            Some(p) => {
                let s = p.to_string_lossy().into_owned();
                env.new_string(s).map(Some).map_err(|e| e.to_string())
            }
            None => Ok(None),
        }
    });
    match result {
        Some(s) => s,
        None => unsafe { JString::from_raw(std::ptr::null_mut()) },
    }
}

