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

/// Marketing screenshots for the README and the website, from a real Claude
/// Code session. Opt-in (uses your Claude login and a little of your plan):
///   PROMPTLY_SHOTS=/tmp/m cargo test -p promptly marketing -- --ignored --nocapture
/// Needs `promptly-ctl` next to the test binary for live usage.
#[test]
#[ignore]
fn marketing() {
    use std::time::{Duration, Instant};
    let dir = std::path::PathBuf::from(
        std::env::var("PROMPTLY_SHOTS").unwrap_or_else(|_| "/tmp/promptly-marketing".into()),
    );
    std::fs::create_dir_all(&dir).unwrap();
    let tmp = tempfile::tempdir().unwrap();
    let data = tmp.path().join("data");
    std::fs::create_dir_all(&data).unwrap();
    // Real plan-limit history so the dashboard has a trend.
    let real = promptly_core::paths::data_dir();
    for f in std::fs::read_dir(&real).into_iter().flatten().flatten() {
        let n = f.file_name().to_string_lossy().to_string();
        if n.starts_with("usage") && n.ends_with(".json") {
            let _ = std::fs::copy(f.path(), data.join(&n));
        }
    }
    // Show a display name instead of an email in public images.
    let home = promptly_core::paths::home().join(".claude");
    let accounts = serde_json::json!({
        "names": { home.to_string_lossy(): "Martin" },
        "active": home.to_string_lossy(),
    });
    std::fs::write(data.join("accounts.json"), accounts.to_string()).unwrap();
    // SAFETY: test-only setup before any app threads start.
    unsafe {
        std::env::set_var("PROMPTLY_DATA_DIR", &data);
        std::env::set_var("XDG_CONFIG_HOME", tmp.path().join("config"));
        std::env::set_var("XDG_RUNTIME_DIR", tmp.path().join("run"));
        for k in [
            "PROMPTLY_CONTROL",
            "PROMPTLY_SOCKET",
            "PROMPTLY_TOKEN",
            "PROMPTLY_SESSION",
            "PROMPTLY_FAKE_VERSION",
        ] {
            std::env::remove_var(k);
        }
    }
    let mut h = Harness::builder()
        .with_size(egui::vec2(1440.0, 900.0))
        .with_pixels_per_point(2.0)
        .wgpu()
        .build_eframe(|cc| App::new(cc));
    // Render shortcuts as on a Mac (⌘), where these screenshots are used.
    h.ctx.set_os(egui::os::OperatingSystem::Mac);
    let pump = |h: &mut Harness<'_, App>, secs: f32| {
        let t0 = Instant::now();
        while t0.elapsed().as_secs_f32() < secs {
            h.step();
            std::thread::sleep(Duration::from_millis(50));
        }
    };
    pump(&mut h, 1.0);
    let id = h
        .state_mut()
        .demo_claude("/private/tmp/prdemo2")
        .expect("claude session");
    // Wait for Claude's prompt; accept the one-time folder trust question.
    let t0 = Instant::now();
    let mut trusted = false;
    loop {
        pump(&mut h, 0.5);
        let screen = h.state().demo_screen(id);
        if !trusted && screen.contains("trust this folder") {
            // The default answer is "No, exit": move to "Yes" first.
            h.state().demo_type(id, b"\x1b[B");
            pump(&mut h, 0.4);
            h.state().demo_type(id, b"\r");
            trusted = true;
            pump(&mut h, 2.0);
            continue;
        }
        if screen.contains("❯") || t0.elapsed() > Duration::from_secs(40) {
            break;
        }
    }
    pump(&mut h, 1.5);
    h.state_mut().demo_send(
        id,
        "Read src/middleware/rate-limit.ts and tell me in two short sentences what it does and one risk you see.",
    );
    // Wait for the turn to start, then to finish.
    let t0 = Instant::now();
    while !h.state().demo_working(id) && t0.elapsed() < Duration::from_secs(30) {
        pump(&mut h, 0.3);
    }
    let t0 = Instant::now();
    while (h.state().demo_working(id) || !h.state().demo_settled(id))
        && t0.elapsed() < Duration::from_secs(180)
    {
        pump(&mut h, 0.5);
    }
    pump(&mut h, 2.0);
    let shot = |h: &mut Harness<'_, App>, name: &str| {
        pump(h, 1.2);
        let img = h.render().expect("render");
        let path = dir.join(format!("{name}.png"));
        img.save(&path).expect("save png");
        println!("wrote {}", path.display());
    };
    h.state_mut().demo_view(true, false, false);
    shot(&mut h, "main");
    h.state_mut().demo_view(false, true, false);
    shot(&mut h, "usage");
    h.state_mut().demo_view(false, false, true);
    shot(&mut h, "palette");
}

