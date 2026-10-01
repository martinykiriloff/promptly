//! Developer-only frame recorder for demo videos.
//!
//! With `PROMPTLY_RECORD_DIR` set, the app captures its own rendered frames
//! (independent of window position, Spaces or occlusion) and writes them as
//! numbered PNGs at half resolution. `PROMPTLY_RECORD_FPS` sets the rate
//! (default 12). Turn the frames into a video with ffmpeg.

use egui::{ColorImage, Context, Event, UserData, ViewportCommand};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::mpsc::{self, Sender};
use std::time::{Duration, Instant};

pub struct Recorder {
    interval: Duration,
    last: Instant,
    next: u64,
    tx: Sender<(u64, Arc<ColorImage>)>,
}

impl Recorder {
    pub fn from_env() -> Option<Self> {
        let dir = PathBuf::from(std::env::var_os("PROMPTLY_RECORD_DIR")?);
        std::fs::create_dir_all(&dir).ok()?;
        let fps: f64 = std::env::var("PROMPTLY_RECORD_FPS")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(12.0);
        let (tx, rx) = mpsc::channel::<(u64, Arc<ColorImage>)>();
        std::thread::Builder::new()
            .name("frame writer".into())
            .spawn(move || {
                for (n, img) in rx {
                    let [w, h] = img.size;
                    let (hw, hh) = (w / 2, h / 2);
                    let mut buf = Vec::with_capacity(hw * hh * 4);
                    for y in 0..hh {
                        for x in 0..hw {
                            let c = img.pixels[(y * 2) * w + x * 2];
                            buf.extend_from_slice(&[c.r(), c.g(), c.b(), 255]);
                        }
                    }
                    let path = dir.join(format!("frame-{n:05}.png"));
                    let _ = image::save_buffer(
                        &path,
                        &buf,
                        hw as u32,
                        hh as u32,
                        image::ExtendedColorType::Rgba8,
                    );
                }
            })
            .ok()?;
        Some(Self {
            interval: Duration::from_secs_f64(1.0 / fps.max(1.0)),
            last: Instant::now(),
            next: 0,
            tx,
        })
    }

    /// Call once per frame.
    pub fn frame(&mut self, ctx: &Context) {
        let shots: Vec<Arc<ColorImage>> = ctx.input(|i| {
            i.raw
                .events
                .iter()
                .filter_map(|e| match e {
                    Event::Screenshot { image, .. } => Some(image.clone()),
                    _ => None,
                })
                .collect()
        });
        for img in shots {
            let _ = self.tx.send((self.next, img));
            self.next += 1;
        }
        if self.last.elapsed() >= self.interval {
            self.last = Instant::now();
            ctx.send_viewport_cmd(ViewportCommand::Screenshot(UserData::default()));
        }
        ctx.request_repaint_after(self.interval / 2);
    }
}
