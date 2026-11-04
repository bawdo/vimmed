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
//!
//! Resampling a decoded image to its box in device pixels (tens of
//! milliseconds for a large image) runs on the same worker. `scaled` asks for
//! a box, keyed by (source, width, height): a request already in flight is not
//! queued again, and the result comes back through `poll` and the waker. A
//! result nobody claims before the next `poll` is dropped, so a box that
//! changed or scrolled away leaves nothing behind. The last `SCALED_KEEP`
//! claimed copies are kept, so an image scrolled back into view is a plain
//! copy again.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::mpsc::{Receiver, Sender, TryRecvError, channel};

use editor_core::host::Waker;
use editor_core::tokens::{Theme, TokenId, Tokens};
use image::ImageReader;
use image::imageops::FilterType;
use tiny_skia::Pixmap;

use crate::paint::draw_scaled;

/// Decoded pixels are kept at no more than this multiple of the display size
/// in points, enough for a 2x display.
const DECODE_OVERSAMPLE: f32 = 2.0;

/// How many resampled copies `Images` keeps for reuse, most recently used
/// first. Each is box-sized (a full-column image at scale 2 is about 4 MB),
/// so this bounds what is kept beyond the copies on-screen lines hold.
pub const SCALED_KEEP: usize = 4;

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

/// A decoded image resampled to a box in device pixels.
#[derive(Clone)]
pub struct Scaled {
    pub pixmap: Arc<Pixmap>,
    /// Every pixel has full alpha, so a redraw is a plain row copy.
    pub opaque: bool,
}

/// A resampled copy: the source and the box in pixels.
#[derive(Clone, PartialEq, Eq, Hash)]
struct ScaleKey {
    src: String,
    w: u32,
    h: u32,
}

impl ScaleKey {
    fn new(src: &str, w: u32, h: u32) -> ScaleKey {
        ScaleKey {
            src: src.to_owned(),
            w,
            h,
        }
    }
}

struct Entry {
    natural: (u32, u32),
    state: ImageState,
}

enum Job {
    Decode {
        src: String,
        path: PathBuf,
        target: (u32, u32),
    },
    Scale {
        key: ScaleKey,
        from: Arc<Pixmap>,
    },
}

