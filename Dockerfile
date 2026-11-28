# syntax=docker/dockerfile:1
#
# RustNVR image: the web interface (Node), the program (Rust, with the
# built-in ONNX recognition), then a small Debian with FFmpeg.
#
#   docker compose up -d --build        (see https://github.com/Guiyomee/RustNVR/wiki/EN-Docker)

# --- Web interface -----------------------------------------------------------
# Built once, on the machine that builds (the files are the same everywhere).
FROM --platform=$BUILDPLATFORM node:24-slim AS web
WORKDIR /src/web
COPY web/package.json web/package-lock.json ./
RUN npm ci --no-audit --no-fund
COPY web/ ./
RUN npm run build

# --- Program -----------------------------------------------------------------
# Same Debian release as the final image (same glibc). Compiled on the
# machine that builds, for the target architecture (amd64 or arm64): no
# emulated compiler, which would take hours for arm64.
FROM --platform=$BUILDPLATFORM rust:1-trixie AS build
ARG TARGETARCH
# aws-lc-sys (TLS) and libopus build C code with CMake; for arm64, the cross
# compilers of Debian.
RUN apt-get update \
    && apt-get install -y --no-install-recommends cmake clang \
    && if [ "$TARGETARCH" = arm64 ] && [ "$(dpkg --print-architecture)" != arm64 ]; then \
         apt-get install -y --no-install-recommends gcc-aarch64-linux-gnu g++-aarch64-linux-gnu libc6-dev-arm64-cross \
         && rustup target add aarch64-unknown-linux-gnu; \
       fi \
    && rm -rf /var/lib/apt/lists/*
WORKDIR /src
COPY Cargo.toml Cargo.lock ./
COPY crates ./crates
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/src/target,id=rustnvr-target-$TARGETARCH \
    set -eu; \
    if [ "$TARGETARCH" = arm64 ] && [ "$(dpkg --print-architecture)" != arm64 ]; then \
        export CARGO_TARGET_AARCH64_UNKNOWN_LINUX_GNU_LINKER=aarch64-linux-gnu-gcc \
               CC_aarch64_unknown_linux_gnu=aarch64-linux-gnu-gcc \
               CXX_aarch64_unknown_linux_gnu=aarch64-linux-gnu-g++ \
               AR_aarch64_unknown_linux_gnu=aarch64-linux-gnu-ar; \
        cargo build --release --locked --bin rustnvr --target aarch64-unknown-linux-gnu; \
        cp target/aarch64-unknown-linux-gnu/release/rustnvr /usr/local/bin/rustnvr; \
    else \
        cargo build --release --locked --bin rustnvr; \
        cp target/release/rustnvr /usr/local/bin/rustnvr; \
    fi

# --- Final image -------------------------------------------------------------
FROM debian:trixie-slim
# FFmpeg: snapshots, motion detection, H.264 conversions, exports.
# pci.ids: names of the graphics cards shown in the interface.
# VAAPI drivers, for the H.264 conversions on the graphics card (live.hwaccel):
# Intel iHD (Broadwell and newer; its free build cannot encode H.264 before
# Ice Lake), Intel i965 (up to Coffee Lake, H.264 encoding included), AMD.
# The Intel drivers exist for amd64 only.
ARG TARGETARCH
RUN apt-get update \
    && apt-get install -y --no-install-recommends ffmpeg ca-certificates tzdata pci.ids mesa-va-drivers \
    && if [ "$TARGETARCH" = amd64 ]; then \
         apt-get install -y --no-install-recommends intel-media-va-driver i965-va-driver; \
       fi \
    && rm -rf /var/lib/apt/lists/*
# Unprivileged account (uid 1000, as the first user of most Linux hosts).
RUN groupadd --gid 1000 rustnvr \
    && useradd --uid 1000 --gid 1000 --no-create-home --home-dir /data --shell /usr/sbin/nologin rustnvr \
    && mkdir -p /config /data \
    && chown rustnvr:rustnvr /config /data
COPY --from=build /usr/local/bin/rustnvr /usr/local/bin/rustnvr
COPY --from=web /src/web/dist /usr/share/rustnvr/web
COPY docker/config.toml /usr/share/rustnvr/config.toml
# Licences: RustNVR, and libopus linked into the program.
COPY LICENSE LICENSE-libopus licenses/LICENSE-MIT licenses/LICENSE-APACHE /usr/share/doc/rustnvr/
COPY --chmod=0755 docker/entrypoint.sh /usr/local/bin/rustnvr-entrypoint

USER rustnvr
WORKDIR /data
# /config: config.toml and the settings made in the interface (*.ui.toml,
# camera passwords included). /data: recordings, database, thumbnails.
VOLUME ["/config", "/data"]
# 8554: RTSP republication of the cameras, when enabled ([restream]).
EXPOSE 8080 8554
# Health (« healthy » in Portainer / docker ps): the program asks the running
# server for GET /api/v1/health on its own port (there is no curl here).
HEALTHCHECK --interval=30s --timeout=10s --start-period=60s --retries=3 \
    CMD ["rustnvr", "health", "-c", "/config/config.toml"]
ENTRYPOINT ["rustnvr-entrypoint"]
CMD ["run", "-c", "/config/config.toml"]
