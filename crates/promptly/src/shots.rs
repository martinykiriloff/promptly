//! Offscreen renders of the real app for design review and golden images.
//! Opt-in (spawns a shell, uses the GPU):
//!   PROMPTLY_SHOTS=/tmp/shots cargo test -p promptly shots -- --ignored --nocapture

use crate::app::App;
use egui_kittest::Harness;

fn render(h: &mut Harness<'_, App>, dir: &std::path::Path, name: &str) {
    for _ in 0..12 {
        h.step();
    }
    std::thread::sleep(std::time::Duration::from_millis(400)); // let the shell draw
    for _ in 0..6 {
        h.step();
    }
    let img = h.render().expect("render");
    let path = dir.join(format!("{name}.png"));
    img.save(&path).expect("save png");
    println!("wrote {}", path.display());
}

#[test]
#[ignore]
fn shots() {
    let dir = std::path::PathBuf::from(
        std::env::var("PROMPTLY_SHOTS").unwrap_or_else(|_| "/tmp/promptly-shots".into()),
    );
    std::fs::create_dir_all(&dir).unwrap();
    let tmp = tempfile::tempdir().unwrap();
    // SAFETY: test-only setup before any app threads start.
    unsafe {
        std::env::set_var("PROMPTLY_DATA_DIR", tmp.path().join("data"));
        std::env::set_var("XDG_CONFIG_HOME", tmp.path().join("config"));
        std::env::set_var("XDG_RUNTIME_DIR", tmp.path().join("run"));
        // Look older than the latest release so "Check now" finds an update.
        std::env::set_var("PROMPTLY_FAKE_VERSION", "2026.1.0");
        for k in [
            "PROMPTLY_CONTROL",
            "PROMPTLY_SOCKET",
            "PROMPTLY_TOKEN",
            "PROMPTLY_SESSION",
        ] {
            std::env::remove_var(k);
        }
    }
    let mut h = Harness::builder()
        .with_size(egui::vec2(1440.0, 900.0))
        .with_pixels_per_point(2.0)
        .wgpu()
        .build_eframe(|cc| App::new(cc));
    render(&mut h, &dir, "01-main");
    for (name, f) in crate::app::shot_scenes() {
        f(h.state_mut());
        // Wait for a real Claude reply when a scene asks for a command.
        let t0 = std::time::Instant::now();
        while h.state().ask_pending() && t0.elapsed().as_secs() < 90 {
            h.step();
            std::thread::sleep(std::time::Duration::from_millis(100));
        }
        render(&mut h, &dir, name);
    }
    // Press Enter on the last safe suggestion: it should run in the shell.
    h.state_mut().run_ask_scene();
    let t0 = std::time::Instant::now();
    while h.state().ask_pending() && t0.elapsed().as_secs() < 90 {
        h.step();
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
    render(&mut h, &dir, "18-ask-ready");
    h.key_press(egui::Key::Enter);
    std::thread::sleep(std::time::Duration::from_millis(800));
    render(&mut h, &dir, "19-ask-ran");
    // Open the account switcher (top of the sidebar) with a real click.
    let at = egui::pos2(150.0, 34.0 + 22.0);
    for pressed in [true, false] {
        h.input_mut().events.push(egui::Event::PointerButton {
            pos: at,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: Default::default(),
        });
        h.step();
    }
    render(&mut h, &dir, "13-account-switcher");
}