enum Done {
    Decoded {
        src: String,
        pixmap: Option<Pixmap>,
    },
    /// `None` when the box could not be made, or a later request for the
    /// same source replaced it before it started.
    Scaled {
        key: ScaleKey,
        scaled: Option<Scaled>,
    },
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
    /// Boxes asked for and not landed yet.
    scaling: HashSet<ScaleKey>,
    /// Boxes from the latest `poll`, waiting to be claimed through `scaled`.
    landed: Vec<(ScaleKey, Scaled)>,
    /// Claimed boxes, most recently used first, at most `SCALED_KEEP`.
    kept: Vec<(ScaleKey, Scaled)>,
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

fn decode(path: &Path, target: (u32, u32)) -> Option<Pixmap> {
    let img = ImageReader::open(path)
        .ok()?
        .with_guessed_format()
        .ok()?
        .decode()
        .ok()?;
    let (tw, th) = target;
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

/// `from` resampled to the `w` by `h` box. Runs on the worker.
fn resample(from: &Pixmap, w: u32, h: u32) -> Option<Scaled> {
    let mut boxed = Pixmap::new(w, h)?;
    let (wf, hf) = (w as f32, h as f32);
    draw_scaled(&mut boxed, 0.0, 0.0, wf, hf, from, &(0.0..hf));
    let opaque = boxed.data().as_chunks::<4>().0.iter().all(|p| p[3] == 255);
    Some(Scaled {
        pixmap: Arc::new(boxed),
        opaque,
    })
}

/// Runs one batch of jobs in order, calling the waker after each result.
/// A resample is skipped (sent back as `None`) when a later job in the batch
/// resamples the same source: only the newest box for a source is wanted.
/// False once the receiving side has gone.
fn run_batch(batch: Vec<Job>, done: &Sender<Done>, waker: &Waker) -> bool {
    let newest: HashMap<String, usize> = batch
        .iter()
        .enumerate()
        .filter_map(|(k, j)| match j {
            Job::Scale { key, .. } => Some((key.src.clone(), k)),
            Job::Decode { .. } => None,
        })
        .collect();
    for (k, job) in batch.into_iter().enumerate() {
        let out = match job {
            Job::Decode { src, path, target } => Done::Decoded {
                src,
                pixmap: decode(&path, target),
            },
            Job::Scale { key, from } => {
                let wanted = newest.get(&key.src) == Some(&k);
                Done::Scaled {
                    scaled: wanted.then(|| resample(&from, key.w, key.h)).flatten(),
                    key,
                }
            }
        };
        if done.send(out).is_err() {
            return false;
        }
        waker();
    }
    true
}

impl Images {
    /// Images relative to `base_dir`; `waker` is called after each decode
    /// and each resample.
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
            scaling: HashSet::new(),
            landed: Vec::new(),
            kept: Vec::new(),
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

    /// Takes finished decodes and resamples. Returns the source lines whose
    /// image changed state or has a resampled box waiting in `scaled`.
    /// Boxes landed by the previous call and never claimed are dropped
    /// first. Never blocks.
    pub fn poll(&mut self) -> Vec<usize> {
        self.landed.clear();
        let Some(w) = &self.worker else {
            return Vec::new();
        };
        let mut changed_src: HashSet<String> = HashSet::new();
        let mut lost = false;
        loop {
            match w.done.try_recv() {
                Ok(Done::Decoded { src, pixmap }) => {
                    self.pending = self.pending.saturating_sub(1);
                    let Some(e) = self.entries.get_mut(&src) else {
                        continue;
                    };
                    e.state = match pixmap {
                        Some(p) => ImageState::Ready(Arc::new(p)),
                        None => ImageState::Missing,
                    };
                    changed_src.insert(src);
                }
                Ok(Done::Scaled { key, scaled }) => {
                    self.scaling.remove(&key);
                    if let Some(sc) = scaled {
                        changed_src.insert(key.src.clone());
                        self.landed.push((key, sc));
                    }
                }
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => {
                    lost = true;
                    break;
                }
            }
        }
        if lost {
            // The worker is gone (it panicked), so nothing in flight will
            // land: pending decodes become missing, and the next job starts
            // a new worker.
            self.worker = None;
            self.pending = 0;
            self.scaling.clear();
            for (src, e) in &mut self.entries {
                if matches!(e.state, ImageState::Pending) {
                    e.state = ImageState::Missing;
                    changed_src.insert(src.clone());
                }
            }
        }
        let mut changed: Vec<usize> = self
            .lines
            .iter()
            .filter(|(_, s)| changed_src.contains(*s))
            .map(|(l, _)| *l)
            .collect();
        changed.sort_unstable();
        changed
    }

    /// The image `src` resampled to `w` by `h` pixels, when a copy is ready.
    /// Otherwise, once `src` is decoded, asks the worker for it (once per
    /// box) and returns `None`; a later `poll` reports the lines showing
    /// `src`. Never resamples on the caller's thread.
    pub fn scaled(&mut self, src: &str, w: u32, h: u32) -> Option<Scaled> {
        let key = ScaleKey::new(src, w, h);
        let found = match self.kept.iter().position(|(k, _)| *k == key) {
            Some(i) => Some(self.kept.remove(i)),
            None => self
                .landed
                .iter()
                .position(|(k, _)| *k == key)
                .map(|i| self.landed.swap_remove(i)),
        };
        if let Some((key, sc)) = found {
            self.kept.insert(0, (key, sc.clone()));
            self.kept.truncate(SCALED_KEEP);
            return Some(sc);
        }
        let from = match self.entries.get(src).map(|e| &e.state) {
            Some(ImageState::Ready(px)) => px.clone(),
            _ => return None,
        };
        if w > 0 && h > 0 && !self.scaling.contains(&key) {
            let job = Job::Scale {
                key: key.clone(),
                from,
            };
            if self.queue(job) {
                self.scaling.insert(key);
            }
        }
        None
    }

    /// True while a copy of `src` at `w` by `h` is kept or waiting to be
    /// claimed (for tests and diagnostics).
    pub fn holds_scaled(&self, src: &str, w: u32, h: u32) -> bool {
        let key = ScaleKey::new(src, w, h);
        self.kept.iter().chain(&self.landed).any(|(k, _)| *k == key)
    }

    /// The resampled copies kept for reuse, at most `SCALED_KEEP`.
    pub fn scaled_held(&self) -> usize {
        self.kept.len()
    }

    /// Resamples asked for and not landed yet.
    pub fn resamples_queued(&self) -> usize {
        self.scaling.len()
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
        let queued = self.queue(Job::Decode {
            src: src.to_owned(),
            path,
            target,
        });
        if queued {
            self.pending += 1;
        }
        Entry {
            natural,
            // With no worker to decode it, it would stay pending forever.
            state: if queued {
                ImageState::Pending
            } else {
                ImageState::Missing
            },
        }
    }

    /// Sends `job` to the worker, starting it on the first job. False when
    /// the worker has gone.
    fn queue(&mut self, job: Job) -> bool {
        let worker = self.worker.get_or_insert_with(|| {
            let (jobs, job_rx) = channel::<Job>();
            let (done_tx, done) = channel::<Done>();
            let waker = self.waker.clone();
            std::thread::spawn(move || {
                while let Ok(first) = job_rx.recv() {
                    let mut batch = vec![first];
                    batch.extend(job_rx.try_iter());
                    if !run_batch(batch, &done_tx, &waker) {
                        break;
                    }
                }
            });
            Worker { jobs, done }
        });
        worker.jobs.send(job).is_ok()
    }
}
