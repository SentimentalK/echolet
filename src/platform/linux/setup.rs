use std::fs;
use std::path::Path;
use std::process::Command;

const UDEV_RULE_PATH: &str = "/etc/udev/rules.d/70-echolet-uinput.rules";
const UDEV_RULE_CONTENT: &str =
    "KERNEL==\"uinput\", SUBSYSTEM==\"misc\", TAG+=\"uaccess\", OPTIONS+=\"static_node=uinput\"\n";

/// Hardened privileged setup handler for `echolet setup-uinput`.
/// Only executed under root via pkexec. Attack surface is strictly bounded:
/// - Checks euid == 0
/// - Accepts zero dynamic parameters
/// - Writes only to fixed /etc/udev/rules.d/70-echolet-uinput.rules
/// - Reloads and settles udev rules, then exits immediately.
pub fn handle_setup_uinput_subcommand() -> Result<(), Box<dyn std::error::Error>> {
    let euid = unsafe { libc::geteuid() };
    if euid != 0 {
        return Err("[Setup] Permission denied: `setup-uinput` must be executed as root via pkexec.".into());
    }

    println!("[Setup] Installing Echolet uinput uaccess udev rule...");

    let rule_path = Path::new(UDEV_RULE_PATH);
    if let Some(parent) = rule_path.parent() {
        fs::create_dir_all(parent)?;
    }

    fs::write(rule_path, UDEV_RULE_CONTENT)?;
    println!("[Setup] Wrote udev rule to: {}", UDEV_RULE_PATH);

    // 1. Ensure kernel module is loaded
    let _ = Command::new("modprobe").arg("uinput").output();

    // 2. Reload udev rules
    let reload = Command::new("udevadm")
        .args(["control", "--reload-rules"])
        .output();
    if let Err(e) = reload {
        eprintln!("[Setup] Warning: udevadm reload-rules failed: {}", e);
    }

    // 3. Trigger uinput device
    let trigger = Command::new("udevadm")
        .args(["trigger", "--name-match=uinput"])
        .output();
    if let Err(e) = trigger {
        eprintln!("[Setup] Warning: udevadm trigger failed: {}", e);
    }

    // 4. Settle udev events
    let _ = Command::new("udevadm").arg("settle").output();

    println!("[Setup] Echolet uinput permissions configured successfully.");
    Ok(())
}
