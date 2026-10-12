//! iOS model-management C ABI (iPhoneOS device only).
//!
//! A minimal, panic-free bridge over the shared Echolet core model stack:
//! * catalog snapshot — canonical embedded registry + locally discovered
//!   installs (`echolet::models::ios_surface` projection; the Swift side never
//!   invents a duplicate catalog);
//! * download spec — only via registry lookup of a validated model id
//!   (HTTPS URL, trusted sha256 anchor, advertised sizes);
//! * install — the downloaded archive path is verified by Rust (SHA256 against
//!   the registry anchor), then extracted/committed atomically by the shared
//!   installer; at most one in-flight install per model;
//! * select — fail-closed persisted selection via
//!   `echolet::models::ios_surface::select_model_persisted`.
//!
//! Wire/ownership conventions (explicit):
//! * every function returning `*mut c_char` hands back a malloc'd, NUL
//!   terminated UTF-8 C string owned by the CALLER, who must release it with
//!   exactly one `echolet_ios_model_string_free`; you must ONLY pass pointers
//!   previously returned by this ABI;
//! * NULL return = failure; for manager-attached calls the failure reason is
//!   retrievable via `echolet_ios_model_error_json` (it is never a panic);
//! * the manager is NOT thread-safe; Swift must keep every call on one serial
//!   queue (the containing App model store does exactly that);
//! * no panics cross the C ABI (every body is `catch_unwind` guarded), no
//!   invented data, and no network calls exist downstream (transport is the
//!   Swift URLSession download coordinator only).

use echolet::models::progress::InstallPhase;
use echolet::models::ios_surface;
use echolet::models::manager::ModelManager;
use echolet::models::progress::ProgressCallback;
use std::collections::HashSet;
use std::os::raw::{c_char, c_int, c_void};
use std::panic::{catch_unwind, AssertUnwindSafe};

pub struct EcholetIosModelManager {
    manager: ModelManager,
    last_error: Option<String>,
    in_flight_install: HashSet<String>,
}

/// Externally callable install-progress hook (phase codes: 0 Starting,
/// 1 Verifying, 2 Extracting, 3 Installing, 4 Completed).
pub type EcholetIosInstallProgress =
    Option<unsafe extern "C" fn(user_data: *mut c_void, phase: c_int)>;

fn panic_guard<T>(default: T, f: impl FnOnce() -> T) -> T {
    catch_unwind(AssertUnwindSafe(f)).unwrap_or(default)
}

fn read_utf8(ptr: *const c_char) -> Option<String> {
    unsafe {
        if ptr.is_null() {
            return None;
        }
        std::ffi::CStr::from_ptr(ptr).to_str().ok().map(|s| s.to_string())
    }
}

/// Deduced bundled dir: intentionally a path that cannot exist so the manager
/// always falls back to the canonical EMBEDDED registry (no .app bundle model
/// resources in the product flow) and discovers only the App-private user dir.
fn no_bundled_dir() -> std::path::PathBuf {
    std::path::PathBuf::from("__echolet_no_bundled_models__")
}

fn owned_c_string(text: &str) -> *mut c_char {
    match std::ffi::CString::new(text.as_bytes().to_vec()) {
        Ok(cs) => cs.into_raw(),
        // Interior NULs cannot occur in our JSON output; keep the ABI total.
        Err(_) => std::ptr::null_mut(),
    }
}

// ---------------------------------------------------------------------------
// Manager lifecycle
// ---------------------------------------------------------------------------

