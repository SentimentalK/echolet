use std::env;
use std::process::Command;

pub fn register_gnome_shortcut() {
    if let Ok(exe_path) = env::current_exe() {
        let exe_str = exe_path.to_string_lossy();
        let cmd = format!("{} toggle", exe_str);

        let _ = Command::new("gsettings")
            .args([
                "set",
                "org.gnome.settings-daemon.plugins.media-keys",
                "custom-keybindings",
                "['/org/gnome/settings-daemon/plugins/media-keys/custom-keybindings/voice-toggle/']",
            ])
            .output();

        let _ = Command::new("gsettings")
            .args([
                "set",
                "org.gnome.settings-daemon.plugins.media-keys.custom-keybinding:/org/gnome/settings-daemon/plugins/media-keys/custom-keybindings/voice-toggle/",
                "name",
                "Voice Input Toggle",
            ])
            .output();

        let _ = Command::new("gsettings")
            .args([
                "set",
                "org.gnome.settings-daemon.plugins.media-keys.custom-keybinding:/org/gnome/settings-daemon/plugins/media-keys/custom-keybindings/voice-toggle/",
                "command",
                &cmd,
            ])
            .output();

        let _ = Command::new("gsettings")
            .args([
                "set",
                "org.gnome.settings-daemon.plugins.media-keys.custom-keybinding:/org/gnome/settings-daemon/plugins/media-keys/custom-keybindings/voice-toggle/",
                "binding",
                "F10",
            ])
            .output();

        println!("[Hotkey] Global shortcut F10 registered via GNOME: `{}`", cmd);
    }
}
