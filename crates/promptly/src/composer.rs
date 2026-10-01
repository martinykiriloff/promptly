//! Docked multi-line prompt composer: @file completion, history, snippets.
//! Sends into the session's PTY as a bracketed paste followed by Enter.

use egui::RichText;
use std::path::{Path, PathBuf};
use std::sync::Arc;

const HISTORY_CAP: usize = 200;
const MAX_FILES: usize = 20_000;

#[derive(Default)]
pub struct Composer {
    pub text: String,
    history: Vec<String>,
    history_pos: Option<usize>,
    files: Option<(PathBuf, Arc<Vec<String>>)>,
    completion: Vec<String>,
    completion_sel: usize,
    pub focus_requested: bool,
}

pub enum ComposerAction {
    Send(String),
    FocusTerminal,
}

impl Composer {
    pub fn load_history() -> Vec<String> {
        std::fs::read_to_string(history_path())
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default()
    }

    pub fn with_history(history: Vec<String>) -> Self {
        Self {
            history,
            ..Default::default()
        }
    }

    fn save_history(&self) {
        if let Ok(b) = serde_json::to_vec(&self.history) {
            let p = history_path();
            if let Some(d) = p.parent() {
                let _ = std::fs::create_dir_all(d);
            }
            let _ = promptly_core::util::write_private(&p, &b);
        }
    }

