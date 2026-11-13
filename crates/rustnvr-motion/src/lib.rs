//! RustNVR motion detection, without AI.
//!
//! For each camera with detection enabled, FFmpeg decodes its live stream
//! (the sub stream when there is one) into small grey pictures, a few per
//! second ([`ffmpeg`]); each picture is compared with a slowly adapting
//! background, inside the configured zones ([`detector`]); pictures with
//! motion become events with a start and an end ([`tracker`]).
//!
//! Detection only reads the live tap: recording and viewers are unaffected,
//! and a slow detector restarts from the next key frame instead of slowing
//! ingestion.

pub mod crossing;
pub mod detector;
pub mod ffmpeg;
pub mod night;
pub mod tracker;

use crate::detector::Detector;
use crate::ffmpeg::{FeedEnd, Ffmpeg, analysis_size, feed, grey_args, read_picture};
use crate::tracker::{Change, Tracker};
use rustnvr_core::config::MotionConfig;
use rustnvr_core::{CameraId, MotionSettings, VideoFrame};
use rustnvr_stream::LiveTap;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use tokio::sync::broadcast::error::RecvError;
use tokio::sync::{mpsc, watch};
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

pub use ffmpeg::{boxed_jpeg, mask_filters, snapshot_jpeg};

/// Delay before restarting a failed decoder, doubled up to [`MAX_RETRY`].
const FIRST_RETRY: Duration = Duration::from_secs(2);
const MAX_RETRY: Duration = Duration::from_secs(60);
/// A decoder running this long resets the retry delay.
const STABLE_AFTER: Duration = Duration::from_secs(60);

/// Settings shared by all cameras.
#[derive(Clone, Debug)]
pub struct MotionOptions {
    /// FFmpeg executable.
    pub ffmpeg: String,
    /// Pictures analysed per second.
    pub fps: u32,
    /// Width of the analysed picture.
    pub width: u32,
    /// An event ends after this long without motion.
    pub post: Duration,
    /// Longer events are split.
    pub max_event: Duration,
}

impl MotionOptions {
    /// Options from the `[motion]` section and the FFmpeg executable.
    pub fn new(config: &MotionConfig, ffmpeg: &str) -> Self {
        Self {
            ffmpeg: ffmpeg.to_owned(),
            fps: config.fps,
            width: config.width,
            post: Duration::from_secs(config.post_seconds.into()),
            max_event: Duration::from_secs(config.max_event_seconds.into()),
        }
    }
}

/// What the detection of a camera reports, times in milliseconds since the Unix epoch.
#[derive(Clone, Debug, PartialEq)]
pub enum MotionUpdate {
    /// An event started.
    Started {
        /// Camera.
        camera: CameraId,
        /// Start.
        start_ms: i64,
        /// Changed fraction, 0..1.
        score: f32,
        /// Zones with motion.
        zones: Vec<String>,
    },
    /// Motion was seen in more zones.
    Zones {
        /// Camera.
        camera: CameraId,
        /// Every zone with motion during the event.
        zones: Vec<String>,
    },
    /// The event ended.
    Ended {
        /// Camera.
        camera: CameraId,
        /// Last motion.
        end_ms: i64,
        /// Largest changed fraction of the event.
        score: f32,
        /// Every zone with motion during the event.
        zones: Vec<String>,
    },
    /// What moves crossed a line.
    Crossing {
        /// Camera.
        camera: CameraId,
        /// Name of the line.
        line: String,
        /// From the left of the line (as drawn) to its right.
        forward: bool,
        /// When.
        at_ms: i64,
    },
}

/// State of the detection of one camera.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct MotionStatus {
    /// Pictures are being analysed.
    pub running: bool,
    /// An event is ongoing.
    pub active: bool,
    /// Last decoder failure, cleared once pictures flow again.
    pub error: Option<String>,
}

/// A running detection.
pub struct MotionHandle {
    task: JoinHandle<()>,
    cancel: CancellationToken,
    /// Current state.
    pub status: watch::Receiver<MotionStatus>,
}

