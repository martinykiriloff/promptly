//! PTY manager: spawns a child in a PTY, drives alacritty_terminal's VT
//! engine on a reader thread, and tees output through a small OSC scanner
//! for shell-integration marks (OSC 133 / OSC 7) the engine ignores.

use alacritty_terminal::event::{Event as TermEvent, EventListener, OnResize, WindowSize};
use alacritty_terminal::event_loop::{EventLoop, EventLoopSender, Msg};
use alacritty_terminal::sync::FairMutex;
use alacritty_terminal::term::{Config as TermConfig, Term};
use alacritty_terminal::tty::{self, EventedPty, EventedReadWrite, Options, Shell};
use parking_lot::Mutex;
use std::borrow::Cow;
use std::collections::HashMap;
use std::fs::File;
use std::io::{self, Read};
use std::path::PathBuf;
use std::sync::mpsc::Sender;
use std::sync::{Arc, OnceLock};

use crate::events::AppEvent;
use crate::theme;

pub type PaneId = u64;

/// Shell-integration marks parsed out of the output stream.
#[derive(Debug, Clone, PartialEq)]
pub enum ShellMark {
    PromptStart,
    CommandStart,
    CommandExecuted,
    CommandFinished(Option<i32>),
    Cwd(PathBuf),
}

/// Incremental OSC parser. Only OSC 7 and 133 are surfaced; it never
/// modifies the byte stream.
#[derive(Default)]
pub struct OscScanner {
    state: ScanState,
    buf: Vec<u8>,
}

#[derive(Default, PartialEq)]
enum ScanState {
    #[default]
    Ground,
    Esc,
    Osc,
    OscEsc,
}

const MAX_OSC: usize = 4096;

impl OscScanner {
    pub fn feed(&mut self, bytes: &[u8], out: &mut Vec<ShellMark>) {
        for &b in bytes {
            self.state = match (&self.state, b) {
                (ScanState::Ground, 0x1b) => ScanState::Esc,
                (ScanState::Ground, _) => ScanState::Ground,
                (ScanState::Esc, b']') => {
                    self.buf.clear();
                    ScanState::Osc
                }
                (ScanState::Esc, 0x1b) => ScanState::Esc,
                (ScanState::Esc, _) => ScanState::Ground,
                (ScanState::Osc, 0x07) => {
                    self.finish(out);
                    ScanState::Ground
                }
                (ScanState::Osc, 0x1b) => ScanState::OscEsc,
                (ScanState::Osc, _) => {
                    if self.buf.len() < MAX_OSC {
                        self.buf.push(b);
                    }
                    ScanState::Osc
                }
                (ScanState::OscEsc, b'\\') => {
                    self.finish(out);
                    ScanState::Ground
                }
                (ScanState::OscEsc, b']') => {
                    self.buf.clear();
                    ScanState::Osc
                }
                (ScanState::OscEsc, _) => ScanState::Ground,
            };
        }
    }

    fn finish(&mut self, out: &mut Vec<ShellMark>) {
        let s = String::from_utf8_lossy(&self.buf);
        if let Some(rest) = s.strip_prefix("133;") {
            let mut parts = rest.split(';');
            match parts.next() {
                Some("A") => out.push(ShellMark::PromptStart),
                Some("B") => out.push(ShellMark::CommandStart),
                Some("C") => out.push(ShellMark::CommandExecuted),
                Some("D") => out.push(ShellMark::CommandFinished(
                    parts.next().and_then(|c| c.parse().ok()),
                )),
                _ => {}
            }
        } else if let Some(url) = s.strip_prefix("7;")
            && let Some(path) = file_url_path(url)
        {
            out.push(ShellMark::Cwd(path));
        }
        self.buf.clear();
    }
}

/// `file://host/path%20x` -> `/path x`.
fn file_url_path(url: &str) -> Option<PathBuf> {
    let rest = url.strip_prefix("file://")?;
    let path = &rest[rest.find('/')?..];
    let mut out = Vec::with_capacity(path.len());
    let b = path.as_bytes();
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%'
            && i + 2 < b.len()
            && let Ok(v) = u8::from_str_radix(&path[i + 1..i + 3], 16)
        {
            out.push(v);
            i += 3;
            continue;
        }
        out.push(b[i]);
        i += 1;
    }
    Some(PathBuf::from(String::from_utf8_lossy(&out).into_owned()))
}

