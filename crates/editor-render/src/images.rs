//! Images in the document: the size is reserved from the file header, the
//! pixels are decoded on one worker thread, and nothing ever blocks the caller.
//!
//! `reserve` reads only the header (`ImageReader::into_dimensions`), so a line
//! knows its final height before a single pixel is decoded. When the worker
//! finishes it calls the waker; the shell then calls `DocView::poll_async`,
//! which paints the pixels into the space already reserved: no relayout.
//!
//! Only local paths are loaded, relative to the document's directory. A source
//! with a scheme is `Remote`: nothing is fetched and no job is queued.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::mpsc::{Receiver, Sender, channel};

use editor_core::host::Waker;
use editor_core::tokens::{Theme, TokenId, Tokens};
use image::ImageReader;
use image::imageops::FilterType;
use tiny_skia::Pixmap;

/// Decoded pixels are kept at no more than this multiple of the display size
/// in points, enough for a 2x display.
const DECODE_OVERSAMPLE: f32 = 2.0;

/// Where an image is in its life.
#[derive(Clone)]
pub enum ImageState {
    /// The header is read and the worker is decoding.
    Pending,
    /// Decoded, premultiplied RGBA, at most `DECODE_OVERSAMPLE` times the display size.
    Ready(Arc<Pixmap>),
    /// The file is missing or could not be decoded.
    Missing,
    /// A URL with a scheme. Nothing is fetched.
    Remote(String),
}

/// An image's reserved display size in points, and its state.
#[derive(Clone)]
pub struct ImageSlot {
    pub w_pt: f32,
    pub h_pt: f32,
    pub state: ImageState,
}

struct Entry {
    natural: (u32, u32),
    state: ImageState,
}

struct Job {
    src: String,
    path: PathBuf,
    target: (u32, u32),
}

struct Done {
    src: String,
    pixmap: Option<Pixmap>,
}

struct Worker {
    jobs: Sender<Job>,
    done: Receiver<Done>,
}

pub struct Images {
    base_dir: PathBuf,
    waker: Waker,
    column_pt: f32,
    max_height_pt: f32,
    entries: HashMap<String, Entry>,
    /// The source each line last reserved.
    lines: HashMap<usize, String>,
    pending: usize,
    /// Started on the first job, so a document without local images spawns nothing.
    worker: Option<Worker>,
}

/// True for a source with a URL scheme (`https://...`, `data:...`).
fn is_remote(src: &str) -> bool {
    let Some((scheme, _)) = src.split_once(':') else {
        return false;
    };
    scheme.len() >= 2
        && scheme.starts_with(|c: char| c.is_ascii_alphabetic())
        && scheme
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '.' | '-'))
}

fn decode(job: &Job) -> Option<Pixmap> {
    let img = ImageReader::open(&job.path)
        .ok()?
        .with_guessed_format()
        .ok()?
        .decode()
        .ok()?;
    let (tw, th) = job.target;
    let img = if img.width() > tw || img.height() > th {
        img.resize_exact(tw, th, FilterType::Triangle)
    } else {
        img
    };
    let rgba = img.to_rgba8();
    let (w, h) = rgba.dimensions();
    let mut data = rgba.into_raw();
    for px in data.as_chunks_mut::<4>().0 {
        let a = u16::from(px[3]);
        for c in &mut px[..3] {
            *c = ((u16::from(*c) * a + 127) / 255) as u8;
        }
    }
    let size = tiny_skia::IntSize::from_wh(w, h)?;
    Pixmap::from_vec(data, size)
}

impl Images {
    /// Images relative to `base_dir`; `waker` is called after each decode.
    pub fn new(base_dir: &Path, waker: Waker) -> Images {
        let t = Tokens::defaults(Theme::Light);
        Images {
            base_dir: base_dir.to_path_buf(),
            waker,
            column_pt: t.px(TokenId::SpaceColumn),
            max_height_pt: t.px(TokenId::SizeImageMaxHeight),
            entries: HashMap::new(),
            lines: HashMap::new(),
            pending: 0,
            worker: None,
        }
    }

