#!/usr/bin/env bash
# Regenerates the small Annex B elementary streams used by the test suite
# (crates/rustnvr-testkit/fixtures). Requires ffmpeg with libx264 and libx265.
#
# The streams are deliberately tiny (320x240, 10 fps, 4 s, one keyframe per
# second, no B-frames) and contain access unit delimiters so the mock RTSP
# server can split them into access units without parsing slice headers.
set -euo pipefail

out="$(dirname "$0")/../crates/rustnvr-testkit/fixtures"
mkdir -p "$out"

ffmpeg -hide_banner -loglevel error -y \
  -f lavfi -i testsrc2=size=320x240:rate=10 -t 4 \
  -c:v libx264 -profile:v baseline -pix_fmt yuv420p -g 10 -bf 0 -b:v 150k \
  -x264-params aud=1:repeat-headers=1 \
  -f h264 "$out/h264_320x240_10fps.h264"

ffmpeg -hide_banner -loglevel error -y \
  -f lavfi -i testsrc2=size=320x240:rate=10 -t 4 \
  -c:v libx265 -pix_fmt yuv420p -b:v 150k \
  -x265-params log-level=error:keyint=10:min-keyint=10:bframes=0:aud=1:repeat-headers=1:scenecut=0 \
  -f hevc "$out/h265_320x240_10fps.h265"

ls -l "$out"
