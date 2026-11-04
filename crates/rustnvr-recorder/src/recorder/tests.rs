#![allow(clippy::unwrap_used)]

use super::*;
use crate::recovery::{RecoveryOutcome, recover_part_file, recover_tree};
use crate::scan::{ScanReport, scan};
use crate::store::{FsStore, SegmentFile, SegmentStore};
use rustnvr_core::VideoCodec;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::AtomicBool;

const RATE: u32 = 90_000;
const FRAME: i64 = 3600; // 25 fps

fn params(variant: u8) -> Arc<VideoParams> {
    let mut entry = vec![0, 0, 0, 16];
    entry.extend_from_slice(b"avc1");
    entry.extend_from_slice(&[variant; 8]);
    Arc::new(VideoParams {
        codec: VideoCodec::H264,
        rfc6381_codec: "avc1.42C01E".into(),
        width: 320,
        height: 240,
        clock_rate: RATE,
        sample_entry: Bytes::from(entry),
    })
}

struct Gen {
    params: Arc<VideoParams>,
    session: u64,
    seq: u64,
    pts: i64,
    gop: u64,
}

impl Gen {
    fn new(gop: u64) -> Self {
        Self {
            params: params(0),
            session: 1,
            seq: 0,
            pts: 0,
            gop,
        }
    }

    /// Next frame of a regular stream with a key frame every `gop` frames.
    fn next(&mut self) -> VideoFrame {
        let is_key = self.seq.is_multiple_of(self.gop);
        let f = VideoFrame {
            session: self.session,
            seq: self.seq,
            params: self.params.clone(),
            pts: self.pts,
            is_key,
            wall_time: SystemTime::UNIX_EPOCH
                + Duration::from_secs(1_793_557_353)
                + Duration::from_micros(self.pts as u64 * 100 / 9),
            loss: 0,
            audio: None,
            data: Bytes::from(vec![if is_key { 0x65 } else { 0x41 }; 100]),
        };
        self.seq += 1;
        self.pts += FRAME;
        f
    }
}

fn settings() -> RecorderSettings {
    RecorderSettings {
        segment_duration: Duration::from_secs(2),
        fragment_duration: Duration::from_millis(500),
        max_timestamp_jump: Duration::from_secs(5),
        retry_after_error: Duration::from_millis(50),
    }
}

fn camera() -> CameraId {
    CameraId::new("cam").unwrap()
}

fn finished(events: &[NvrEvent]) -> Vec<SegmentInfo> {
    events
        .iter()
        .filter_map(|e| match e {
            NvrEvent::SegmentFinished(i) => Some(i.clone()),
            _ => None,
        })
        .collect()
}

fn scan_path(p: &Path) -> ScanReport {
    scan(&mut std::fs::File::open(p).unwrap()).unwrap()
}

fn assert_well_formed(report: &ScanReport) {
    assert_eq!(report.timescale, Some(RATE));
    assert_eq!(report.sample_entry, Some(*b"avc1"));
    assert_eq!(report.valid_len, report.file_len, "trailing garbage");
    assert!(report.fragments.first().unwrap().starts_with_key);
    let mut t = 0;
    for (i, f) in report.fragments.iter().enumerate() {
        assert_eq!(f.sequence as usize, i + 1);
        assert_eq!(f.base_decode_time, t, "decode times must be contiguous");
        t += f.duration;
    }
}

#[test]
fn rotates_segments_on_key_frames() {
    let dir = tempfile::tempdir().unwrap();
    let mut core = RecorderCore::new(camera(), settings(), Box::new(FsStore::new(dir.path())));
    let mut g = Gen::new(25);
    for _ in 0..175 {
        core.handle_frame(g.next());
    }
    core.shutdown();
    let segs = finished(&core.drain_events());
    assert_eq!(segs.len(), 4, "{segs:#?}");
    for s in &segs[..3] {
        assert_eq!(s.duration, Duration::from_secs(2));
        assert_eq!(s.frames, 50);
        let r = scan_path(&s.path);
        assert_well_formed(&r);
        assert_eq!(r.duration(), 2 * u64::from(RATE));
        assert_eq!(r.samples(), 50);
        // One fragment per second-long GOP.
        assert_eq!(r.fragments.len(), 2);
        assert_eq!(std::fs::metadata(&s.path).unwrap().len(), s.bytes);
    }
    let last = &segs[3];
    assert_eq!(last.frames, 25);
    assert_eq!(last.duration, Duration::from_secs(1));
    assert_eq!(scan_path(&last.path).duration(), u64::from(RATE));
    // Files are renamed into place and nothing is left behind.
    let part_files: Vec<_> = walk(dir.path())
        .into_iter()
        .filter(|p| p.to_string_lossy().ends_with(".part"))
        .collect();
    assert!(part_files.is_empty(), "{part_files:?}");
    assert!(
        segs[0]
            .path
            .starts_with(dir.path().join("cam").join("2026-11-01"))
    );
}

