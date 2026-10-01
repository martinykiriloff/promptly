//! Promptly: a terminal whose first-class citizen is a Claude Code session.

mod app;
mod charts;
mod composer;
mod events;
mod fonts;
mod input;
mod notify;
mod pty;
mod review;
mod session;
mod shell_integration;
#[cfg(test)]
mod shots;
mod term_view;
mod theme;
mod ui_kit;

/// `CLAUDE_CONFIG_DIR` Promptly was launched with, if any. It is removed from
/// Promptly's own environment so each session gets exactly its account's
/// value (and the default account gets none).
pub static INHERITED_CONFIG_DIR: std::sync::OnceLock<std::path::PathBuf> =
    std::sync::OnceLock::new();

const INHERITED_CLAUDE_MARKERS: &[&str] = &[
    "CLAUDECODE",
    "CLAUDE_PID",
    "CLAUDE_EFFORT",
    "CLAUDE_CODE_ENTRYPOINT",
    "CLAUDE_CODE_EXECPATH",
    "CLAUDE_CODE_SESSION_ID",
    "CLAUDE_CODE_CHILD_SESSION",
    "CLAUDE_CODE_SESSION_ATTENDED",
    "CLAUDE_CODE_MESSAGING_SOCKET",
    "CLAUDE_CODE_MESSAGING_TOKEN",
    "CLAUDE_CODE_SSE_PORT",
];

fn main() -> eframe::Result<()> {
    if let Some(d) = std::env::var_os(promptly_core::accounts::CONFIG_DIR_ENV) {
        let _ = INHERITED_CONFIG_DIR.set(d.into());
    }
    // Children must not inherit a stale TERM from whatever launched us.
    // SAFETY: called before any threads are spawned.
    unsafe {
        std::env::remove_var("TERM_PROGRAM");
        std::env::remove_var("TERM_PROGRAM_VERSION");
        // Markers describing a *parent* Claude Code session (e.g. Promptly
        // launched from inside one). Inherited, they would make every child
        // `claude` think it is a sub-session and disable transcript saving.
        for k in INHERITED_CLAUDE_MARKERS {
            std::env::remove_var(k);
        }
        std::env::remove_var(promptly_core::accounts::CONFIG_DIR_ENV);
    }
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("Promptly")
            .with_app_id("dev.promptly.Promptly")
            .with_inner_size([1440.0, 900.0])
            .with_min_inner_size([720.0, 420.0])
            // Unified window on macOS: content runs under a transparent title
            // bar and the traffic lights sit over the sidebar.
            .with_fullsize_content_view(cfg!(target_os = "macos"))
            .with_title_shown(!cfg!(target_os = "macos"))
            .with_titlebar_shown(!cfg!(target_os = "macos"))
            .with_icon(
                eframe::icon_data::from_png_bytes(include_bytes!(
                    "../../../packaging/icon/promptly-512.png"
                ))
                .unwrap_or_default(),
            ),
        renderer: eframe::Renderer::Wgpu,
        ..Default::default()
    };
    eframe::run_native(
        "Promptly",
        options,
        Box::new(|cc| Ok(Box::new(app::App::new(cc)))),
    )
}