/// A real Claude session edits a file; the session's changes must show
/// exactly that edit (the file's copy is taken before Claude writes).
///   cargo test -p promptly session_changes_e2e -- --ignored --nocapture
#[test]
#[ignore]
fn session_changes_e2e() {
    use std::time::{Duration, Instant};
    let dir = "/private/tmp/promptly-demo";
    let file = std::path::Path::new(dir).join("main.rs");
    let before = std::fs::read_to_string(&file).unwrap();
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
        .wgpu()
        .build_eframe(|cc| App::new(cc));
    let pump = |h: &mut Harness<'_, App>, secs: f32| {
        let t0 = Instant::now();
        while t0.elapsed().as_secs_f32() < secs {
            h.step();
            std::thread::sleep(Duration::from_millis(50));
        }
    };
    let id = h.state_mut().demo_claude(dir).expect("session");
    let t0 = Instant::now();
    while !h.state().demo_screen(id).contains("❯") && t0.elapsed() < Duration::from_secs(40) {
        pump(&mut h, 0.5);
    }
    pump(&mut h, 1.5);
    h.state_mut().demo_send(
        id,
        "Append exactly one line `// checked by Promptly` to the end of main.rs using your Edit tool. Change nothing else and don't run any commands.",
    );
    let t0 = Instant::now();
    while !h.state().demo_working(id) && t0.elapsed() < Duration::from_secs(30) {
        pump(&mut h, 0.3);
    }
    let mut seen_live = false;
    let t0 = Instant::now();
    while (h.state().demo_working(id) || !h.state().demo_settled(id))
        && t0.elapsed() < Duration::from_secs(180)
    {
        pump(&mut h, 0.5);
        if h.state().demo_working(id) && !h.state().demo_session_changes(id).is_empty() {
            seen_live = true;
        }
    }
    let changes = h.state().demo_session_changes(id);
    let after = std::fs::read_to_string(&file).unwrap();
    std::fs::write(&file, &before).unwrap();
    println!("changes: {changes:?} (seen while working: {seen_live})");
    assert!(
        after.contains("checked by Promptly"),
        "Claude didn't edit the file"
    );
    assert_eq!(changes, vec![("main.rs".to_string(), 1, 0)]);
}