#[test]
fn long_gop_forces_fragments_without_key_frames() {
    let dir = tempfile::tempdir().unwrap();
    let mut core = RecorderCore::new(camera(), settings(), Box::new(FsStore::new(dir.path())));
    let mut g = Gen::new(1000); // a single key frame
    for _ in 0..100 {
        core.handle_frame(g.next());
    }
    core.shutdown();
    let segs = finished(&core.drain_events());
    assert_eq!(segs.len(), 1, "segments are only cut on key frames");
    let r = scan_path(&segs[0].path);
    assert_well_formed(&r);
    assert_eq!(r.samples(), 100);
    // Forced every 2 x 500 ms.
    assert!(r.fragments.len() >= 4, "{}", r.fragments.len());
    assert_eq!(r.fragments.iter().map(|f| f.key_samples).sum::<u32>(), 1);
}

#[test]
fn waits_for_first_key_frame() {
    let dir = tempfile::tempdir().unwrap();
    let mut core = RecorderCore::new(camera(), settings(), Box::new(FsStore::new(dir.path())));
    let mut g = Gen::new(25);
    g.seq = 10; // start mid-GOP
    g.pts = 10 * FRAME;
    for _ in 0..40 {
        core.handle_frame(g.next());
    }
    core.shutdown();
    assert_eq!(core.metrics().frames_skipped.load(Ordering::Relaxed), 15);
    let segs = finished(&core.drain_events());
    assert_eq!(segs.len(), 1);
    assert_eq!(segs[0].frames, 25);
}

fn run_with_discontinuity(mutate: impl FnOnce(&mut Gen)) -> (Vec<SegmentInfo>, u64) {
    let dir = tempfile::tempdir().unwrap();
    let mut core = RecorderCore::new(camera(), settings(), Box::new(FsStore::new(dir.path())));
    let mut g = Gen::new(10);
    for _ in 0..15 {
        core.handle_frame(g.next());
    }
    mutate(&mut g);
    for _ in 0..20 {
        core.handle_frame(g.next());
    }
    core.shutdown();
    let segs = finished(&core.drain_events());
    for s in &segs {
        assert_well_formed(&scan_path(&s.path));
    }
    // Keep the files alive for the assertions above only.
    drop(dir);
    (segs, core.metrics().discontinuities.load(Ordering::Relaxed))
}

#[test]
fn dropped_frames_close_segment() {
    let (segs, disc) = run_with_discontinuity(|g| {
        g.seq += 3;
        g.pts += 3 * FRAME;
    });
    assert_eq!(disc, 1);
    assert_eq!(segs.len(), 2);
    assert_eq!(segs[0].frames, 15);
}

#[test]
fn new_session_closes_segment() {
    let (segs, disc) = run_with_discontinuity(|g| {
        g.session = 2;
        g.seq = 0;
        g.pts = 0;
    });
    assert_eq!(disc, 1);
    assert_eq!(segs.len(), 2);
    assert_eq!(segs[0].frames, 15);
    assert_eq!(segs[1].frames, 20);
}

#[test]
fn parameter_change_closes_segment() {
    let (segs, disc) = run_with_discontinuity(|g| g.params = params(1));
    assert_eq!(disc, 1);
    assert_eq!(segs.len(), 2);
}

#[test]
fn timestamp_backwards_or_jump_closes_segment() {
    let (segs, disc) = run_with_discontinuity(|g| g.pts -= 10 * FRAME);
    assert_eq!((segs.len(), disc), (2, 1));
    let (segs, disc) = run_with_discontinuity(|g| g.pts += 60 * i64::from(RATE));
    assert_eq!((segs.len(), disc), (2, 1));
}

#[test]
fn session_end_closes_segment() {
    let dir = tempfile::tempdir().unwrap();
    let mut core = RecorderCore::new(camera(), settings(), Box::new(FsStore::new(dir.path())));
    let mut g = Gen::new(10);
    for _ in 0..12 {
        core.handle_frame(g.next());
    }
    core.handle_session_end(99); // another session: ignored
    assert!(core.is_recording());
    core.handle_session_end(1);
    assert!(!core.is_recording());
    let segs = finished(&core.drain_events());
    assert_eq!(segs.len(), 1);
    assert_eq!(segs[0].frames, 12);
}