/// Reader half that tees bytes through the OSC scanner.
pub struct TeeReader {
    file: File,
    scanner: OscScanner,
    pane: PaneId,
    tx: Sender<AppEvent>,
    marks: Vec<ShellMark>,
}

impl Read for TeeReader {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let n = self.file.read(buf)?;
        self.scanner.feed(&buf[..n], &mut self.marks);
        for m in self.marks.drain(..) {
            let _ = self.tx.send(AppEvent::Shell(self.pane, m));
        }
        Ok(n)
    }
}

/// `tty::Pty` with a teeing reader.
pub struct TeePty {
    inner: tty::Pty,
    reader: TeeReader,
}

impl EventedReadWrite for TeePty {
    type Reader = TeeReader;
    type Writer = File;

    unsafe fn register(
        &mut self,
        poll: &Arc<polling::Poller>,
        ev: polling::Event,
        mode: polling::PollMode,
    ) -> io::Result<()> {
        // SAFETY: forwarded verbatim; `inner` outlives the registration.
        unsafe { self.inner.register(poll, ev, mode) }
    }
    fn reregister(
        &mut self,
        poll: &Arc<polling::Poller>,
        ev: polling::Event,
        mode: polling::PollMode,
    ) -> io::Result<()> {
        self.inner.reregister(poll, ev, mode)
    }
    fn deregister(&mut self, poll: &Arc<polling::Poller>) -> io::Result<()> {
        self.inner.deregister(poll)
    }
    fn reader(&mut self) -> &mut TeeReader {
        &mut self.reader
    }
    fn writer(&mut self) -> &mut File {
        self.inner.writer()
    }
}

impl EventedPty for TeePty {
    fn next_child_event(&mut self) -> Option<tty::ChildEvent> {
        self.inner.next_child_event()
    }
}

impl OnResize for TeePty {
    fn on_resize(&mut self, size: WindowSize) {
        self.inner.on_resize(size)
    }
}

/// Bridges VT-engine events to the app. Query replies (DA, colors, size)
/// are answered right here on the reader thread so the child never waits on
/// the UI frame loop.
#[derive(Clone)]
pub struct Listener {
    pane: PaneId,
    tx: Sender<AppEvent>,
    ctx: egui::Context,
    sender: Arc<OnceLock<EventLoopSender>>,
    size: Arc<Mutex<WindowSize>>,
}

impl Listener {
    fn write(&self, s: String) {
        if let Some(tx) = self.sender.get() {
            let _ = tx.send(Msg::Input(Cow::Owned(s.into_bytes())));
        }
    }
}

impl EventListener for Listener {
    fn send_event(&self, event: TermEvent) {
        match event {
            TermEvent::Wakeup | TermEvent::MouseCursorDirty | TermEvent::CursorBlinkingChange => {}
            TermEvent::PtyWrite(s) => {
                self.write(s);
                return;
            }
            TermEvent::ColorRequest(idx, fmt) => {
                self.write(fmt(theme::color_for_index(idx)));
                return;
            }
            TermEvent::TextAreaSizeRequest(fmt) => {
                self.write(fmt(*self.size.lock()));
                return;
            }
            // OSC 52 read is disabled in the engine config; never answer.
            TermEvent::ClipboardLoad(..) => return,
            other => {
                let _ = self.tx.send(AppEvent::Term(self.pane, other));
            }
        }
        self.ctx.request_repaint();
    }
}

pub struct SpawnSpec {
    pub program: String,
    pub args: Vec<String>,
    pub cwd: Option<PathBuf>,
    pub env: HashMap<String, String>,
    pub scrollback: usize,
}

pub struct Pane {
    pub id: PaneId,
    pub term: Arc<FairMutex<Term<Listener>>>,
    sender: EventLoopSender,
    size: Arc<Mutex<WindowSize>>,
}

