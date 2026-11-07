//! USB webcams (V4L2): the FFmpeg command reading the device and writing
//! H.264 for the device supervisor of `rustnvr-stream`. The webcam's own
//! H.264 is copied; otherwise its pictures are encoded on the graphics card
//! that passed the test conversion (`live.hwaccel`), else by libx264 with
//! settings light on the processor.

use rustnvr_api::ApiState;
use rustnvr_api::hwaccel::{HwApi, HwEncoder};
use rustnvr_core::config::UsbConfig;
use rustnvr_stream::{CommandSource, DeviceCommand, annexb_output_args};
use std::sync::{Arc, OnceLock};
use std::time::Duration;

/// A key frame every this many seconds (segments are cut on them).
const KEY_FRAME_SECONDS: u32 = 2;
/// Picture rate assumed when the webcam is not asked for one.
const DEFAULT_FRAMERATE: u32 = 15;

fn owned(args: &[&str]) -> Vec<String> {
    args.iter().map(|s| (*s).to_owned()).collect()
}

/// The FFmpeg command for `usb`, encoding on `hw` (`None`: the processor).
pub(super) fn command(usb: &UsbConfig, hw: Option<&HwEncoder>) -> DeviceCommand {
    // The card is not needed to copy the webcam's own H.264.
    let hw = hw.filter(|_| !usb.copies_h264());
    let mut args = owned(&["-hide_banner", "-nostdin", "-loglevel", "error"]);
    if let Some(HwEncoder {
        api: HwApi::Vaapi,
        device: Some(device),
        ..
    }) = hw
    {
        args.extend([
            "-init_hw_device".to_owned(),
            format!("vaapi=va:{device}"),
            "-filter_hw_device".to_owned(),
            "va".to_owned(),
        ]);
    }
    args.extend(owned(&["-f", "v4l2", "-thread_queue_size", "64"]));
    if let Some(f) = &usb.input_format {
        args.extend(["-input_format".to_owned(), f.clone()]);
    }
    if let (Some(w), Some(h)) = (usb.width, usb.height) {
        args.extend(["-video_size".to_owned(), format!("{w}x{h}")]);
    }
    if let Some(f) = usb.framerate {
        args.extend(["-framerate".to_owned(), f.to_string()]);
    }
    args.extend(["-i".to_owned(), usb.device.clone(), "-an".to_owned()]);
    let gop = (usb.framerate.unwrap_or(DEFAULT_FRAMERATE) * KEY_FRAME_SECONDS).to_string();
    let rate = format!("{}k", usb.bitrate_kbps);
    let buf = format!("{}k", usb.bitrate_kbps.saturating_mul(2));
    if usb.copies_h264() {
        args.extend(owned(&["-c:v", "copy"]));
    } else {
        match hw {
            Some(HwEncoder {
                api: HwApi::Vaapi,
                constant_qp,
                ..
            }) => {
                args.extend(owned(&[
                    "-vf",
                    "format=nv12,hwupload",
                    "-c:v",
                    "h264_vaapi",
                    "-bf",
                    "0",
                    "-g",
                    &gop,
                ]));
                if *constant_qp {
                    args.extend(owned(&["-rc_mode", "CQP", "-qp", "26"]));
                } else {
                    args.extend(owned(&["-b:v", &rate, "-maxrate", &rate, "-bufsize", &buf]));
                }
            }
            Some(HwEncoder {
                api: HwApi::Nvenc, ..
            }) => args.extend(owned(&[
                "-vf",
                "format=yuv420p",
                "-c:v",
                "h264_nvenc",
                "-preset",
                "p2",
                "-tune",
                "ll",
                "-bf",
                "0",
                "-g",
                &gop,
                "-forced-idr",
                "1",
                "-rc",
                "vbr",
                "-b:v",
                &rate,
                "-maxrate",
                &rate,
                "-bufsize",
                &buf,
            ])),
            None => args.extend(owned(&[
                "-c:v",
                "libx264",
                "-preset",
                "veryfast",
                "-tune",
                "zerolatency",
                "-pix_fmt",
                "yuv420p",
                "-bf",
                "0",
                "-g",
                &gop,
                "-crf",
                "23",
                "-maxrate",
                &rate,
                "-bufsize",
                &buf,
            ])),
        }
    }
    args.extend(annexb_output_args());
    let env = hw
        .and_then(|h| h.driver.clone())
        .map(|d| vec![("LIBVA_DRIVER_NAME".to_owned(), d)])
        .unwrap_or_default();
    DeviceCommand { args, env }
}

