# retina 0.4.21, patched for RustNVR

A copy of [retina](https://crates.io/crates/retina) 0.4.21 (Scott Lamb, MIT
OR Apache-2.0), from crates.io, used by the root `Cargo.toml` through
`[patch.crates-io]`. Everything is unchanged except the parts marked
"RustNVR patch":

- `src/codec/h26x.rs`: `00 00 00 xx` (`xx` not `00` or `01`) and `00 00 02`
  inside a NAL are kept as body data instead of failing with "invalid
  sequence". The TP-Link Tapo C200 v2 (firmware 1.3.17) sends such
  unescaped zeros; FFmpeg, go2rtc and Frigate read the stream. Before the
  patch the session ended every few seconds.
- `src/codec/h264.rs`: the test `parse_unescaped_zeros` and the doc comment
  of `Depacketizer`.
- `src/testutil.rs`: `init_logging` does nothing (the `mylog` git
  dev-dependency is not in the crates.io package, so the tests did not
  build).

retina 0.4.22 (2026-11-12) still fails on these sequences. To drop this
copy: once a retina release keeps them, remove the `[patch.crates-io]`
entry and `exclude` in the root `Cargo.toml`, delete this directory and
update `retina` in `Cargo.lock`.

Tests: `cargo test --manifest-path crates/retina/Cargo.toml --lib` (then
delete the `crates/retina/Cargo.lock` it writes).
