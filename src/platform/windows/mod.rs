use crate::actions::AppAction;
use crate::platform::PlatformRuntime;
use crossbeam_channel::Sender;

pub fn handle_subcommand(_args: &[String]) -> Result<bool, Box<dyn std::error::Error>> {
    Ok(false)
}

pub fn init(_action_tx: Sender<AppAction>) -> Result<Option<PlatformRuntime>, Box<dyn std::error::Error>> {
    Err("Windows platform adapter is not yet implemented in Phase 3".into())
}