/// The command of each attempt: the graphics card is known once the
/// interface is up and its test conversion over.
pub(super) fn command_source(api: Arc<OnceLock<Arc<ApiState>>>, usb: UsbConfig) -> CommandSource {
    Arc::new(move || {
        let api = api.clone();
        let usb = usb.clone();
        Box::pin(async move {
            let mut hw = None;
            if !usb.copies_h264() {
                for _ in 0..60 {
                    if let Some(api) = api.get() {
                        // The test conversion is spawned with the interface:
                        // let it start before asking for its result.
                        tokio::time::sleep(Duration::from_secs(1)).await;
                        hw = api.hw_encoder().await;
                        break;
                    }
                    tokio::time::sleep(Duration::from_millis(500)).await;
                }
            }
            command(&usb, hw.as_ref())
        })
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn usb() -> UsbConfig {
        UsbConfig {
            device: "/dev/video0".into(),
            input_format: Some("mjpeg".into()),
            width: Some(1280),
            height: Some(720),
            framerate: Some(15),
            bitrate_kbps: 1500,
        }
    }

    #[test]
    fn webcam_commands_for_each_encoder() {
        let cpu = command(&usb(), None).args.join(" ");
        assert!(
            cpu.starts_with("-hide_banner -nostdin -loglevel error -f v4l2 -thread_queue_size 64 -input_format mjpeg -video_size 1280x720 -framerate 15 -i /dev/video0 -an "),
            "{cpu}"
        );
        assert!(
            cpu.contains("-c:v libx264 -preset veryfast -tune zerolatency -pix_fmt yuv420p -bf 0 -g 30 -crf 23 -maxrate 1500k -bufsize 3000k"),
            "{cpu}"
        );
        assert!(cpu.ends_with("-f h264 pipe:1"), "{cpu}");

        let vaapi = HwEncoder {
            api: HwApi::Vaapi,
            device: Some("/dev/dri/renderD128".into()),
            driver: Some("iHD".into()),
            constant_qp: true,
            card: "Intel".into(),
        };
        let c = command(&usb(), Some(&vaapi));
        let text = c.args.join(" ");
        assert!(
            text.starts_with("-hide_banner -nostdin -loglevel error -init_hw_device vaapi=va:/dev/dri/renderD128 -filter_hw_device va -f v4l2"),
            "{text}"
        );
        assert!(
            text.contains(
                "-vf format=nv12,hwupload -c:v h264_vaapi -bf 0 -g 30 -rc_mode CQP -qp 26"
            ),
            "{text}"
        );
        assert_eq!(c.env, [("LIBVA_DRIVER_NAME".to_owned(), "iHD".to_owned())]);

        // The webcam's H.264 is copied, the card left alone.
        let h264 = UsbConfig {
            input_format: Some("h264".into()),
            width: None,
            height: None,
            framerate: None,
            ..usb()
        };
        let c = command(&h264, Some(&vaapi));
        let text = c.args.join(" ");
        assert_eq!(
            text,
            "-hide_banner -nostdin -loglevel error -f v4l2 -thread_queue_size 64 -input_format h264 -i /dev/video0 -an -c:v copy -bsf:v dump_extra=freq=keyframe,h264_metadata=aud=insert -flush_packets 1 -f h264 pipe:1"
        );
        assert!(c.env.is_empty());
    }
}