impl Pane {
    pub fn spawn(
        id: PaneId,
        spec: SpawnSpec,
        tx: Sender<AppEvent>,
        ctx: egui::Context,
    ) -> io::Result<Self> {
        let size = WindowSize {
            num_lines: 24,
            num_cols: 80,
            cell_width: 8,
            cell_height: 16,
        };
        let size_cell = Arc::new(Mutex::new(size));
        let mut env = spec.env;
        env.insert("TERM".into(), "xterm-256color".into());
        env.insert("COLORTERM".into(), "truecolor".into());
        env.insert("TERM_PROGRAM".into(), "Promptly".into());
        env.insert(
            "TERM_PROGRAM_VERSION".into(),
            env!("CARGO_PKG_VERSION").into(),
        );
        let opts = Options {
            shell: Some(Shell::new(spec.program, spec.args)),
            working_directory: spec.cwd,
            drain_on_exit: true,
            env,
        };
        let inner = tty::new(&opts, size, id)?;
        let file = inner.file().try_clone()?;
        let reader = TeeReader {
            file,
            scanner: OscScanner::default(),
            pane: id,
            tx: tx.clone(),
            marks: vec![],
        };
        let pty = TeePty { inner, reader };

        let sender_cell = Arc::new(OnceLock::new());
        let listener = Listener {
            pane: id,
            tx,
            ctx,
            sender: sender_cell.clone(),
            size: size_cell.clone(),
        };
        let config = TermConfig {
            scrolling_history: spec.scrollback,
            kitty_keyboard: true,
            osc52: alacritty_terminal::term::Osc52::OnlyCopy,
            ..TermConfig::default()
        };
        let term = Term::new(
            config,
            &crate::term_view::GridSize {
                cols: 80,
                lines: 24,
            },
            listener.clone(),
        );
        let term = Arc::new(FairMutex::new(term));
        let event_loop = EventLoop::new(term.clone(), listener, pty, true, false)?;
        let sender = event_loop.channel();
        let _ = sender_cell.set(event_loop.channel());
        event_loop.spawn();
        Ok(Self {
            id,
            term,
            sender,
            size: size_cell,
        })
    }

    pub fn write(&self, bytes: impl Into<Cow<'static, [u8]>>) {
        let _ = self.sender.send(Msg::Input(bytes.into()));
    }

    pub fn resize(&self, cols: u16, lines: u16, cell_w: u16, cell_h: u16) {
        let new = WindowSize {
            num_lines: lines,
            num_cols: cols,
            cell_width: cell_w,
            cell_height: cell_h,
        };
        {
            let mut cur = self.size.lock();
            if cur.num_lines == lines && cur.num_cols == cols {
                return;
            }
            *cur = new;
        }
        self.term.lock().resize(crate::term_view::GridSize {
            cols: cols as usize,
            lines: lines as usize,
        });
        let _ = self.sender.send(Msg::Resize(new));
    }
}

impl Drop for Pane {
    fn drop(&mut self) {
        let _ = self.sender.send(Msg::Shutdown);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scanner_extracts_marks_across_chunks() {
        let mut s = OscScanner::default();
        let mut out = vec![];
        s.feed(b"hello\x1b]133;A\x07$ \x1b]133;", &mut out);
        s.feed(
            b"D;2\x1b\\\x1b]7;file://mac/Users/me/my%20dir\x07",
            &mut out,
        );
        assert_eq!(
            out,
            vec![
                ShellMark::PromptStart,
                ShellMark::CommandFinished(Some(2)),
                ShellMark::Cwd(PathBuf::from("/Users/me/my dir")),
            ]
        );
    }

    #[test]
    fn scanner_ignores_other_osc_and_bounds_memory() {
        let mut s = OscScanner::default();
        let mut out = vec![];
        s.feed(b"\x1b]0;title\x07\x1b]52;c;aGk=\x07", &mut out);
        assert!(out.is_empty());
        let big = vec![b'x'; 100_000];
        s.feed(b"\x1b]133;", &mut out);
        s.feed(&big, &mut out);
        assert!(s.buf.len() <= MAX_OSC);
    }
}
