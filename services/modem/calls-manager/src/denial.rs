//! What calls-manager reads from Denial's settings: the calls app and the
//! language.

use std::path::PathBuf;

use serde_json::Value;

const DEFAULT_CALLS_APP: &str = "org.denialwm.Calls";

/// Denial's settings document, read afresh; Null when there is none.
pub fn settings() -> Value {
    std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".config")))
        .map(|dir| dir.join("denial/settings.json"))
        .and_then(|path| std::fs::read(path).ok())
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .unwrap_or(Value::Null)
}

/// The calls app the user set, by its desktop entry's id: Denial's
/// `lockScreen.callsApp`, the one app Denial shows over the lock screen
/// while its call notification stands.
pub fn calls_app(settings: &Value) -> String {
    settings["lockScreen"]["callsApp"]
        .as_str()
        .filter(|app| !app.is_empty())
        .unwrap_or(DEFAULT_CALLS_APP)
        .to_owned()
}