/// Creates the model manager against App-private directories.
///
/// `user_models_dir` — Application Support/Echolet/models (Swift owns folder
/// creation and iCloud-backup exclusion). `config_path` — the persistent
/// selected-model config file path. NULL on failure (invalid UTF-8 or
/// registry/bootstrap failure).
#[no_mangle]
pub extern "C" fn echolet_ios_model_manager_new(
    user_models_dir: *const c_char,
    config_path: *const c_char,
) -> *mut EcholetIosModelManager {
    let dir = match read_utf8(user_models_dir) {
        Some(d) => d,
        None => return std::ptr::null_mut(),
    };
    let config = match read_utf8(config_path) {
        Some(c) => c,
        None => return std::ptr::null_mut(),
    };
    let result = panic_guard(None, || {
        ModelManager::new_with_paths(
            no_bundled_dir(),
            std::path::PathBuf::from(&dir),
            std::path::PathBuf::from(&config),
        )
        .ok()
    });
    match result {
        Some(manager) => Box::into_raw(Box::new(EcholetIosModelManager {
            manager,
            last_error: None,
            in_flight_install: HashSet::new(),
        })),
        None => std::ptr::null_mut(),
    }
}

/// Frees a manager handle. Safe with NULL.
#[no_mangle]
pub extern "C" fn echolet_ios_model_manager_free(handle: *mut EcholetIosModelManager) {
    if !handle.is_null() {
        unsafe {
            drop(Box::from_raw(handle));
        }
    }
}

/// Releases a malloc'd UTF-8 string returned by this ABI. Safe with NULL;
/// must be called exactly once per returned string; NULL result values from
/// model_manager_new failures have no string to release.
#[no_mangle]
pub extern "C" fn echolet_ios_model_string_free(string: *mut c_char) {
    if !string.is_null() {
        unsafe {
            drop(std::ffi::CString::from_raw(string));
        }
    }
}

// ---------------------------------------------------------------------------
// Snapshot / spec / selection / install
// ---------------------------------------------------------------------------

/// Full canonical model snapshot JSON:
/// `{schema_version, default_model_id, models: [{id, display_name, version,
/// languages, verification_status, download_size_bytes, installed_size_bytes,
/// runtime_compatible, installed, installed_dir, active}]}`.
/// Owned by the caller; NULL on failure.
#[no_mangle]
pub extern "C" fn echolet_ios_model_snapshot_json(
    handle: *mut EcholetIosModelManager,
) -> *mut c_char {
    let result = catch_unwind(AssertUnwindSafe(|| -> Option<String> {
        let mgr = unsafe { handle.as_mut()? };
        Some(ios_surface::catalog_snapshot_json(
            &mgr.manager.registry,
            &mgr.manager.installed,
            mgr.manager.active_model_id.as_deref(),
        ))
    }))
    .unwrap_or(None);
    match result {
        Some(json) => owned_c_string(&json),
        None => set_error(
            unsafe { handle.as_mut() },
            "Failed to build catalog snapshot",
        ),
    }
}

/// Canonical HTTPS download spec JSON for a validated model id
/// (`{model_id, url, sha256, download_size_bytes, installed_size_bytes}`).
/// NULL on failure (unknown id / bundled / non-HTTPS / missing sha256).
#[no_mangle]
pub extern "C" fn echolet_ios_model_download_spec_json(
    handle: *mut EcholetIosModelManager,
    model_id: *const c_char,
) -> *mut c_char {
    let id = match read_utf8(model_id) {
        Some(id) => id,
        None => return set_error(unsafe { handle.as_mut() }, "Invalid model id argument"),
    };
    let result = catch_unwind(AssertUnwindSafe(|| -> Result<String, String> {
        let mgr = match unsafe { handle.as_mut() } {
            Some(h) => h,
            None => return Err("manager unavailable".to_string()),
        };
        let entry = mgr
            .manager
            .registry
            .get_model(&id)
            .cloned()
            .ok_or_else(|| format!("Unknown model ID: {}", id))?;
        ios_surface::download_spec_json(&entry)
    }))
    .unwrap_or(Err("panic during download_spec".to_string()));
    match result {
        Ok(json) => owned_c_string(&json),
        Err(e) => set_error(unsafe { handle.as_mut() }, &e),
    }
}