impl MotionHandle {
    /// Stops detection; an ongoing event is ended first.
    pub async fn stop(self) {
        self.cancel.cancel();
        if tokio::time::timeout(Duration::from_secs(5), self.task)
            .await
            .is_err()
        {
            tracing::warn!("motion detection did not stop in time");
        }
    }
}

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| i64::try_from(d.as_millis()).unwrap_or(i64::MAX))
}

/// Starts detection on `source`. Updates are sent to `sink`.
pub fn spawn_motion(
    camera: CameraId,
    source: LiveTap,
    settings: MotionSettings,
    options: MotionOptions,
    sink: mpsc::Sender<MotionUpdate>,
    cancel: CancellationToken,
) -> MotionHandle {
    let (status_tx, status) = watch::channel(MotionStatus::default());
    let runner = Runner {
        names: settings
            .zones
            .iter()
            .filter(|z| z.points.len() >= 3)
            .map(|z| z.name.clone())
            .collect(),
        tracker: Tracker::new(
            i64::try_from(options.post.as_millis()).unwrap_or(i64::MAX),
            i64::try_from(options.max_event.as_millis()).unwrap_or(i64::MAX),
        ),
        camera,
        source,
        settings,
        options,
        sink,
        status: status_tx,
        cancel: cancel.clone(),
    };
    MotionHandle {
        task: tokio::spawn(runner.run()),
        cancel,
        status,
    }
}

struct Runner {
    camera: CameraId,
    source: LiveTap,
    settings: MotionSettings,
    options: MotionOptions,
    sink: mpsc::Sender<MotionUpdate>,
    status: watch::Sender<MotionStatus>,
    cancel: CancellationToken,
    names: Vec<String>,
    tracker: Tracker,
}

/// How one decoding session ended.
enum Session {
    Cancelled,
    Restart(&'static str),
    Failed(String),
}

impl Runner {
    async fn emit(&mut self, change: Option<Change>) {
        let Some(change) = change else { return };
        let names = |idx: Vec<usize>| -> Vec<String> {
            idx.into_iter()
                .filter_map(|i| self.names.get(i).cloned())
                .collect()
        };
        let camera = self.camera.clone();
        let update = match change {
            Change::Started {
                start_ms,
                score,
                zones,
            } => MotionUpdate::Started {
                camera,
                start_ms,
                score,
                zones: names(zones),
            },
            Change::Zones { zones } => MotionUpdate::Zones {
                camera,
                zones: names(zones),
            },
            Change::Ended {
                end_ms,
                score,
                zones,
            } => MotionUpdate::Ended {
                camera,
                end_ms,
                score,
                zones: names(zones),
            },
        };
        let active = self.tracker.is_active();
        self.status.send_modify(|s| s.active = active);
        let _ = self.sink.send(update).await;
    }

    async fn run(mut self) {
        let mut retry = FIRST_RETRY;
        let mut tick = tokio::time::interval(Duration::from_secs(1));
        loop {
            let started = Instant::now();
            match self.session(&mut tick).await {
                Session::Cancelled => break,
                Session::Restart(reason) => {
                    tracing::debug!(camera = %self.camera, reason, "motion detection restarts from the next key frame");
                    continue;
                }
                Session::Failed(reason) => {
                    if started.elapsed() >= STABLE_AFTER {
                        retry = FIRST_RETRY;
                    }
                    if self.status.borrow().error.as_deref() != Some(reason.as_str()) {
                        tracing::warn!(camera = %self.camera, %reason, retry_in_s = retry.as_secs(), "motion detection: decoder failed");
                    }
                    self.status.send_modify(|s| {
                        s.running = false;
                        s.error = Some(reason);
                    });
                    let deadline = tokio::time::Instant::now() + retry;
                    loop {
                        tokio::select! {
                            _ = self.cancel.cancelled() => {
                                let end = self.tracker.finish();
                                self.emit(end).await;
                                return;
                            }
                            _ = tokio::time::sleep_until(deadline) => break,
                            _ = tick.tick() => {
                                let end = self.tracker.tick(now_ms());
                                self.emit(end).await;
                            }
                        }
                    }
                    retry = (retry * 2).min(MAX_RETRY);
                }
            }
        }
        let end = self.tracker.finish();
        self.emit(end).await;
        self.status.send_modify(|s| s.running = false);
    }

