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
        render(&mut h, &dir, name);
    }
}