/// Claude Code on a local Ollama model, switched to from Promptly.
/// Needs Ollama with qwen3:8b:
///   cargo test -p promptly local_model_e2e -- --ignored --nocapture
#[test]
#[ignore]
fn local_model_e2e() {
    use std::time::{Duration, Instant};
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
    h.ctx.set_os(egui::os::OperatingSystem::Mac);
    let pump = |h: &mut Harness<'_, App>, secs: f32| {
        let t0 = Instant::now();
        while t0.elapsed().as_secs_f32() < secs {
            h.step();
            std::thread::sleep(Duration::from_millis(50));
        }
    };
    let shot = |h: &mut Harness<'_, App>, name: &str| {
        pump(h, 1.0);
        let img = h.render().expect("render");
        img.save(dir.join(format!("{name}.png"))).unwrap();
        println!("wrote {name}.png");
    };
    pump(&mut h, 1.0);
    h.state_mut().demo_use_account("local:ollama:qwen3:8b");
    let id = h
        .state_mut()
        .demo_claude(
            &std::env::var("PROMPTLY_LOCAL_DIR").expect("PROMPTLY_LOCAL_DIR: an existing folder"),
        )
        .expect("session");
    // Wait for Claude's prompt; accept the one-time folder trust question.
    let t0 = Instant::now();
    let mut trusted = false;
    loop {
        pump(&mut h, 0.5);
        let screen = h.state().demo_screen(id);
        if !trusted && screen.contains("trust this folder") {
            h.state().demo_type(id, b"\x1b[B");
            pump(&mut h, 0.4);
            h.state().demo_type(id, b"\r");
            trusted = true;
            continue;
        }
        if (screen.contains("❯") && !screen.contains("trust this folder"))
            || t0.elapsed() > Duration::from_secs(60)
        {
            break;
        }
    }
    pump(&mut h, 1.5);
    h.state_mut().demo_send(
        id,
        "Reply with exactly this sentence and nothing else: local model ok",
    );
    // Claude Code reports the model it runs on through its status line.
    let t0 = Instant::now();
    loop {
        pump(&mut h, 0.5);
        if h.state().demo_screen(id).contains("Enter to continue") {
            h.state().demo_type(id, b"\r");
        }
        let model = h.state().demo_session_account(id).and_then(|a| a.2);
        if model.as_deref() == Some("qwen3:8b") || t0.elapsed() > Duration::from_secs(90) {
            break;
        }
    }
    pump(&mut h, 3.0);
    shot(&mut h, "local-session");
    let (account, cost, model) = h.state().demo_session_account(id).unwrap();
    println!("account {account}, model {model:?}, cost {cost:?}");
    assert_eq!(account, "local:ollama:qwen3:8b");
    assert_eq!(
        model.as_deref(),
        Some("qwen3:8b"),
        "Claude Code runs on the local model"
    );
    assert_eq!(cost, None, "no fake cost for a local model");
}

