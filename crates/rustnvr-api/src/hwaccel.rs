//! H.264 conversions on the graphics card: NVIDIA (NVENC), Intel Quick Sync
//! and AMD (VAAPI).
//!
//! When `live.hwaccel = "auto"`, short test conversions are run at startup
//! (and when the setting changes): NVENC when an NVIDIA card is reachable
//! (`/dev/nvidia…`, or `/dev/dxg` under WSL), then VAAPI on each Intel or AMD
//! card RustNVR may use (`/dev/dri/renderD…`) with the default driver then,
//! for Intel, the two Intel drivers by name. The first that works is used by
//! the live, playback and export conversions. Without a card, or when every
//! test fails, conversions stay on the processor (libx264), as before.
//!
//! Pipeline: the card decodes when it can (FFmpeg decodes on the processor a
//! codec the card does not support), the pictures are filtered on the
//! processor (scaling, privacy masks, 8-bit 4:2:0 for H.264), then encoded by
//! the card. A conversion that fails on the card is redone on the processor.

use crate::dto::GpuDto;
use std::process::Stdio;
use std::time::Duration;
use tokio::io::AsyncReadExt as _;
use tokio::process::Command;

/// A test conversion taking longer than this fails.
const TEST_TIMEOUT: Duration = Duration::from_secs(20);

/// How FFmpeg talks to the card.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HwApi {
    /// NVIDIA: `h264_nvenc`, decoding with CUDA.
    Nvenc,
    /// Intel, AMD: `h264_vaapi`.
    Vaapi,
}

impl HwApi {
    /// `nvenc` or `vaapi`.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Nvenc => "nvenc",
            Self::Vaapi => "vaapi",
        }
    }
}

/// A card that passed the test conversion.
#[derive(Clone, Debug, PartialEq)]
pub struct HwEncoder {
    /// How FFmpeg uses it.
    pub api: HwApi,
    /// VAAPI render node (`/dev/dri/renderD128`); `None` for NVENC.
    pub device: Option<String>,
    /// `LIBVA_DRIVER_NAME` that worked (VAAPI), `None` for the default.
    pub driver: Option<String>,
    /// VAAPI driver offering constant QP only (Debian's free `iHD` on some
    /// Intel cards: "supported modes: CQP"): live conversions at a constant
    /// quality instead of a capped bit rate.
    pub constant_qp: bool,
    /// Card name, for the interface and the log.
    pub card: String,
}

impl HwEncoder {
    /// Selects the VAAPI driver that passed the test.
    pub(crate) fn apply_env(&self, cmd: &mut Command) {
        if let Some(d) = &self.driver {
            cmd.env("LIBVA_DRIVER_NAME", d);
        }
    }
}

fn owned(args: &[&str]) -> Vec<String> {
    args.iter().map(|s| (*s).to_owned()).collect()
}

/// Arguments before `-i`: decoding on the card (pictures brought back to the
/// processor, so that software decoding stays possible).
pub(crate) fn input_args(hw: Option<&HwEncoder>) -> Vec<String> {
    match hw {
        None => Vec::new(),
        Some(HwEncoder {
            api: HwApi::Nvenc, ..
        }) => owned(&["-hwaccel", "cuda"]),
        Some(hw) => vec![
            "-init_hw_device".to_owned(),
            format!("vaapi=va:{}", hw.device.as_deref().unwrap_or_default()),
            "-hwaccel".to_owned(),
            "vaapi".to_owned(),
            "-hwaccel_device".to_owned(),
            "va".to_owned(),
            "-filter_hw_device".to_owned(),
            "va".to_owned(),
        ],
    }
}

/// Filter chain: `filters` (processor), then what the card's encoder needs:
/// 8-bit 4:2:0 pictures (10-bit H.265 cameras), uploaded for VAAPI. `None`:
/// no filter needed.
pub(crate) fn filter_chain(hw: Option<&HwEncoder>, filters: &str) -> Option<String> {
    let tail = match hw.map(|h| h.api) {
        None => return (!filters.is_empty()).then(|| filters.to_owned()),
        Some(HwApi::Nvenc) => "format=yuv420p",
        Some(HwApi::Vaapi) => "format=nv12,hwupload",
    };
    Some(if filters.is_empty() {
        tail.to_owned()
    } else {
        format!("{filters},{tail}")
    })
}

/// Card encoders spend more bits than libx264 at the same number: added to
/// the CRF. NVENC: measured (1080p H.265 → 720p H.264, 30 s: CRF 26 3.1 MB,
/// CQ 26 10.8 MB, CQ 32 4.6 MB). VAAPI: estimated, constant QP.
const NVENC_QUALITY_OFFSET: u8 = 6;
const VAAPI_QUALITY_OFFSET: u8 = 4;

