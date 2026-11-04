#!/usr/bin/env python3
"""Independent RTSP server for manual interoperability and load tests.

Serves a live test pattern encoded by GStreamer (gst-rtsp-server, x264, x265):

    rtsp://127.0.0.1:<port>/h264
    rtsp://127.0.0.1:<port>/h265

Each mount point is shared: every client receives the same encoded stream, so
several RustNVR "cameras" can point at the same URL without multiplying the
encoding cost on the test machine.

Requirements (Debian/Ubuntu):
    apt install python3-gi gir1.2-gst-rtsp-server-1.0 \
        gstreamer1.0-plugins-good gstreamer1.0-plugins-bad gstreamer1.0-plugins-ugly
"""

import argparse

import gi

gi.require_version("Gst", "1.0")
gi.require_version("GstRtspServer", "1.0")
from gi.repository import GLib, Gst, GstRtspServer  # noqa: E402


def alternate(_factory, media, period: int) -> None:
    """Switches the input selector between the still and the moving input."""
    selector = media.get_element().get_by_name("sel")
    pads = selector.sinkpads
    state = {"moving": False}

    def toggle() -> bool:
        state["moving"] = not state["moving"]
        selector.set_property("active-pad", pads[1 if state["moving"] else 0])
        print("motion" if state["moving"] else "still", flush=True)
        return True

    selector.set_property("active-pad", pads[0])
    GLib.timeout_add_seconds(period, toggle)


def main() -> None:
    p = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    p.add_argument("--port", type=int, default=8554)
    p.add_argument("--width", type=int, default=1920)
    p.add_argument("--height", type=int, default=1080)
    p.add_argument("--fps", type=int, default=25)
    p.add_argument("--bitrate", type=int, default=4000, help="kbit/s")
    p.add_argument("--gop", type=int, default=50, help="frames between key frames")
    p.add_argument(
        "--motion-period",
        type=int,
        default=0,
        help="alternate every N seconds between a still picture and a moving ball "
        "(same background), to test motion detection; 0: SMPTE bars with a clock",
    )
    args = p.parse_args()

    Gst.init(None)
    caps = f"video/x-raw,width={args.width},height={args.height},framerate={args.fps}/1"
    if args.motion_period > 0:
        # Both inputs share the grey background: switching changes only the ball.
        grey = "0xff404040"
        # The selector comes last so that the encoder is linked to its output.
        source = (
            f"videotestsrc is-live=true pattern=solid-color foreground-color={grey} ! {caps} ! sel. "
            # Drawn at a quarter of the size then scaled up: a ball as large as
            # a person a few metres away.
            f"videotestsrc is-live=true pattern=ball background-color={grey} ! "
            f"video/x-raw,width={args.width // 4},height={args.height // 4},framerate={args.fps}/1 ! "
            f"videoscale ! {caps} ! sel. "
            "input-selector name=sel"
        )
    else:
        source = f"videotestsrc is-live=true pattern=smpte ! {caps} ! timeoverlay"
    encoders = {
        "h264": (
            f"x264enc tune=zerolatency speed-preset=ultrafast bitrate={args.bitrate} "
            f"key-int-max={args.gop} bframes=0 ! video/x-h264,profile=main ! "
            "rtph264pay name=pay0 pt=96 config-interval=-1"
        ),
        "h265": (
            f"x265enc tune=zerolatency speed-preset=ultrafast bitrate={args.bitrate} "
            f"key-int-max={args.gop} ! video/x-h265,profile=main ! "
            "rtph265pay name=pay0 pt=96 config-interval=-1"
        ),
    }

    server = GstRtspServer.RTSPServer()
    server.set_service(str(args.port))
    mounts = server.get_mount_points()
    for name, encoder in encoders.items():
        factory = GstRtspServer.RTSPMediaFactory()
        factory.set_launch(f"( {source} ! {encoder} )")
        factory.set_shared(True)
        if args.motion_period > 0:
            factory.connect("media-configure", alternate, args.motion_period)
        mounts.add_factory(f"/{name}", factory)
    server.attach(None)
    print(
        f"serving rtsp://127.0.0.1:{args.port}/h264 and /h265 "
        f"({args.width}x{args.height} {args.fps} fps {args.bitrate} kbit/s, GOP {args.gop})",
        flush=True,
    )
    GLib.MainLoop().run()


if __name__ == "__main__":
    main()
