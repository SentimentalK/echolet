use serde::{Deserialize, Serialize};

/// Emit a `Downloading` progress event at most once per this many bytes.
pub const PROGRESS_BYTE_STEP: u64 = 256 * 1024;

/// Granularity of coalesced progress when the total size is unknown.
pub const PROGRESS_COALESCE_UNKNOWN_BYTES: u64 = 1024 * 1024;

/// Platform-neutral progress contract for a model download/install.
///
/// This is intentionally small so it can be projected onto a future Desktop /
/// Mobile UI without pulling UI dependencies into the core crate.
///
/// Byte progress is **globally monotonic across retries**:
/// `downloaded_bytes` is the cumulative number of bytes transferred for this
/// install, including bytes transferred by attempts that later failed. After a
/// partial transfer is retried it therefore continues from the previous value
/// instead of resetting to zero, and it may exceed `total_bytes` if one or more
/// retries occurred. `total_bytes` is the expected final object size (the
/// server's advertised `Content-Length`), or `None` when the server did not
/// advertise one; UI code should treat it as a target rather than a hard cap.
/// Consumers can divide `downloaded_bytes` by `total_bytes` only under the
/// assumption of no retries.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum InstallPhase {
    Starting,
    Downloading {
        downloaded_bytes: u64,
        total_bytes: Option<u64>,
    },
    Verifying,
    Extracting,
    Installing,
    Completed,
}

/// Coarse, platform-neutral projection of the download/install lifecycle for UI
/// surfaces. Unlike [`InstallPhase`] this is cheap to clone and carries the
/// terminal `Failed` state a tray/status row needs; it is the single shape the
/// app thread stores and the platform UI renders.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum DownloadStatus {
    /// No download is in flight for this model.
    NotDownloading,
    Starting,
    Downloading {
        downloaded_bytes: u64,
        total_bytes: Option<u64>,
    },
    Verifying,
    Extracting,
    Installing,
    Completed,
    /// The download/install ended in a failure and may be retried.
    Failed,
}

impl DownloadStatus {
    /// Projects a raw downloader phase into the UI status type.
    pub fn from_phase(phase: &InstallPhase) -> Self {
        match phase {
            InstallPhase::Starting => DownloadStatus::Starting,
            InstallPhase::Downloading {
                downloaded_bytes,
                total_bytes,
            } => DownloadStatus::Downloading {
                downloaded_bytes: *downloaded_bytes,
                total_bytes: *total_bytes,
            },
            InstallPhase::Verifying => DownloadStatus::Verifying,
            InstallPhase::Extracting => DownloadStatus::Extracting,
            InstallPhase::Installing => DownloadStatus::Installing,
            InstallPhase::Completed => DownloadStatus::Completed,
        }
    }

    /// Whether a download is actively in progress (used to disable conflicting
    /// UI actions such as selecting another model or starting a duplicate
    /// download).
    pub fn is_in_progress(&self) -> bool {
        matches!(
            self,
            DownloadStatus::Starting
                | DownloadStatus::Downloading { .. }
                | DownloadStatus::Verifying
                | DownloadStatus::Extracting
                | DownloadStatus::Installing
        )
    }
}

/// Coalesces raw downloader progress into bounded UI updates.
///
/// The downloader already emits `Downloading` at most once per
/// [`PROGRESS_BYTE_STEP`] plus the terminal event of each attempt; this filter
/// further reduces that to at most one update per whole percent (when the total
/// size is known) or per fixed byte bucket (when it is not), and passes through
/// every genuine phase transition exactly once. It is deliberately pure and
/// deterministic so the coalescing contract can be unit tested without a
/// network or clock.
#[derive(Debug, Default)]
pub struct ProgressThrottle {
    last: Option<DownloadStatus>,
}

impl ProgressThrottle {
    pub fn new() -> Self {
        Self { last: None }
    }

    /// Returns `true` when `status` should be projected to the UI, updating the
    /// internal last-emitted marker only when it does.
    pub fn should_emit(&mut self, status: DownloadStatus) -> bool {
        let emit = match &self.last {
            None => true,
            Some(prev) => progress_identity(prev) != progress_identity(&status),
        };
        if emit {
            self.last = Some(status);
        }
        emit
    }
}

/// A comparable identity for progress coalescing. `Downloading` collapses to a
/// percent (known total) or a coarse byte bucket (unknown total), so two
/// adjacent byte-level events within the same bucket compare equal.
fn progress_identity(status: &DownloadStatus) -> (u8, u64, Option<u8>) {
    match status {
        DownloadStatus::Downloading {
            downloaded_bytes,
            total_bytes,
        } => {
            let percent = total_bytes
                .filter(|t| *t > 0)
                .map(|t| (downloaded_bytes.saturating_mul(100) / t).min(100) as u8);
            let bucket = match percent {
                Some(_) => 0,
                None => downloaded_bytes / PROGRESS_COALESCE_UNKNOWN_BYTES,
            };
            (1, bucket, percent)
        }
        DownloadStatus::NotDownloading => (0, 0, None),
        DownloadStatus::Starting => (2, 0, None),
        DownloadStatus::Verifying => (3, 0, None),
        DownloadStatus::Extracting => (4, 0, None),
        DownloadStatus::Installing => (5, 0, None),
        DownloadStatus::Completed => (6, 0, None),
        DownloadStatus::Failed => (7, 0, None),
    }
}

/// Callback type accepted by the install APIs.
pub type ProgressCallback<'a> = &'a mut dyn FnMut(InstallPhase);

/// Tar-based archive formats the downloader can install.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ArchiveFormat {
    TarZstd,
    TarBzip2,
    Tar,
}

impl ArchiveFormat {
    /// Best-effort format hint derived from a URL path (query/fragment ignored).
    pub fn from_url(url: &str) -> Option<Self> {
        let path = url
            .split(['?', '#'])
            .next()
            .unwrap_or(url)
            .to_ascii_lowercase();
        if path.ends_with(".tar.zst") || path.ends_with(".tar.zstd") || path.ends_with(".tzst") {
            Some(ArchiveFormat::TarZstd)
        } else if path.ends_with(".tar.bz2") || path.ends_with(".tbz2") || path.ends_with(".tbz") {
            Some(ArchiveFormat::TarBzip2)
        } else if path.ends_with(".tar") {
            Some(ArchiveFormat::Tar)
        } else {
            None
        }
    }
}
