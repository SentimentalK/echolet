use std::env;
use std::process::Command;

const ECHOLET_BINDING_PATH: &str =
    "/org/gnome/settings-daemon/plugins/media-keys/custom-keybindings/echolet-toggle/";

pub fn register_gnome_shortcut() {
    let exe_path = match env::current_exe() {
        Ok(p) => p,
        Err(_) => return,
    };

    let exe_str = exe_path.to_string_lossy();
    let cmd = format!("{} toggle", exe_str);

    // 1. Read existing custom keybindings array to perform idempotent merge
    let current_bindings_output = Command::new("gsettings")
        .args([
            "get",
            "org.gnome.settings-daemon.plugins.media-keys",
            "custom-keybindings",
        ])
        .output();

    if let Ok(output) = current_bindings_output {
        let output_str = String::from_utf8_lossy(&output.stdout).trim().to_string();

        let mut entries: Vec<String> = Vec::new();
        // Parse format like: ['/path/1/', '/path/2/'] or @as []
        if output_str.starts_with('[') && output_str.ends_with(']') {
            let inner = &output_str[1..output_str.len() - 1];
            for part in inner.split(',') {
                let clean = part.trim().trim_matches('\'').trim();
                if !clean.is_empty() {
                    entries.push(clean.to_string());
                }
            }
        }

        // Add our binding if not present
        if !entries.contains(&ECHOLET_BINDING_PATH.to_string()) {
            entries.push(ECHOLET_BINDING_PATH.to_string());

            let formatted_array = format!(
                "[{}]",
                entries
                    .iter()
                    .map(|s| format!("'{}'", s))
                    .collect::<Vec<_>>()
                    .join(", ")
            );

            let _ = Command::new("gsettings")
                .args([
                    "set",
                    "org.gnome.settings-daemon.plugins.media-keys",
                    "custom-keybindings",
                    &formatted_array,
                ])
                .output();
        }
    }

    // 2. Refresh/update our specific custom keybinding with current executable path
    let schema = format!(
        "org.gnome.settings-daemon.plugins.media-keys.custom-keybinding:{}",
        ECHOLET_BINDING_PATH
    );

    let _ = Command::new("gsettings")
        .args(["set", &schema, "name", "Echolet Voice Input"])
        .output();

    let _ = Command::new("gsettings")
        .args(["set", &schema, "command", &cmd])
        .output();

    let _ = Command::new("gsettings")
        .args(["set", &schema, "binding", "F10"])
        .output();

    println!("[Hotkey] Global shortcut F10 registered via GNOME: `{}`", cmd);
}