/// Themes and the review's Commits view, on a throwaway repository.
///   PROMPTLY_SHOTS=/tmp/s cargo test -p promptly themes_and_commits -- --ignored --nocapture
#[test]
#[ignore]
fn themes_and_commits() {
    use promptly_core::git::DiffScope;
    use std::time::{Duration, Instant};
    let dir = std::path::PathBuf::from(
        std::env::var("PROMPTLY_SHOTS").unwrap_or_else(|_| "/tmp/promptly-shots".into()),
    );
    std::fs::create_dir_all(&dir).unwrap();
    let tmp = tempfile::tempdir().unwrap();
    // A repo: main, then a feature branch with three commits.
    let repo = tmp.path().join("shop-api");
    std::fs::create_dir_all(repo.join("src")).unwrap();
    let g = |args: &[&str]| {
        let o = std::process::Command::new("git")
            .arg("-C")
            .arg(&repo)
            .args(["-c", "user.email=dev@example.com", "-c", "user.name=Martin"])
            .args(args)
            .output()
            .unwrap();
        assert!(o.status.success(), "{args:?}");
    };
    g(&["init", "-q", "-b", "main"]);
    std::fs::write(repo.join("README.md"), "# Shop API\n").unwrap();
    std::fs::write(
        repo.join("src/server.ts"),
        "import express from \"express\";\nconst app = express();\napp.listen(3000);\n",
    )
    .unwrap();
    g(&["add", "-A"]);
    g(&["commit", "-qm", "Initial server"]);
    g(&["checkout", "-qb", "feature/rate-limits"]);
    std::fs::write(repo.join("src/rate-limit.ts"), "export function rateLimit(perMinute: number) {\n  const buckets = new Map<string, number>();\n  return (key: string) => {\n    const left = (buckets.get(key) ?? perMinute) - 1;\n    buckets.set(key, left);\n    return left >= 0;\n  };\n}\n").unwrap();
    g(&["add", "-A"]);
    g(&["commit", "-qm", "Add token-bucket rate limiter"]);
    std::fs::write(repo.join("src/server.ts"), "import express from \"express\";\nimport { rateLimit } from \"./rate-limit\";\nconst app = express();\nconst allow = rateLimit(100);\napp.use((req, res, next) => (allow(req.ip) ? next() : res.sendStatus(429)));\napp.listen(3000);\n").unwrap();
    g(&["commit", "-qam", "Use the limiter in the server"]);
    std::fs::write(
        repo.join("README.md"),
        "# Shop API\n\n100 requests per minute per client.\n",
    )
    .unwrap();
    g(&["commit", "-qam", "Document rate limits"]);

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
    h.ctx.set_os(egui::os::OperatingSystem::Mac);
    let pump = |h: &mut Harness<'_, App>, secs: f32| {
        let t0 = Instant::now();
        while t0.elapsed().as_secs_f32() < secs {
            h.step();
            std::thread::sleep(Duration::from_millis(50));
        }
    };
    let shot = |h: &mut Harness<'_, App>, name: &str| {
        pump(h, 1.2);
        h.render()
            .expect("render")
            .save(dir.join(format!("{name}.png")))
            .unwrap();
        println!("wrote {name}.png");
    };
    let id = h.state_mut().demo_shell(repo.to_str().unwrap()).unwrap();
    pump(&mut h, 1.5);
    // Colored output to show the terminal palette.
    h.state().demo_type(id, b"clear; git log --oneline --graph --color=always --decorate; printf '\\e[31mred \\e[32mgreen \\e[33myellow \\e[34mblue \\e[35mmagenta \\e[36mcyan\\e[0m\\n'\r");
    pump(&mut h, 1.0);

    // Commits: newest by default, only that commit's files.
    h.state_mut().demo_review(DiffScope::Commits, None);
    pump(&mut h, 2.0);
    let d = h.state().demo_review_shown().expect("commits loaded");
    let subjects: Vec<_> = d.commits.iter().map(|c| c.subject.clone()).collect();
    println!("commits: {subjects:?}");
    assert_eq!(
        subjects,
        [
            "Document rate limits",
            "Use the limiter in the server",
            "Add token-bucket rate limiter"
        ]
    );
    assert_eq!(d.commit.as_ref().unwrap().subject, "Document rate limits");
    let files: Vec<_> = d.files.iter().map(|f| f.path.clone()).collect();
    assert_eq!(files, ["README.md"], "only the newest commit's files");
    shot(&mut h, "commits-newest");

    // Open an older commit: only its changes.
    let older = d.commits[1].sha.clone();
    h.state_mut().demo_review(DiffScope::Commits, Some(older));
    pump(&mut h, 2.0);
    let d = h.state().demo_review_shown().unwrap();
    assert_eq!(
        d.commit.as_ref().unwrap().subject,
        "Use the limiter in the server"
    );
    let files: Vec<_> = d
        .files
        .iter()
        .map(|f| (f.path.clone(), f.added, f.removed))
        .collect();
    println!("older commit files: {files:?}");
    assert_eq!(files, [("src/server.ts".to_string(), 3, 0)]);
    shot(&mut h, "commits-older");

    // Themes: the picker, then the main window in a few of them.
    h.state_mut().demo_settings_tab(2);
    shot(&mut h, "theme-picker");
    h.state_mut().demo_close_overlays();
    for name in ["GitHub Light", "Dracula", "Tokyo Night", "Catppuccin Mocha"] {
        h.state_mut().demo_theme(name);
        shot(
            &mut h,
            &format!("theme-{}", name.to_lowercase().replace(' ', "-")),
        );
    }
    // Commits view in a light theme too.
    h.state_mut().demo_theme("GitHub Light");
    h.state_mut().demo_review(DiffScope::Commits, None);
    shot(&mut h, "commits-light");
    h.state_mut().demo_theme("Promptly");
}

/// A fresh folder for real-Claude tests (never an existing project).
fn fresh_dir(tag: &str) -> std::path::PathBuf {
    let base = std::env::var("PROMPTLY_TEST_ROOT").expect("PROMPTLY_TEST_ROOT: a scratch folder");
    let d = std::path::Path::new(&base).join(format!(
        "{tag}-{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis()
    ));
    std::fs::create_dir_all(&d).unwrap();
    d
}