/// H.264 encoder of constant quality `quality` (libx264 CRF; on the card,
/// constant QP / CQ raised to give files of a comparable size), without
/// B-frames on the card.
pub(crate) fn encoder_args(hw: Option<&HwEncoder>, quality: u8, preset: &str) -> Vec<String> {
    let q = |offset: u8| quality.saturating_add(offset).min(51).to_string();
    match hw.map(|h| h.api) {
        None => owned(&[
            "-c:v",
            "libx264",
            "-preset",
            preset,
            "-crf",
            &q(0),
            "-pix_fmt",
            "yuv420p",
        ]),
        Some(HwApi::Vaapi) => owned(&[
            "-c:v",
            "h264_vaapi",
            "-bf",
            "0",
            "-qp",
            &q(VAAPI_QUALITY_OFFSET),
        ]),
        Some(HwApi::Nvenc) => owned(&[
            "-c:v",
            "h264_nvenc",
            "-preset",
            "p4",
            "-bf",
            "0",
            "-rc",
            "vbr",
            "-cq",
            &q(NVENC_QUALITY_OFFSET),
            "-b:v",
            "0",
        ]),
    }
}

/// Constant quality of live conversions (libx264 CRF).
const LIVE_QUALITY: u8 = 26;

/// Live conversion: filters and encoder, low latency, bit rate capped at
/// `rate` (`buf`: rate control buffer), or at a constant quality on a VAAPI
/// driver that offers nothing else.
pub(crate) fn live_encoder_args(hw: Option<&HwEncoder>, rate: &str, buf: &str) -> Vec<String> {
    if hw.is_some_and(|h| h.api == HwApi::Vaapi && h.constant_qp) {
        return owned(&[
            "-vf",
            "format=nv12,hwupload",
            "-c:v",
            "h264_vaapi",
            "-bf",
            "0",
            "-rc_mode",
            "CQP",
            "-qp",
            &LIVE_QUALITY
                .saturating_add(VAAPI_QUALITY_OFFSET)
                .to_string(),
        ]);
    }
    match hw.map(|h| h.api) {
        None => owned(&[
            "-c:v",
            "libx264",
            "-preset",
            "ultrafast",
            "-tune",
            "zerolatency",
            "-pix_fmt",
            "yuv420p",
            "-crf",
            &LIVE_QUALITY.to_string(),
            "-maxrate",
            rate,
            "-bufsize",
            buf,
        ]),
        // No B-frames: one output frame per input frame, at once.
        Some(HwApi::Vaapi) => owned(&[
            "-vf",
            "format=nv12,hwupload",
            "-c:v",
            "h264_vaapi",
            "-bf",
            "0",
            "-b:v",
            rate,
            "-maxrate",
            rate,
            "-bufsize",
            buf,
        ]),
        // Forced key frames as IDR: new viewers start on them.
        Some(HwApi::Nvenc) => owned(&[
            "-vf",
            "format=yuv420p",
            "-c:v",
            "h264_nvenc",
            "-preset",
            "p1",
            "-tune",
            "ll",
            "-bf",
            "0",
            "-forced-idr",
            "1",
            "-rc",
            "vbr",
            "-b:v",
            rate,
            "-maxrate",
            rate,
            "-bufsize",
            buf,
        ]),
    }
}

/// Why no card is used.
#[derive(Debug, PartialEq)]
pub(crate) enum NoEncoder {
    /// No NVIDIA card reachable, no Intel or AMD card RustNVR may open.
    NoCard,
    /// Every test conversion failed (last reason).
    Failed(String),
}

/// What [`detect`] may try.
#[derive(Clone, Debug, PartialEq)]
enum Candidate {
    Nvenc {
        card: String,
    },
    Vaapi {
        card: String,
        device: String,
        vendor: &'static str,
    },
}

/// Devices showing an NVIDIA card outside `/sys` (Docker with the NVIDIA
/// toolkit, WSL).
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct NvidiaHints {
    /// `/dev/nvidiactl` exists.
    pub nvidiactl: bool,
    /// `/dev/dxg` exists (WSL: the Windows graphics driver).
    pub dxg: bool,
}

impl NvidiaHints {
    /// From `/dev`.
    pub(crate) fn read() -> Self {
        Self {
            nvidiactl: std::path::Path::new("/dev/nvidiactl").exists(),
            dxg: std::path::Path::new("/dev/dxg").exists(),
        }
    }
}