    /// Waits for a key frame, then decodes and analyses until the stream
    /// changes, FFmpeg fails or detection is stopped.
    async fn session(&mut self, tick: &mut tokio::time::Interval) -> Session {
        let (backlog, mut rx) = self.source.subscribe();
        let mut backlog = backlog.into_iter();
        let first: VideoFrame = loop {
            if let Some(f) = backlog.next() {
                break f; // the backlog starts with a key frame
            }
            tokio::select! {
                _ = self.cancel.cancelled() => return Session::Cancelled,
                _ = tick.tick() => {
                    let end = self.tracker.tick(now_ms());
                    self.emit(end).await;
                }
                r = rx.recv() => match r {
                    Ok(f) if f.is_key => break f,
                    Ok(_) | Err(RecvError::Lagged(_)) => {}
                    Err(RecvError::Closed) => return Session::Cancelled,
                },
            }
        };
        let (w, h) = analysis_size(&first.params, self.options.width);
        let mut process =
            match Ffmpeg::spawn(&self.options.ffmpeg, &grey_args(self.options.fps, w, h)) {
                Ok(p) => p,
                Err(e) => return Session::Failed(e),
            };
        let mut detector = Detector::new(w as usize, h as usize, self.options.fps, &self.settings);
        let mut lines = crossing::LineTracker::new(&self.settings.lines);

        // Pictures are read by their own task: a partial read must never be
        // interrupted by the other branches below.
        let (pictures_tx, mut pictures) = mpsc::channel::<Vec<u8>>(2);
        let mut stdout = process.stdout;
        let len = w as usize * h as usize;
        let reader = tokio::spawn(async move {
            loop {
                let mut buf = vec![0u8; len];
                match read_picture(&mut stdout, &mut buf).await {
                    Ok(Some(())) => {
                        if pictures_tx.send(buf).await.is_err() {
                            return;
                        }
                    }
                    Ok(None) | Err(_) => return,
                }
            }
        });
        let _abort_reader = AbortOnDrop(reader);
        let rest: Vec<VideoFrame> = backlog.collect();
        let feeding = feed(first, rest, rx, &mut process.stdin);
        tokio::pin!(feeding);
        let last_error = process.last_error.clone();
        let failed = |what: &str| {
            let detail = last_error.lock().unwrap_or_else(|e| e.into_inner()).clone();
            Session::Failed(if detail.is_empty() {
                format!("FFmpeg {what}")
            } else {
                format!("FFmpeg {what} : {detail}")
            })
        };
        loop {
            tokio::select! {
                _ = self.cancel.cancelled() => return Session::Cancelled,
                _ = tick.tick() => {
                    let end = self.tracker.tick(now_ms());
                    self.emit(end).await;
                }
                end = &mut feeding => return match end {
                    Ok(FeedEnd::Restart(reason)) => Session::Restart(reason),
                    Ok(FeedEnd::Closed) => Session::Cancelled,
                    Err(_) => failed("s'est arrêté"),
                },
                picture = pictures.recv() => {
                    let Some(picture) = picture else {
                        return failed("ne produit plus d'images");
                    };
                    if !self.status.borrow().running {
                        tracing::info!(camera = %self.camera, width = w, height = h, fps = self.options.fps, "motion detection running");
                        self.status.send_modify(|s| { s.running = true; s.error = None; });
                    }
                    let analysis = detector.process(&picture);
                    tracing::trace!(camera = %self.camera, motion = analysis.motion, score = analysis.score, "picture analysed");
                    let now = now_ms();
                    for change in self.tracker.update(now, &analysis) {
                        self.emit(Some(change)).await;
                    }
                    if !lines.is_empty() {
                        for c in lines.update(analysis.centre) {
                            let _ = self.sink.send(MotionUpdate::Crossing {
                                camera: self.camera.clone(),
                                line: lines.name(c.line).to_owned(),
                                forward: c.forward,
                                at_ms: now,
                            }).await;
                        }
                    }
                }
            }
        }
    }
}

struct AbortOnDrop(JoinHandle<()>);

impl Drop for AbortOnDrop {
    fn drop(&mut self) {
        self.0.abort();
    }
}