/// In-memory store whose files fail with "storage full" once `fail` is set.
struct FlakyStore {
    fail: Arc<AtomicBool>,
    abandoned: Arc<Mutex<u32>>,
}

struct FlakyFile {
    fail: Arc<AtomicBool>,
    abandoned: Arc<Mutex<u32>>,
    path: PathBuf,
}

impl std::io::Write for FlakyFile {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        if self.fail.load(Ordering::SeqCst) {
            Err(io::Error::new(
                io::ErrorKind::StorageFull,
                "No space left on device",
            ))
        } else {
            Ok(buf.len())
        }
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

impl SegmentFile for FlakyFile {
    fn path(&self) -> &Path {
        &self.path
    }
    fn finish(self: Box<Self>) -> io::Result<PathBuf> {
        Ok(self.path.clone())
    }
    fn abandon(self: Box<Self>) -> Option<PathBuf> {
        *self.abandoned.lock().unwrap() += 1;
        None
    }
}

impl SegmentStore for FlakyStore {
    fn create(&mut self, _: &CameraId, _: SystemTime) -> io::Result<Box<dyn SegmentFile>> {
        Ok(Box::new(FlakyFile {
            fail: self.fail.clone(),
            abandoned: self.abandoned.clone(),
            path: PathBuf::from("/virtual.mp4"),
        }))
    }
}

#[test]
fn write_errors_abandon_segment_and_retry_later() {
    let fail = Arc::new(AtomicBool::new(false));
    let abandoned = Arc::new(Mutex::new(0));
    let store = FlakyStore {
        fail: fail.clone(),
        abandoned: abandoned.clone(),
    };
    let mut core = RecorderCore::new(camera(), settings(), Box::new(store));
    let mut g = Gen::new(10);
    for _ in 0..12 {
        core.handle_frame(g.next());
    }
    fail.store(true, Ordering::SeqCst); // disk full
    for _ in 0..20 {
        core.handle_frame(g.next());
    }
    let events = core.drain_events();
    let errors: Vec<_> = events
        .iter()
        .filter(|e| matches!(e, NvrEvent::RecorderError { message, .. } if message.contains("No space")))
        .collect();
    assert_eq!(errors.len(), 1, "{events:#?}");
    assert_eq!(*abandoned.lock().unwrap(), 1);
    assert!(!core.is_recording());

    // Space is back; recording resumes on a key frame once the delay expired.
    fail.store(false, Ordering::SeqCst);
    std::thread::sleep(Duration::from_millis(60));
    for _ in 0..25 {
        core.handle_frame(g.next());
    }
    assert!(core.is_recording());
    core.shutdown();
    assert_eq!(finished(&core.drain_events()).len(), 1);
    assert_eq!(core.metrics().write_errors.load(Ordering::Relaxed), 1);
}

fn walk(dir: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    for e in std::fs::read_dir(dir).unwrap().flatten() {
        let p = e.path();
        if p.is_dir() {
            out.extend(walk(&p));
        } else {
            out.push(p);
        }
    }
    out
}

/// Writes an init segment plus `fragments` complete fragments of 5 frames,
/// then `extra` garbage bytes simulating a torn write.
fn write_partial(path: &Path, fragments: u32, torn_tail: &[u8]) {
    let p = params(0);
    let mut data = fmp4::init_segment(&p);
    for i in 0..fragments {
        let metas: Vec<_> = (0..5)
            .map(|k| SampleMeta {
                duration: 3600,
                size: 50,
                is_key: k == 0,
            })
            .collect();
        data.extend(fmp4::fragment_header(i + 1, u64::from(i) * 18_000, &metas));
        data.extend(std::iter::repeat_n(0xab, 250));
    }
    data.extend_from_slice(torn_tail);
    std::fs::write(path, data).unwrap();
}

#[test]
fn recovery_truncates_torn_fragment() {
    let dir = tempfile::tempdir().unwrap();
    let part = dir.path().join("x.mp4.part");
    // A torn third fragment: a moof header that announces more than is there.
    let torn = fmp4::fragment_header(
        3,
        36_000,
        &[SampleMeta {
            duration: 3600,
            size: 5000,
            is_key: true,
        }],
    );
    write_partial(&part, 2, &[&torn[..], &[1, 2, 3][..]].concat());
    let outcome = recover_part_file(&part).unwrap();
    let RecoveryOutcome::Recovered {
        path,
        fragments,
        truncated_bytes,
    } = outcome
    else {
        panic!("expected recovery, got {outcome:?}");
    };
    assert_eq!(fragments, 2);
    assert_eq!(truncated_bytes as usize, torn.len() + 3);
    assert_eq!(path, dir.path().join("x.mp4"));
    assert!(!part.exists());
    let r = scan_path(&path);
    assert_well_formed(&r);
    assert_eq!(r.duration(), 2 * 18_000);
}

#[test]
fn recovery_deletes_files_without_fragments_and_walks_tree() {
    let dir = tempfile::tempdir().unwrap();
    let sub = dir.path().join("cam").join("2026-11-01");
    std::fs::create_dir_all(&sub).unwrap();
    write_partial(&sub.join("a.mp4.part"), 0, b"moo");
    write_partial(&sub.join("b.mp4.part"), 1, &[]);
    std::fs::write(sub.join("c.mp4"), b"complete segments are left alone").unwrap();
    let report = recover_tree(dir.path());
    assert_eq!(report.deleted, vec![sub.join("a.mp4.part")]);
    assert_eq!(report.recovered, vec![sub.join("b.mp4")]);
    assert!(report.failed.is_empty());
    assert!(recover_tree(&dir.path().join("missing")).failed.is_empty());
}

#[test]
fn fragment_roundtrip_through_scanner() {
    let mut data = fmp4::init_segment(&params(0));
    let metas = [
        SampleMeta {
            duration: 3000,
            size: 10,
            is_key: true,
        },
        SampleMeta {
            duration: 3001,
            size: 20,
            is_key: false,
        },
        SampleMeta {
            duration: 2999,
            size: 30,
            is_key: false,
        },
    ];
    let header = fmp4::fragment_header(7, 123_456_789_012, &metas);
    data.extend_from_slice(&header);
    data.extend(std::iter::repeat_n(0, 60));
    let r = scan(&mut std::io::Cursor::new(&data)).unwrap();
    assert_eq!(r.fragments.len(), 1);
    let f = &r.fragments[0];
    assert_eq!(
        (
            f.sequence,
            f.base_decode_time,
            f.duration,
            f.samples,
            f.key_samples
        ),
        (7, 123_456_789_012, 9000, 3, 1)
    );
    assert!(f.starts_with_key);
    assert_eq!(r.valid_len, data.len() as u64);
}

// --- Sound ---------------------------------------------------------------

const AUDIO_RATE: u32 = 16_000;
const AAC_FRAME: u32 = 1024; // 64 ms at 16 kHz

fn audio_params() -> Arc<rustnvr_core::AudioParams> {
    let mut entry = vec![0, 0, 0, 16];
    entry.extend_from_slice(b"mp4a");
    entry.extend_from_slice(&[0; 8]);
    Arc::new(rustnvr_core::AudioParams {
        rfc6381_codec: "mp4a.40.2".into(),
        clock_rate: AUDIO_RATE,
        channels: 1,
        sample_entry: Bytes::from(entry),
    })
}

/// Video frames (25 fps) carrying the AAC frames received before each of
/// them, as `rustnvr_stream` delivers them.
struct AvGen {
    video: Gen,
    params: Arc<rustnvr_core::AudioParams>,
    next_audio: i64,
}

impl AvGen {
    fn new(gop: u64) -> Self {
        Self {
            video: Gen::new(gop),
            params: audio_params(),
            next_audio: 0,
        }
    }

