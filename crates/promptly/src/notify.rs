//! Native notifications: UNUserNotificationCenter-backed on macOS (via
//! notify-rust), freedesktop D-Bus on Linux. A click only deep-links into
//! the session; it never approves anything.

use crate::events::AppEvent;
use crate::pty::PaneId;
use std::sync::mpsc::Sender;

pub fn send(
    title: String,
    body: String,
    pane: Option<PaneId>,
    tx: Sender<AppEvent>,
    ctx: egui::Context,
) {
    std::thread::spawn(move || {
        let mut n = notify_rust::Notification::new();
        n.summary(&title).body(&body).appname("Promptly");
        #[cfg(all(unix, not(target_os = "macos")))]
        {
            n.action("default", "Open session");
            n.hint(notify_rust::Hint::Category("im.received".into()));
            if let Ok(handle) = n.show() {
                handle.wait_for_action(|action| {
                    if action == "default" {
                        if let Some(p) = pane {
                            let _ = tx.send(AppEvent::FocusSession(p));
                            ctx.request_repaint();
                        }
                    }
                });
            }
        }
        #[cfg(target_os = "macos")]
        {
            // Unbundled builds cannot receive click callbacks; the in-app
            // attention queue (Cmd+J) is the deep link.
            let _ = (&pane, &tx, &ctx);
            let _ = n.show();
        }
    });
}