/// Start Claude in `dir` and wait for its prompt, accepting the one-time
/// folder trust question.
fn start_claude(h: &mut Harness<'_, App>, dir: &std::path::Path) -> crate::pty::PaneId {
    use std::time::{Duration, Instant};
    assert!(dir.is_dir(), "test folder must exist: {}", dir.display());
    let id = h
        .state_mut()
        .demo_claude(dir.to_str().unwrap())
        .expect("session");
    let t0 = Instant::now();
    let mut trusted = false;
    loop {
        let t1 = Instant::now();
        while t1.elapsed() < Duration::from_millis(500) {
            h.step();
            std::thread::sleep(Duration::from_millis(50));
        }
        let screen = h.state().demo_screen(id);
        if !trusted && screen.contains("trust this folder") {
            h.state().demo_type(id, b"\x1b[B");
            std::thread::sleep(Duration::from_millis(300));
            h.state().demo_type(id, b"\r");
            trusted = true;
            continue;
        }
        if screen.contains("Enter to continue") {
            h.state().demo_type(id, b"\r");
            continue;
        }
        if (screen.contains("❯") && !screen.contains("trust this folder"))
            || t0.elapsed() > Duration::from_secs(60)
        {
            return id;
        }
    }
}

/// Approve and deny Claude's permission prompts from Promptly's buttons.
///   PROMPTLY_TEST_ROOT=<scratch> cargo test -p promptly approve_deny_e2e -- --ignored --nocapture
#[test]
#[ignore]
fn approve_deny_e2e() {
    use promptly_core::state::SessionState;
    use std::time::{Duration, Instant};
    let shots = std::path::PathBuf::from(
        std::env::var("PROMPTLY_SHOTS").unwrap_or_else(|_| "/tmp/promptly-shots".into()),
    );
    std::fs::create_dir_all(&shots).unwrap();
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
    h.ctx.set_os(egui::os::OperatingSystem::Mac);
    let pump = |h: &mut Harness<'_, App>, secs: f32| {
        let t0 = Instant::now();
        while t0.elapsed().as_secs_f32() < secs {
            h.step();
            std::thread::sleep(Duration::from_millis(50));
        }
    };
    // Normal permission mode, so Claude asks.
    h.state_mut()
        .demo_claude_args(&["--permission-mode", "default"]);
    for (approve, file) in [(true, "approved.txt"), (false, "denied.txt")] {
        let dir = fresh_dir(if approve { "approve" } else { "deny" });
        let id = start_claude(&mut h, &dir);
        pump(&mut h, 1.0);
        h.state_mut().demo_send(
            id,
            &format!(
                "Run exactly this shell command with the Bash tool and nothing else: touch {file}"
            ),
        );
        let t0 = Instant::now();
        let prompt = loop {
            pump(&mut h, 0.5);
            if let Some(p) = h.state().demo_permission(id) {
                break p;
            }
            assert!(
                t0.elapsed() < Duration::from_secs(120),
                "no permission prompt"
            );
        };
        assert_eq!(
            h.state().demo_state(id),
            Some(SessionState::WaitingPermission)
        );
        println!("options: {:?}", prompt.options);
        if approve {
            let img = h.render().unwrap();
            img.save(shots.join("permission-buttons.png")).unwrap();
        }
        let choice = if approve {
            prompt.approve_option().expect("a Yes option")
        } else {
            prompt.deny_option().expect("a No option")
        };
        h.state_mut().demo_answer(id, choice);
        let t0 = Instant::now();
        while h.state().demo_permission(id).is_some() && t0.elapsed() < Duration::from_secs(20) {
            pump(&mut h, 0.3);
        }
        pump(&mut h, 4.0);
        let exists = dir.join(file).exists();
        println!(
            "{}: {file} exists = {exists}",
            if approve { "approve" } else { "deny" }
        );
        assert_eq!(exists, approve);
    }
}

