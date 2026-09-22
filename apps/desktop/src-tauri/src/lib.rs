//! Agent OS desktop shell (Tauri v2).
//!
//! The window navigates to the running `agentgw` HTTP gateway — the
//! shell is a thin native wrapper, so all UI/logic stays in the shared
//! web app. `AGENTOS_GATEWAY` overrides the URL (default
//! `http://127.0.0.1:7740`); set it to a reachable gateway — local or
//! remote — before launching, or to a `tauri dev` frontend URL.

use tauri::{Manager, WebviewUrl, WebviewWindowBuilder};

const DEFAULT_GATEWAY: &str = "http://127.0.0.1:7740";

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .setup(|app| {
            let gateway = std::env::var("AGENTOS_GATEWAY")
                .ok()
                .filter(|v| !v.trim().is_empty())
                .unwrap_or_else(|| DEFAULT_GATEWAY.to_owned());
            let url = gateway
                .parse()
                .map_err(|e| format!("invalid AGENTOS_GATEWAY `{gateway}`: {e}"))?;
            WebviewWindowBuilder::new(app, "main", WebviewUrl::External(url))
                .title("Agent OS")
                .inner_size(1280.0, 840.0)
                .min_inner_size(900.0, 600.0)
                .build()?;
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("agent-os desktop failed to start");
}