    fn next(&mut self) -> VideoFrame {
        let mut f = self.video.next();
        let until = f.pts * i64::from(AUDIO_RATE) / i64::from(RATE);
        let mut samples = Vec::new();
        while self.next_audio <= until {
            samples.push(rustnvr_core::AudioSample {
                pts: self.next_audio,
                duration: AAC_FRAME,
                data: Bytes::from(vec![0x21; 30]),
            });
            self.next_audio += i64::from(AAC_FRAME);
        }
        f.audio = Some(Arc::new(rustnvr_core::FrameAudio {
            params: self.params.clone(),
            samples,
        }));
        f
    }
}

fn audio_samples(r: &ScanReport) -> u64 {
    r.fragments.iter().map(|f| u64::from(f.audio_samples)).sum()
}

#[test]
fn records_sound_in_the_same_fragments() {
    let dir = tempfile::tempdir().unwrap();
    let mut core = RecorderCore::new(camera(), settings(), Box::new(FsStore::new(dir.path())));
    let mut g = AvGen::new(25);
    for _ in 0..150 {
        core.handle_frame(g.next());
    }
    core.shutdown();
    let segs = finished(&core.drain_events());
    assert_eq!(segs.len(), 3, "{segs:#?}");
    let mut total_audio = 0;
    for s in &segs {
        let r = scan_path(&s.path);
        // Video unchanged: durations, samples, contiguous decode times.
        assert_well_formed(&r);
        assert!(r.has_audio);
        assert_eq!(r.duration(), 2 * u64::from(RATE));
        assert_eq!(r.samples(), 50);
        assert_eq!(std::fs::metadata(&s.path).unwrap().len(), s.bytes);
        // 2 s of sound: 31 or 32 AAC frames of 64 ms.
        let a = audio_samples(&r);
        assert!((30..=32).contains(&a), "{a} audio samples");
        total_audio += a;
    }
    // Only the AAC frames before the first picture can be missing.
    let sent = (6 * AUDIO_RATE / AAC_FRAME) as u64;
    assert!(total_audio + 3 >= sent, "{total_audio} of {sent}");
}

#[test]
fn sound_can_be_switched_off_and_on() {
    let dir = tempfile::tempdir().unwrap();
    let switch = Arc::new(AtomicBool::new(false));
    let mut core = RecorderCore::new(camera(), settings(), Box::new(FsStore::new(dir.path())))
        .with_audio_switch(switch.clone());
    let mut g = AvGen::new(25);
    // Off: no audio track.
    for _ in 0..30 {
        core.handle_frame(g.next());
    }
    // On: a new segment with sound starts at the next key frame (frame 50),
    // without waiting for the 2 s segment duration.
    switch.store(true, Ordering::Relaxed);
    for _ in 30..80 {
        core.handle_frame(g.next());
    }
    // Off again: the sound stops in the current segment.
    switch.store(false, Ordering::Relaxed);
    for _ in 80..100 {
        core.handle_frame(g.next());
    }
    core.shutdown();
    let segs = finished(&core.drain_events());
    assert_eq!(segs.len(), 2, "{segs:#?}");
    let (first, second) = (scan_path(&segs[0].path), scan_path(&segs[1].path));
    assert!(!first.has_audio);
    assert_eq!(first.samples(), 50, "rotated at the key frame");
    assert_eq!(audio_samples(&first), 0);
    assert!(second.has_audio);
    assert_eq!(second.valid_len, second.file_len);
    // Sound from frame 50 to frame 80 only (1.2 s ≈ 18 AAC frames).
    let a = audio_samples(&second);
    assert!((16..=20).contains(&a), "{a} audio samples");
}

#[test]
fn recovery_keeps_fragments_with_sound() {
    let dir = tempfile::tempdir().unwrap();
    let mut store = FsStore::new(dir.path());
    let mut file = store
        .create(
            &camera(),
            SystemTime::UNIX_EPOCH + Duration::from_secs(1_793_557_353),
        )
        .unwrap();
    let p = params(0);
    let a = audio_params();
    file.write_all(&fmp4::init_segment_av(&p, Some(&a)))
        .unwrap();
    for i in 0..3u32 {
        let video = [SampleMeta {
            duration: 90_000,
            size: 100,
            is_key: true,
        }];
        let audio = vec![
            SampleMeta {
                duration: AAC_FRAME,
                size: 30,
                is_key: true,
            };
            15
        ];
        file.write_all(&fmp4::fragment_header_av(
            i + 1,
            u64::from(i) * 90_000,
            &video,
            Some((u64::from(i) * 15 * u64::from(AAC_FRAME), &audio)),
        ))
        .unwrap();
        file.write_all(&[0x65; 100]).unwrap();
        file.write_all(&[0x21; 30 * 15]).unwrap();
    }
    // A torn fourth fragment.
    file.write_all(&fmp4::fragment_header(
        4,
        270_000,
        &[SampleMeta {
            duration: 90_000,
            size: 100,
            is_key: true,
        }],
    ))
    .unwrap();
    file.write_all(&[0x65; 10]).unwrap();
    file.flush().unwrap();
    let part = file.path().to_path_buf();
    drop(file);

    let before = scan_path(&part);
    assert!(before.has_audio);
    assert_eq!(before.fragments.len(), 3, "audio data counted in the mdat");
    assert_eq!(audio_samples(&before), 45);
    assert_eq!(
        before.duration(),
        3 * 90_000,
        "duration from the video track"
    );
    match recover_part_file(&part).unwrap() {
        RecoveryOutcome::Recovered { path, .. } => {
            let after = scan_path(&path);
            assert_eq!(after.valid_len, after.file_len);
            assert_eq!(after.fragments.len(), 3);
        }
        other => panic!("{other:?}"),
    }
}