/// Per-prompt changes, revert, and commit + push with a Claude-written
/// message, on a real session.
///   PROMPTLY_TEST_ROOT=<scratch> cargo test -p promptly review_actions_e2e -- --ignored --nocapture
#[test]
#[ignore]
fn review_actions_e2e() {
    use promptly_core::git::DiffScope;
    use promptly_core::state::SessionState;
    use std::time::{Duration, Instant};
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
    let repo = fresh_dir("review");
    let remote = fresh_dir("remote").join("r.git");
    let g = |dir: &std::path::Path, args: &[&str]| {
        let o = std::process::Command::new("git")
            .arg("-C")
            .arg(dir)
            .args(["-c", "user.email=t@t", "-c", "user.name=Tester"])
            .args(args)
            .output()
            .unwrap();
        assert!(
            o.status.success(),
            "{args:?}: {}",
            String::from_utf8_lossy(&o.stderr)
        );
        String::from_utf8_lossy(&o.stdout).to_string()
    };
    g(
        remote.parent().unwrap(),
        &[
            "init",
            "-q",
            "--bare",
            "-b",
            "main",
            remote.to_str().unwrap(),
        ],
    );
    g(&repo, &["init", "-q", "-b", "main"]);
    g(&repo, &["config", "user.email", "t@t"]);
    g(&repo, &["config", "user.name", "Tester"]);
    g(
        &repo,
        &["remote", "add", "origin", remote.to_str().unwrap()],
    );
    std::fs::write(repo.join("README.md"), "# Demo\n").unwrap();
    g(&repo, &["add", "-A"]);
    g(&repo, &["commit", "-qm", "init"]);

    let mut h = Harness::builder()
        .with_size(egui::vec2(1440.0, 900.0))
        .wgpu()
        .build_eframe(|cc| App::new(cc));
    let pump = |h: &mut Harness<'_, App>, secs: f32| {
        let t0 = Instant::now();
        while t0.elapsed().as_secs_f32() < secs {
            h.step();
            std::thread::sleep(Duration::from_millis(50));
        }
    };
    // Edits without asking, so the turns run unattended.
    h.state_mut()
        .demo_claude_args(&["--permission-mode", "acceptEdits"]);
    let id = start_claude(&mut h, &repo);
    pump(&mut h, 1.0);
    let turn = |h: &mut Harness<'_, App>, text: &str| {
        h.state_mut().demo_send(id, text);
        let t0 = Instant::now();
        while h.state().demo_state(id) != Some(SessionState::Working)
            && t0.elapsed() < Duration::from_secs(30)
        {
            pump(h, 0.3);
        }
        let t0 = Instant::now();
        while h.state().demo_state(id) == Some(SessionState::Working)
            && t0.elapsed() < Duration::from_secs(180)
        {
            pump(h, 0.5);
        }
        pump(h, 1.5);
    };
    turn(
        &mut h,
        "Create a file notes.md containing exactly one line: alpha. Use the Write tool. Do nothing else.",
    );
    turn(
        &mut h,
        "Append a second line beta to notes.md, and create todo.md containing exactly one line: x. Use the Edit and Write tools. Do nothing else.",
    );
    let session = h.state().demo_changes(id, false);
    let last = h.state().demo_changes(id, true);
    println!("session: {session:?}\nlast prompt: {last:?}");
    assert_eq!(
        session.iter().map(|f| f.0.as_str()).collect::<Vec<_>>(),
        ["notes.md", "todo.md"]
    );
    assert_eq!(
        last,
        [
            ("notes.md".to_string(), 1, 0),
            ("todo.md".to_string(), 1, 0)
        ],
        "only the second prompt's edits"
    );

    // Revert notes.md in the Last prompt view: back to after prompt 1.
    h.state_mut().demo_review(DiffScope::Turn, None);
    pump(&mut h, 2.0);
    let msg = h.state_mut().demo_revert_file("notes.md").unwrap();
    println!("{msg}");
    assert_eq!(
        std::fs::read_to_string(repo.join("notes.md"))
            .unwrap()
            .trim(),
        "alpha"
    );

    // Commit with a Claude-written message, then push.
    let message = h.state().demo_commit_message(&repo).unwrap();
    println!("message: {message:?}");
    assert!(!message.is_empty() && message.lines().next().unwrap().len() <= 100);
    promptly_core::git::commit_all(&repo, &message).unwrap();
    promptly_core::git::push(&repo).unwrap();
    let pushed = g(&remote, &["log", "-1", "--format=%s", "main"]);
    println!("remote head: {pushed}");
    assert_eq!(pushed.trim(), message.lines().next().unwrap().trim());
}
