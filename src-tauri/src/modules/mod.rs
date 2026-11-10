pub mod agent;
pub mod agent_events;
pub mod agent_json;
pub mod ai_sessions;
#[cfg(target_os = "macos")]
pub mod app_menu;
#[cfg(feature = "bench")]
pub mod bench;
pub mod control;
// Only macOS registers the terax:// scheme (Info.plist); see focus_link.rs.
#[cfg(any(target_os = "macos", test))]
pub mod focus_link;
pub mod fs;
pub mod git;
pub mod history;
pub mod ipc_body;
pub mod lsp;
pub mod net;
pub mod proc;
pub mod pty;
pub mod runtime;
pub mod secrets;
pub mod shell;
pub mod vibrancy;
pub mod window_presentation;
pub mod workspace;
