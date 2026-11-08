# syntax=docker/dockerfile:1
#
# RustNVR image: the web interface (Node), the program (Rust, with the
# built-in ONNX recognition), then a small Debian with FFmpeg.
#
#   docker compose up -d --build        (see docs/DOCKER.md)

# --- Web interface -----------------------------------------------------------
FROM node:24-slim AS web
WORKDIR /src/web
COPY web/package.json web/package-lock.json ./
RUN npm ci --no-audit --no-fund
COPY web/ ./
RUN npm run build

# --- Program -----------------------------------------------------------------
# Same Debian release as the final image (same glibc).
FROM rust:1-trixie AS build
# aws-lc-sys (TLS) builds C code with CMake.
RUN apt-get update \
    && apt-get install -y --no-install-recommends cmake clang \
    && rm -rf /var/lib/apt/lists/*
WORKDIR /src
COPY Cargo.toml Cargo.lock ./
COPY crates ./crates
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/src/target \
    cargo build --release --locked --bin rustnvr \
    && cp target/release/rustnvr /usr/local/bin/rustnvr

# --- Final image -------------------------------------------------------------
FROM debian:trixie-slim
# FFmpeg: snapshots, motion detection, H.264 conversions, exports.
# pci.ids: names of the graphics cards shown in the interface.
# VAAPI drivers, for the H.264 conversions on the graphics card (live.hwaccel):
# Intel iHD (Broadwell and newer; its free build cannot encode H.264 before
# Ice Lake), Intel i965 (up to Coffee Lake, H.264 encoding included), AMD.
RUN apt-get update \
    && apt-get install -y --no-install-recommends ffmpeg ca-certificates tzdata pci.ids \
       intel-media-va-driver i965-va-driver mesa-va-drivers \
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
COPY LICENSE-MIT LICENSE-APACHE LICENSE-libopus /usr/share/doc/rustnvr/
COPY --chmod=0755 docker/entrypoint.sh /usr/local/bin/rustnvr-entrypoint

USER rustnvr
WORKDIR /data
# /config: config.toml and the settings made in the interface (*.ui.toml,
# camera passwords included). /data: recordings, database, thumbnails.
VOLUME ["/config", "/data"]
EXPOSE 8080
# Health (« healthy » in Portainer / docker ps): the program asks the running
# server for GET /api/v1/health on its own port (there is no curl here).
HEALTHCHECK --interval=30s --timeout=10s --start-period=60s --retries=3 \
    CMD ["rustnvr", "health", "-c", "/config/config.toml"]
ENTRYPOINT ["rustnvr-entrypoint"]
CMD ["run", "-c", "/config/config.toml"]