/// Last recorded error for this manager as `{"error": "..."}` JSON (owned by
/// the caller). Reading does not clear the error. NULL when none recorded.
#[no_mangle]
pub extern "C" fn echolet_ios_model_error_json(
    handle: *mut EcholetIosModelManager,
) -> *mut c_char {
    let text = catch_unwind(AssertUnwindSafe(|| -> Option<String> {
        let mgr = unsafe { handle.as_mut()? };
        mgr.last_error.clone()
    }))
    .unwrap_or(None);
    match text {
        Some(e) => {
            let payload = serde_json::json!({ "error": e });
            let rendered = serde_json::to_string(&payload)
                .unwrap_or_else(|_| "{\"error\":\"internal\"}".to_string());
            owned_c_string(&rendered)
        }
        None => std::ptr::null_mut(),
    }
}

fn set_error(maybe: Option<&mut EcholetIosModelManager>, message: &str) -> *mut c_char {
    if let Some(mgr) = maybe {
        mgr.last_error = Some(message.to_string());
    }
    std::ptr::null_mut()
}

fn set_error_int(maybe: Option<&mut EcholetIosModelManager>, message: &str) -> c_int {
    if let Some(mgr) = maybe {
        mgr.last_error = Some(message.to_string());
    }
    -1
}

/// Re-scan the App-private install dir and re-resolve the active model
/// (relaunch/self-heal path). Returns 0 on success, -1 with error recorded.
#[no_mangle]
pub extern "C" fn echolet_ios_model_refresh(handle: *mut EcholetIosModelManager) -> c_int {
    catch_unwind(AssertUnwindSafe(|| {
        if let Some(mgr) = unsafe { handle.as_mut() } {
            mgr.manager.discover_installed();
            mgr.manager.resolve_active_model();
            return 0;
        }
        -1
    }))
    .unwrap_or(-1)
}

/// Rust-verified safe install of a downloaded archive.
///
/// Verifies the archive SHA256 against the trusted REGISTRY anchor, extracts
/// through the shared safe unpacker, validates the manifest, and atomically
/// commits into `models_dir/<model_id>`. At most one in-flight install per
/// model; duplicate calls fail with a clear error. Returns 0 on success; on
/// failure the previously installed model (if any) remains intact, staging is
/// cleaned, and the error is recorded.
#[no_mangle]
pub extern "C" fn echolet_ios_model_install_archive(
    handle: *mut EcholetIosModelManager,
    model_id: *const c_char,
    archive_path: *const c_char,
    progress: EcholetIosInstallProgress,
    user_data: *mut c_void,
) -> c_int {
    let id = match read_utf8(model_id) {
        Some(id) if !id.is_empty() => id,
        Some(_) => return set_error_int(unsafe { handle.as_mut() },"Empty model id"),
        None => return set_error_int(unsafe { handle.as_mut() },"Invalid model id argument"),
    };
    let archive = match read_utf8(archive_path) {
        Some(a) if !a.is_empty() => a,
        _ => return set_error_int(unsafe { handle.as_mut() },"Invalid archive path argument"),
    };

    let outcome = catch_unwind(AssertUnwindSafe(|| -> Result<(), String> {
        let mgr = match unsafe { handle.as_mut() } {
            Some(h) => h,
            None => return Err("manager unavailable".to_string()),
        };
        // Registry validation BEFORE any disk work (unknown / traversal ids can
        // never match a canonical entry).
        if mgr.manager.registry.get_model(&id).is_none() {
            return Err(format!("Unknown model ID: {}", id));
        }
        if mgr.in_flight_install.contains(&id) {
            return Err(format!("Install already in flight for model {}", id));
        }
        mgr.in_flight_install.insert(id.clone());
        mgr.last_error = None;
        Ok(())
    }))
    .unwrap_or(Err("panic before install".to_string()));
    if let Err(e) = outcome {
        return set_error_int(unsafe { handle.as_mut() },&e);
    }

    if let Some(mgr) = unsafe { handle.as_mut() } {
        let _ = mgr.in_flight_install.insert(id.clone());
    }

    let models_dir = match unsafe { handle.as_ref() } {
        Some(h) => h.manager.user_models_dir.clone(),
        None => return set_error_int(unsafe { handle.as_mut() },"manager unavailable"),
    };
    let registry = match unsafe { handle.as_ref() } {
        Some(h) => h.manager.registry.clone(),
        None => return set_error_int(unsafe { handle.as_mut() },"manager unavailable"),
    };

    fn call_install(
        registry: &echolet::models::registry::ModelRegistry,
        id: &str,
        archive: &str,
        models_dir: &std::path::Path,
        progress: Option<ProgressCallback<'_>>,
    ) -> Result<echolet::models::manifest::ModelManifest, String> {
        ios_surface::install_model_archive_validated(
            registry,
            id,
            std::path::Path::new(archive),
            models_dir,
            progress,
        )
    }

    let result = match progress {
        Some(handler) => {
            catch_unwind(AssertUnwindSafe(|| {
                let mut wrapper = |phase: InstallPhase| unsafe { handler(user_data, phase_code(phase)) };
                call_install(&registry, &id, &archive, &models_dir, Some(&mut wrapper))
            }))
            .unwrap_or_else(|_| Err("panic during install".to_string()))
        }
        None => catch_unwind(AssertUnwindSafe(|| {
            call_install(&registry, &id, &archive, &models_dir, None)
        }))
        .unwrap_or_else(|_| Err("panic during install".to_string())),
    };

    catch_unwind(AssertUnwindSafe(|| {
        if let Some(mgr) = unsafe { handle.as_mut() } {
            mgr.in_flight_install.remove(&id);
            match &result {
                Ok(_) => {
                    // Re-discover from disk so the snapshot reflects real truth.
                    // The active model is intentionally NOT changed by install.
                    mgr.manager.discover_installed();
                    mgr.manager.resolve_active_model();
                    mgr.last_error = None;
                }
                Err(e) => {
                    mgr.last_error = Some(e.clone());
                }
            }
        }
    }))
    .unwrap_or(());

    match result {
        Ok(_) => 0,
        Err(e) => set_error_int(unsafe { handle.as_mut() },&e),
    }
}