/// Cards to try, NVIDIA first.
fn candidates(gpus: &[GpuDto], hints: NvidiaHints) -> Vec<Candidate> {
    let mut out = Vec::new();
    let nvidia = gpus
        .iter()
        .find(|g| g.vendor == "nvidia" && g.status == "usable");
    if nvidia.is_some() || hints.nvidiactl || hints.dxg {
        out.push(Candidate::Nvenc {
            card: nvidia.map_or_else(|| "NVIDIA".to_owned(), |g| g.name.clone()),
        });
    }
    for g in gpus {
        if g.status == "usable"
            && matches!(g.vendor, "intel" | "amd")
            && let Some(device) = g.device.as_deref().filter(|d| d.starts_with("/dev/dri/"))
        {
            out.push(Candidate::Vaapi {
                card: g.name.clone(),
                device: device.to_owned(),
                vendor: g.vendor,
            });
        }
    }
    out
}

/// VAAPI drivers to try for a card: the default one, then by name.
fn vaapi_drivers(vendor: &str) -> &'static [Option<&'static str>] {
    match vendor {
        "intel" => &[None, Some("iHD"), Some("i965")],
        _ => &[None],
    }
}

/// Arguments of a test conversion: one second of a test pattern, encoded on
/// the card with the live settings (at a constant QP when `constant_qp`),
/// written nowhere.
fn test_args(api: HwApi, device: Option<&str>, constant_qp: bool) -> Vec<String> {
    let hw = HwEncoder {
        api,
        device: device.map(str::to_owned),
        driver: None,
        constant_qp,
        card: String::new(),
    };
    let mut args = owned(&["-hide_banner", "-nostdin", "-loglevel", "error"]);
    if api == HwApi::Vaapi {
        args.extend([
            "-init_hw_device".to_owned(),
            format!("vaapi=va:{}", device.unwrap_or_default()),
            "-filter_hw_device".to_owned(),
            "va".to_owned(),
        ]);
    }
    args.extend(owned(&[
        "-f",
        "lavfi",
        "-i",
        "testsrc2=size=1280x720:rate=25",
        "-frames:v",
        "25",
    ]));
    args.extend(live_encoder_args(Some(&hw), "2000k", "4000k"));
    args.extend(owned(&["-f", "null", "-"]));
    args
}

/// Lines FFmpeg ends a failed run with, which never tell why.
const GENERIC_FAILURES: &[&str] = &[
    "Nothing was written into output file",
    "Conversion failed",
    "Error while filtering",
    "Error sending frames to consumers",
    "Task finished with error code",
    "Terminating thread with return code",
    "Error opening output file",
    "Error opening output files",
    "Error while opening encoder",
];

/// Why a failed FFmpeg run failed, from its error output: the first line that
/// says why ("Driver does not support any RC mode…"), and the last one when
/// it differs (the final summary).
pub(crate) fn failure_reason(stderr: &str) -> Option<String> {
    let lines: Vec<&str> = stderr
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .collect();
    let last = *lines.last()?;
    let cause = lines
        .iter()
        .find(|l| !GENERIC_FAILURES.iter().any(|g| l.contains(g)))
        .copied();
    Some(match cause {
        Some(c) if c != last => format!("{c} (puis : {last})"),
        _ => last.to_owned(),
    })
}

/// Runs one test conversion; why FFmpeg failed when it does.
async fn test(ffmpeg: &str, args: Vec<String>, driver: Option<&str>) -> Result<(), String> {
    let mut cmd = Command::new(ffmpeg);
    cmd.args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    if let Some(d) = driver {
        cmd.env("LIBVA_DRIVER_NAME", d);
    }
    let mut child = cmd
        .spawn()
        .map_err(|e| format!("FFmpeg ne démarre pas ({ffmpeg:?}) : {e}"))?;
    let mut stderr = child.stderr.take();
    let run = async {
        let mut text = String::new();
        if let Some(s) = stderr.as_mut() {
            let _ = s.read_to_string(&mut text).await;
        }
        (child.wait().await, text)
    };
    match tokio::time::timeout(TEST_TIMEOUT, run).await {
        Err(_) => Err(format!(
            "essai trop long (plus de {} s)",
            TEST_TIMEOUT.as_secs()
        )),
        Ok((Err(e), _)) => Err(e.to_string()),
        Ok((Ok(status), _)) if status.success() => Ok(()),
        Ok((Ok(status), text)) => {
            Err(failure_reason(&text).unwrap_or_else(|| format!("FFmpeg : {status}")))
        }
    }
}