    /// The column width and the tallest image, in points. Display sizes come
    /// from the natural size each time, so changing these never needs a decode.
    pub fn set_limits(&mut self, column_pt: f32, max_height_pt: f32) {
        self.column_pt = column_pt;
        self.max_height_pt = max_height_pt;
    }

    /// Reserves the space for the image `src` on `line`. Reads the header
    /// only, and queues one decode the first time a source is seen.
    pub fn reserve(&mut self, line: usize, src: &str) -> ImageSlot {
        self.lines.insert(line, src.to_owned());
        if !self.entries.contains_key(src) {
            let entry = self.open(src);
            self.entries.insert(src.to_owned(), entry);
        }
        self.slot_of(src).expect("just inserted")
    }

    /// The slot for a source already reserved.
    pub fn slot_of(&self, src: &str) -> Option<ImageSlot> {
        let e = self.entries.get(src)?;
        let (w_pt, h_pt) = self.display_size(e.natural);
        Some(ImageSlot {
            w_pt,
            h_pt,
            state: e.state.clone(),
        })
    }

    /// Takes finished decodes. Returns the source lines whose image changed
    /// state. Never blocks.
    pub fn poll(&mut self) -> Vec<usize> {
        let mut changed = Vec::new();
        let Some(w) = &self.worker else {
            return changed;
        };
        while let Ok(done) = w.done.try_recv() {
            self.pending = self.pending.saturating_sub(1);
            let Some(e) = self.entries.get_mut(&done.src) else {
                continue;
            };
            e.state = match done.pixmap {
                Some(p) => ImageState::Ready(Arc::new(p)),
                None => ImageState::Missing,
            };
            changed.extend(
                self.lines
                    .iter()
                    .filter(|(_, s)| **s == done.src)
                    .map(|(l, _)| *l),
            );
        }
        changed.sort_unstable();
        changed
    }

    /// Decodes queued or running.
    pub fn pending(&self) -> usize {
        self.pending
    }

    /// The display size in points: never wider than the column, never
    /// enlarged, no taller than the limit, aspect kept.
    fn display_size(&self, (nw, nh): (u32, u32)) -> (f32, f32) {
        if nw == 0 || nh == 0 {
            return (0.0, 0.0);
        }
        let (nw, nh) = (nw as f32, nh as f32);
        let mut w = nw.min(self.column_pt);
        let mut h = nh * w / nw;
        if h > self.max_height_pt {
            h = self.max_height_pt;
            w = nw * h / nh;
        }
        (w, h)
    }

    fn open(&mut self, src: &str) -> Entry {
        if is_remote(src) {
            return Entry {
                natural: (0, 0),
                state: ImageState::Remote(src.to_owned()),
            };
        }
        let missing = Entry {
            natural: (0, 0),
            state: ImageState::Missing,
        };
        let path = self.base_dir.join(src);
        let dims = ImageReader::open(&path)
            .and_then(|r| r.with_guessed_format())
            .ok()
            .and_then(|r| r.into_dimensions().ok());
        let Some(natural) = dims.filter(|&(w, h)| w > 0 && h > 0) else {
            return missing;
        };
        let (w_pt, h_pt) = self.display_size(natural);
        let target = (
            ((w_pt * DECODE_OVERSAMPLE).ceil() as u32).max(1),
            ((h_pt * DECODE_OVERSAMPLE).ceil() as u32).max(1),
        );
        self.queue(Job {
            src: src.to_owned(),
            path,
            target,
        });
        Entry {
            natural,
            state: ImageState::Pending,
        }
    }

    fn queue(&mut self, job: Job) {
        let worker = self.worker.get_or_insert_with(|| {
            let (jobs, job_rx) = channel::<Job>();
            let (done_tx, done) = channel::<Done>();
            let waker = self.waker.clone();
            std::thread::spawn(move || {
                while let Ok(job) = job_rx.recv() {
                    let pixmap = decode(&job);
                    if done_tx
                        .send(Done {
                            src: job.src,
                            pixmap,
                        })
                        .is_err()
                    {
                        break;
                    }
                    waker();
                }
            });
            Worker { jobs, done }
        });
        if worker.jobs.send(job).is_ok() {
            self.pending += 1;
        }
    }
}