    fn file_list(&mut self, cwd: &Path) -> Arc<Vec<String>> {
        if let Some((d, f)) = &self.files
            && d == cwd
        {
            return f.clone();
        }
        let list = promptly_core::util::run_stdout(
            std::process::Command::new("git").arg("-C").arg(cwd).args([
                "ls-files",
                "--cached",
                "--others",
                "--exclude-standard",
            ]),
        )
        .map(|s| {
            s.lines()
                .take(MAX_FILES)
                .map(str::to_owned)
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
        let list = Arc::new(list);
        self.files = Some((cwd.to_path_buf(), list.clone()));
        list
    }

    /// The `@partial` token at the end of the text, if any.
    fn at_token(&self) -> Option<&str> {
        let last = self.text.rsplit(|c: char| c.is_whitespace()).next()?;
        last.strip_prefix('@')
    }

    pub fn show(
        &mut self,
        ui: &mut egui::Ui,
        cwd: &Path,
        snippets: &std::collections::BTreeMap<String, String>,
        target: &str,
    ) -> Option<ComposerAction> {
        use crate::theme::tokens as t;
        use crate::ui_kit::{self as kit, Icon};
        let mut action = None;

        // Completion candidates for a trailing @token.
        let token = self.at_token().map(str::to_owned);
        if let Some(tok) = &token {
            let files = self.file_list(cwd);
            let q = tok.to_ascii_lowercase();
            self.completion = files
                .iter()
                .filter(|f| fuzzy(&f.to_ascii_lowercase(), &q))
                .take(8)
                .cloned()
                .collect();
            self.completion_sel = self
                .completion_sel
                .min(self.completion.len().saturating_sub(1));
        } else {
            self.completion.clear();
        }

        // Keys handled before the TextEdit sees them.
        let has_completion = !self.completion.is_empty();
        let (send, esc, tab, up, down, sel_up, sel_down) = ui.input_mut(|i| {
            (
                i.consume_key(egui::Modifiers::COMMAND, egui::Key::Enter),
                has_completion && i.consume_key(egui::Modifiers::NONE, egui::Key::Escape),
                has_completion && i.consume_key(egui::Modifiers::NONE, egui::Key::Tab),
                i.consume_key(egui::Modifiers::CTRL, egui::Key::ArrowUp),
                i.consume_key(egui::Modifiers::CTRL, egui::Key::ArrowDown),
                has_completion && i.consume_key(egui::Modifiers::ALT, egui::Key::ArrowUp),
                has_completion && i.consume_key(egui::Modifiers::ALT, egui::Key::ArrowDown),
            )
        });
        if sel_up {
            self.completion_sel = self.completion_sel.saturating_sub(1);
        }
        if sel_down {
            self.completion_sel =
                (self.completion_sel + 1).min(self.completion.len().saturating_sub(1));
        }

        // @file suggestions float just above the card.
        if has_completion {
            ui.horizontal_wrapped(|ui| {
                ui.spacing_mut().item_spacing = egui::vec2(4.0, 4.0);
                let (r, _) = ui.allocate_exact_size(egui::vec2(14.0, 14.0), egui::Sense::hover());
                kit::paint_icon(ui, r, Icon::Search, t::TEXT_3);
                for (i, c) in self.completion.iter().enumerate() {
                    let selected = i == self.completion_sel;
                    let col = if selected { t::TEXT } else { t::TEXT_2 };
                    let chip =
                        egui::Button::new(RichText::new(c).monospace().size(11.5).color(col))
                            .fill(if selected { t::ACTIVE } else { t::BG_ELEVATED })
                            .corner_radius(egui::CornerRadius::same(5));
                    if ui.add(chip).clicked() {
                        self.completion_sel = i;
                    }
                }
                ui.label(RichText::new("Tab to insert").size(11.0).color(t::TEXT_3));
            });
            ui.add_space(4.0);
        }

        let mut text_has_focus = false;
        let card = egui::Frame::new()
            .fill(t::BG_INPUT)
            .stroke(egui::Stroke::new(1.0, t::BORDER))
            .corner_radius(egui::CornerRadius::same(10))
            .inner_margin(egui::Margin {
                left: 12,
                right: 8,
                top: 10,
                bottom: 8,
            });
        let card_resp = card.show(ui, |ui| {
            let edit = egui::TextEdit::multiline(&mut self.text)
                .id_salt("composer")
                .frame(egui::Frame::NONE)
                .desired_rows(2)
                .desired_width(f32::INFINITY)
                .font(egui::FontId::proportional(14.0))
                .hint_text(
                    RichText::new(format!("Message {target}…  @ to reference a file"))
                        .size(14.0)
                        .color(t::TEXT_3),
                );
            let resp = ui.add(edit);
            if self.focus_requested {
                resp.request_focus();
                self.focus_requested = false;
            }
            text_has_focus = resp.has_focus();
            if text_has_focus {
                if tab
                    && let (Some(tok), Some(pick)) =
                        (&token, self.completion.get(self.completion_sel).cloned())
                {
                    let cut = self.text.len() - tok.len();
                    self.text.truncate(cut);
                    self.text.push_str(&pick);
                    self.text.push(' ');
                    self.completion.clear();
                }
                if esc {
                    self.completion.clear();
                } else if ui.input(|i| i.key_pressed(egui::Key::Escape)) {
                    action = Some(ComposerAction::FocusTerminal);
                }
                if up {
                    self.history_step(true);
                }
                if down {
                    self.history_step(false);
                }
            }
            ui.add_space(4.0);
            let mut send_clicked = false;
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 6.0;
                ui.menu_button(
                    RichText::new("Snippets").size(12.0).color(t::TEXT_2),
                    |ui| {
                        for (name, body) in snippets {
                            if ui.button(name).on_hover_text(body).clicked() {
                                if !self.text.is_empty() && !self.text.ends_with('\n') {
                                    self.text.push('\n');
                                }
                                self.text.push_str(body);
                                self.focus_requested = true;
                                ui.close();
                            }
                        }
                    },
                );
                ui.label(RichText::new("⌃↑↓ history").size(11.0).color(t::TEXT_3));
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let can_send = !self.text.trim().is_empty();
                    ui.add_enabled_ui(can_send, |ui| {
                        let shortcut = if cfg!(target_os = "macos") {
                            "⌘↵"
                        } else {
                            "Ctrl+↵"
                        };
                        if kit::primary_button(ui, Some(Icon::Send), "Send", Some(shortcut), false)
                            .clicked()
                        {
                            send_clicked = true;
                        }
                    });
                });
            });
            send_clicked
        });
        // Focus ring on the card while typing.
        if text_has_focus {
            ui.painter().rect_stroke(
                card_resp.response.rect,
                egui::CornerRadius::same(10),
                egui::Stroke::new(1.0, t::ACCENT.gamma_multiply(0.8)),
                egui::StrokeKind::Inside,
            );
        }
        let send = send || card_resp.inner;
        if send && !self.text.trim().is_empty() {
            let text = std::mem::take(&mut self.text);
            self.history.retain(|h| h != &text);
            self.history.push(text.clone());
            if self.history.len() > HISTORY_CAP {
                self.history.remove(0);
            }
            self.history_pos = None;
            self.save_history();
            action = Some(ComposerAction::Send(text));
        }
        action
    }

    fn history_step(&mut self, back: bool) {
        if self.history.is_empty() {
            return;
        }
        let n = self.history.len();
        let pos = match (self.history_pos, back) {
            (None, true) => n - 1,
            (None, false) => return,
            (Some(p), true) => p.saturating_sub(1),
            (Some(p), false) if p + 1 >= n => {
                self.history_pos = None;
                self.text.clear();
                return;
            }
            (Some(p), false) => p + 1,
        };
        self.history_pos = Some(pos);
        self.text = self.history[pos].clone();
    }
}

/// Subsequence match, so `@srcmain` finds `src/main.rs`.
pub fn fuzzy(hay: &str, needle: &str) -> bool {
    let mut it = hay.chars();
    needle.chars().all(|n| it.any(|h| h == n))
}

fn history_path() -> PathBuf {
    promptly_core::paths::data_dir().join("composer-history.json")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fuzzy_subsequence() {
        assert!(fuzzy("src/main.rs", "srcmain"));
        assert!(fuzzy("src/main.rs", ""));
        assert!(!fuzzy("src/main.rs", "zz"));
    }

    #[test]
    fn at_token() {
        let mut c = Composer {
            text: "look at @src/ma".into(),
            ..Default::default()
        };
        assert_eq!(c.at_token(), Some("src/ma"));
        c.text = "email me@x.com please".into();
        assert_eq!(c.at_token(), None);
    }
}
