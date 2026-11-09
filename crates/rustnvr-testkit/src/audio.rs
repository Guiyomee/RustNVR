//! Test sound: a tone and its G.711 µ-law bytes.

/// Sample `n` of a 440 Hz tone at 8 kHz, a quarter of full scale.
pub fn tone(n: u64) -> i16 {
    (8_000.0 * (2.0 * std::f64::consts::PI * 440.0 * n as f64 / 8_000.0).sin()) as i16
}

/// One G.711 µ-law byte (ITU-T G.711).
pub fn mulaw(sample: i16) -> u8 {
    let mut s = i32::from(sample);
    let sign = if s < 0 {
        s = -s;
        0x80
    } else {
        0
    };
    s = s.min(32_635) + 0x84;
    let mut exponent = 7;
    while exponent > 0 && s & (0x80 << exponent) == 0 {
        exponent -= 1;
    }
    let mantissa = (s >> (exponent + 3)) & 0x0f;
    !(sign | (exponent << 4) as u8 | mantissa as u8)
}