/// The first card whose test conversion succeeds: NVIDIA (NVENC), then
/// Intel / AMD (VAAPI).
pub(crate) async fn detect(
    ffmpeg: &str,
    gpus: &[GpuDto],
    hints: NvidiaHints,
) -> Result<HwEncoder, NoEncoder> {
    let candidates = candidates(gpus, hints);
    if candidates.is_empty() {
        return Err(NoEncoder::NoCard);
    }
    let mut failures = Vec::new();
    for candidate in candidates {
        match candidate {
            Candidate::Nvenc { card } => {
                match test(ffmpeg, test_args(HwApi::Nvenc, None, false), None).await {
                    Ok(()) => {
                        return Ok(HwEncoder {
                            api: HwApi::Nvenc,
                            device: None,
                            driver: None,
                            constant_qp: false,
                            card,
                        });
                    }
                    Err(e) => {
                        tracing::debug!(error = %e, "NVENC test conversion failed");
                        failures.push(format!("{card} (NVENC) : {e}"));
                    }
                }
            }
            Candidate::Vaapi {
                card,
                device,
                vendor,
            } => {
                for driver in vaapi_drivers(vendor) {
                    let name = driver.unwrap_or("défaut");
                    // With a capped bit rate first; some drivers offer
                    // constant QP only.
                    let mut errors = Vec::new();
                    for constant_qp in [false, true] {
                        let args = test_args(HwApi::Vaapi, Some(&device), constant_qp);
                        match test(ffmpeg, args, *driver).await {
                            Ok(()) => {
                                return Ok(HwEncoder {
                                    api: HwApi::Vaapi,
                                    device: Some(device),
                                    driver: driver.map(str::to_owned),
                                    constant_qp,
                                    card,
                                });
                            }
                            Err(e) => {
                                tracing::debug!(device, driver = name, constant_qp, error = %e, "VAAPI test conversion failed");
                                errors.push(e);
                            }
                        }
                    }
                    let detail = match errors.as_slice() {
                        [rate, qp] if rate != qp => format!("{rate} ; à QP constant : {qp}"),
                        [first, ..] => first.clone(),
                        [] => String::new(),
                    };
                    failures.push(format!("{card} (VAAPI, pilote {name}) : {detail}"));
                }
            }
        }
    }
    Err(NoEncoder::Failed(failures.join(" ; ")))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn vaapi() -> HwEncoder {
        HwEncoder {
            api: HwApi::Vaapi,
            device: Some("/dev/dri/renderD128".into()),
            driver: Some("iHD".into()),
            constant_qp: false,
            card: "Intel".into(),
        }
    }

    fn nvenc() -> HwEncoder {
        HwEncoder {
            api: HwApi::Nvenc,
            device: None,
            driver: None,
            constant_qp: false,
            card: "NVIDIA".into(),
        }
    }

    fn card(vendor: &'static str, status: &'static str, device: &str) -> GpuDto {
        GpuDto {
            id: "card0".into(),
            name: format!("{vendor} card"),
            vendor,
            driver: None,
            device: Some(device.into()),
            status,
            busy_percent: None,
        }
    }

    #[test]
    fn builds_processor_and_card_arguments() {
        assert!(input_args(None).is_empty());
        assert_eq!(
            input_args(Some(&vaapi())).join(" "),
            "-init_hw_device vaapi=va:/dev/dri/renderD128 -hwaccel vaapi -hwaccel_device va -filter_hw_device va"
        );
        assert_eq!(input_args(Some(&nvenc())).join(" "), "-hwaccel cuda");
        assert_eq!(filter_chain(None, ""), None);
        assert_eq!(
            filter_chain(None, "scale=640:-2").as_deref(),
            Some("scale=640:-2")
        );
        assert_eq!(
            filter_chain(Some(&vaapi()), "").as_deref(),
            Some("format=nv12,hwupload")
        );
        assert_eq!(
            filter_chain(Some(&vaapi()), "scale=640:-2").as_deref(),
            Some("scale=640:-2,format=nv12,hwupload")
        );
        assert_eq!(
            filter_chain(Some(&nvenc()), "scale=640:-2").as_deref(),
            Some("scale=640:-2,format=yuv420p")
        );
        assert_eq!(
            encoder_args(None, 23, "veryfast").join(" "),
            "-c:v libx264 -preset veryfast -crf 23 -pix_fmt yuv420p"
        );
        assert_eq!(
            encoder_args(Some(&vaapi()), 23, "veryfast").join(" "),
            "-c:v h264_vaapi -bf 0 -qp 27"
        );
        assert_eq!(
            encoder_args(Some(&nvenc()), 23, "veryfast").join(" "),
            "-c:v h264_nvenc -preset p4 -bf 0 -rc vbr -cq 29 -b:v 0"
        );
        let live = live_encoder_args(Some(&nvenc()), "2000k", "4000k").join(" ");
        assert!(
            live.contains("-c:v h264_nvenc -preset p1 -tune ll -bf 0 -forced-idr 1"),
            "{live}"
        );
        // The test conversion uses the live settings.
        let test = test_args(HwApi::Nvenc, None, false).join(" ");
        assert!(test.contains(&live), "{test}");
    }

    #[test]
    fn a_driver_offering_constant_qp_only_gets_no_bit_rate() {
        let rate = live_encoder_args(Some(&vaapi()), "2000k", "4000k").join(" ");
        assert!(
            rate.contains("-b:v 2000k -maxrate 2000k -bufsize 4000k"),
            "{rate}"
        );
        let cqp = HwEncoder {
            constant_qp: true,
            ..vaapi()
        };
        let live = live_encoder_args(Some(&cqp), "2000k", "4000k").join(" ");
        assert_eq!(
            live,
            "-vf format=nv12,hwupload -c:v h264_vaapi -bf 0 -rc_mode CQP -qp 30"
        );
        let test = test_args(HwApi::Vaapi, Some("/dev/dri/renderD128"), true).join(" ");
        assert!(test.contains(&live), "{test}");
        assert!(!test.contains("maxrate"), "{test}");
        // Recordings converted for the browser are at constant QP already.
        assert!(
            !encoder_args(Some(&cqp), 23, "veryfast")
                .join(" ")
                .contains("-b:v")
        );
    }

    #[test]
    fn the_reason_of_a_failed_test_is_the_line_that_says_why() {
        // Intel UHD 630, Debian's iHD driver (GitHub issue #2).
        let stderr = "[h264_vaapi @ 0x55] Driver does not support any RC mode compatible with selected options (supported modes: CQP).\n\
            [vost#0:0/h264_vaapi @ 0x56] Error while opening encoder - maybe incorrect parameters such as bit_rate, rate, width or height.\n\
            Nothing was written into output file, because at least one of its streams received no packets.\n";
        let reason = failure_reason(stderr).expect("reason");
        assert!(
            reason.starts_with("[h264_vaapi @ 0x55] Driver does not support any RC mode"),
            "{reason}"
        );
        assert!(reason.contains("Nothing was written"), "{reason}");
        assert_eq!(
            failure_reason("Error parsing global options: Input/output error\n").as_deref(),
            Some("Error parsing global options: Input/output error")
        );
        assert_eq!(failure_reason("\n  \n"), None);
    }

    #[test]
    fn tries_nvidia_then_intel_and_amd() {
        let none = NvidiaHints::default();
        assert!(candidates(&[], none).is_empty());
        assert!(
            candidates(
                &[card("other", "display_only", "/dev/dri/renderD128")],
                none
            )
            .is_empty()
        );
        assert!(candidates(&[card("intel", "not_passed", "/dev/dri/renderD128")], none).is_empty());
        // WSL: no card in /sys, but the Windows driver is there.
        let wsl = NvidiaHints { dxg: true, ..none };
        assert_eq!(
            candidates(&[], wsl),
            [Candidate::Nvenc {
                card: "NVIDIA".into()
            }]
        );
        let both = candidates(
            &[
                card("intel", "usable", "/dev/dri/renderD128"),
                card("nvidia", "usable", "/dev/nvidia0"),
            ],
            none,
        );
        assert_eq!(
            both,
            [
                Candidate::Nvenc {
                    card: "nvidia card".into()
                },
                Candidate::Vaapi {
                    card: "intel card".into(),
                    device: "/dev/dri/renderD128".into(),
                    vendor: "intel"
                }
            ]
        );
    }

    #[tokio::test]
    async fn without_a_usable_card_conversions_stay_on_the_processor() {
        let none = NvidiaHints::default();
        assert_eq!(
            detect("/nonexistent/ffmpeg", &[], none).await,
            Err(NoEncoder::NoCard)
        );
        // A card whose test cannot run: failed, with the reason.
        let Err(NoEncoder::Failed(reason)) = detect(
            "/nonexistent/ffmpeg",
            &[card("intel", "usable", "/dev/dri/renderD128")],
            none,
        )
        .await
        else {
            panic!("failure expected");
        };
        assert!(reason.contains("FFmpeg ne démarre pas"), "{reason}");
        assert!(reason.contains("pilote i965"), "{reason}");
    }
}