fn phase_code(phase: InstallPhase) -> c_int {
    match phase {
        InstallPhase::Verifying => 1,
        InstallPhase::Extracting => 2,
        InstallPhase::Installing => 3,
        InstallPhase::Completed => 4,
        InstallPhase::Starting | InstallPhase::Downloading { .. } => 0,
    }
}

/// Fail-closed persisted selection (registry-validated, save-checked, and the
/// previous active id is restored on save failure). Returns 0 on success,
/// -1 with the error recorded.
#[no_mangle]
pub extern "C" fn echolet_ios_model_select(
    handle: *mut EcholetIosModelManager,
    model_id: *const c_char,
) -> c_int {
    let id = match read_utf8(model_id) {
        Some(id) => id,
        None => return set_error_int(unsafe { handle.as_mut() },"Invalid model id argument"),
    };
    let result = catch_unwind(AssertUnwindSafe(|| -> Result<(), String> {
        let mgr = match unsafe { handle.as_mut() } {
            Some(h) => h,
            None => return Err("manager unavailable".to_string()),
        };
        match ios_surface::select_model_persisted(&mut mgr.manager, &id) {
            Ok(()) => {
                mgr.last_error = None;
                Ok(())
            }
            Err(e) => Err(e),
        }
    }))
    .unwrap_or(Err("panic during select".to_string()));
    match result {
        Ok(()) => 0,
        Err(e) => set_error_int(unsafe { handle.as_mut() },&e),
    }
}

/// Absolute directory of the active installed model, or NULL when none.
/// Owned by the caller (`echolet_ios_model_string_free`).
#[no_mangle]
pub extern "C" fn echolet_ios_model_active_dir(
    handle: *mut EcholetIosModelManager,
) -> *mut c_char {
    let dir = catch_unwind(AssertUnwindSafe(|| -> Option<String> {
        let mgr = unsafe { handle.as_mut()? };
        Some(
            ios_surface::active_model_dir(&mgr.manager)?
                .to_string_lossy()
                .into_owned(),
        )
    }))
    .unwrap_or(None);
    match dir {
        Some(d) => owned_c_string(&d),
        None => std::ptr::null_mut(),
    }
}
