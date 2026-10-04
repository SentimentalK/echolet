#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ProcessRss {
    pub bytes: u64,
}

impl ProcessRss {
    pub fn new(bytes: u64) -> Self {
        Self { bytes }
    }

    pub fn bytes(&self) -> u64 {
        self.bytes
    }

    pub fn kib(&self) -> f64 {
        self.bytes as f64 / 1024.0
    }

    pub fn mib(&self) -> f64 {
        self.bytes as f64 / (1024.0 * 1024.0)
    }
}

/// Parses resident set size in pages from Linux `/proc/self/statm` content.
///
/// Linux `/proc/[pid]/statm` format:
/// `size resident shared text lib data dt`
/// Field 1 (0-indexed) is the resident pages count.
pub fn parse_statm_rss_pages(content: &str) -> Option<u64> {
    let mut parts = content.split_whitespace();
    let _vmsize_pages = parts.next()?;
    let rss_pages = parts.next()?;
    rss_pages.parse::<u64>().ok()
}

/// Parses resident set size from Linux `/proc/self/status` content.
/// Searches for `VmRSS:\t   <number> <unit>`.
pub fn parse_status_vm_rss_bytes(content: &str) -> Option<u64> {
    for line in content.lines() {
        if line.starts_with("VmRSS:") {
            let part = line.strip_prefix("VmRSS:")?.trim();
            let mut parts = part.split_whitespace();
            let val = parts.next()?.parse::<u64>().ok()?;
            let unit = parts.next().unwrap_or("kB");
            let multiplier: u64 = match unit {
                "kB" | "KB" | "kb" => 1024,
                "mB" | "MB" | "mb" => 1024 * 1024,
                "gB" | "GB" | "gb" => 1024 * 1024 * 1024,
                "B" | "b" => 1,
                _ => 1024,
            };
            return Some(val.saturating_mul(multiplier));
        }
    }
    None
}

/// Retrieves the current process resident set size (RSS).
///
/// - On Linux: reads `/proc/self/statm` multiplied by system page size,
///   falling back to `/proc/self/status` `VmRSS`.
/// - On macOS: queries Mach task basic info resident size.
/// - On other platforms (e.g. Windows in this J3): returns `None`.
pub fn get_current_rss() -> Option<ProcessRss> {
    #[cfg(target_os = "linux")]
    {
        if let Ok(statm) = std::fs::read_to_string("/proc/self/statm") {
            if let Some(pages) = parse_statm_rss_pages(&statm) {
                let page_size = {
                    let sz = unsafe { libc::sysconf(libc::_SC_PAGESIZE) };
                    if sz > 0 {
                        sz as u64
                    } else {
                        4096
                    }
                };
                return Some(ProcessRss::new(pages.saturating_mul(page_size)));
            }
        }

        if let Ok(status) = std::fs::read_to_string("/proc/self/status") {
            if let Some(bytes) = parse_status_vm_rss_bytes(&status) {
                return Some(ProcessRss::new(bytes));
            }
        }

        None
    }

    #[cfg(target_os = "macos")]
    {
        #[allow(deprecated)]
        unsafe {
            let mut info: libc::mach_task_basic_info = std::mem::zeroed();
            let mut count = (std::mem::size_of::<libc::mach_task_basic_info>()
                / std::mem::size_of::<libc::natural_t>())
                as libc::mach_msg_type_number_t;
            let kret = libc::task_info(
                libc::mach_task_self(),
                libc::MACH_TASK_BASIC_INFO,
                &mut info as *mut libc::mach_task_basic_info as libc::task_info_t,
                &mut count,
            );
            if kret == libc::KERN_SUCCESS {
                Some(ProcessRss::new(info.resident_size))
            } else {
                None
            }
        }
    }

    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_statm_rss_pages_standard() {
        let sample = "45281 12345 3421 150 0 8500 0\n";
        assert_eq!(parse_statm_rss_pages(sample), Some(12345));
    }

    #[test]
    fn test_parse_statm_rss_pages_whitespace_variations() {
        let sample = "  1000   2000   3000\t4000\n";
        assert_eq!(parse_statm_rss_pages(sample), Some(2000));
    }

    #[test]
    fn test_parse_statm_rss_pages_invalid() {
        assert_eq!(parse_statm_rss_pages(""), None);
        assert_eq!(parse_statm_rss_pages("123"), None);
        assert_eq!(parse_statm_rss_pages("abc def"), None);
    }

    #[test]
    fn test_parse_status_vm_rss_bytes_standard() {
        let sample = r#"
Name:	echolet
State:	S (sleeping)
VmPeak:	  184320 kB
VmSize:	  172032 kB
VmRSS:	   49152 kB
VmData:	   81920 kB
"#;
        assert_eq!(parse_status_vm_rss_bytes(sample), Some(49152 * 1024));
    }

    #[test]
    fn test_parse_status_vm_rss_bytes_missing_vmrss() {
        let sample = "Name: echolet\nState: S\nVmSize: 1000 kB\n";
        assert_eq!(parse_status_vm_rss_bytes(sample), None);
    }

    #[test]
    fn test_parse_status_vm_rss_bytes_units() {
        let sample_mb = "VmRSS: 64 mB\n";
        assert_eq!(parse_status_vm_rss_bytes(sample_mb), Some(64 * 1024 * 1024));

        let sample_b = "VmRSS: 4096 B\n";
        assert_eq!(parse_status_vm_rss_bytes(sample_b), Some(4096));
    }

    #[test]
    fn test_process_rss_conversions() {
        let rss = ProcessRss::new(1048576);
        assert_eq!(rss.bytes(), 1048576);
        assert_eq!(rss.kib(), 1024.0);
        assert_eq!(rss.mib(), 1.0);
    }

    #[test]
    fn test_get_current_rss_runs_on_supported_platforms() {
        let rss = get_current_rss();
        #[cfg(any(target_os = "linux", target_os = "macos"))]
        {
            assert!(rss.is_some(), "RSS should be present on Linux and macOS");
            assert!(rss.unwrap().bytes() > 0, "RSS must be > 0 bytes");
        }
        #[cfg(not(any(target_os = "linux", target_os = "macos")))]
        {
            let _ = rss;
        }
    }
}
