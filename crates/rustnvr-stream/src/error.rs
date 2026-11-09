//! Errors of the ingestion layer.

use std::time::Duration;

/// Why an RTSP session could not be established or was lost.
///
/// Messages never include the stream URL or credentials. RTSP errors from
/// Retina include connection addresses and protocol details, which are useful
/// for diagnosis and do not contain secrets because credentials are never part
/// of the URL handed to Retina.
#[derive(Debug, thiserror::Error)]
pub enum StreamError {
    /// A step did not complete in time.
    #[error("{stage} timed out after {after:?}")]
    Timeout {
        /// RTSP step (DESCRIBE, SETUP, PLAY).
        stage: &'static str,
        /// Configured timeout.
        after: Duration,
    },
    /// The RTSP client reported an error.
    ///
    /// The Retina error is part of the message rather than exposed as
    /// `source()`, so that error reports do not print it twice.
    #[error("{stage} failed: {error}")]
    Rtsp {
        /// RTSP step or `stream` once playing.
        stage: &'static str,
        /// Error from Retina.
        error: retina::Error,
    },
    /// The server's presentation has no H.264/H.265 video stream.
    #[error("no H.264/H.265 video stream (available: {available})")]
    NoSupportedVideo {
        /// `media/encoding` of each stream offered.
        available: String,
    },
    /// The video stream's codec cannot be recorded.
    #[error("unsupported video codec in stream parameters")]
    UnsupportedCodec,
    /// Parameters were still unknown after a frame was received.
    #[error("video parameters unavailable after first frame")]
    MissingParameters,
    /// The MP4 sample entry could not be built from the parameters.
    #[error("cannot build MP4 sample entry: {0}")]
    SampleEntry(retina::Error),
    /// The server ended the stream.
    #[error("stream ended by server")]
    Ended,
    /// No frame was received for the stall timeout.
    #[error("no video frame for {0:?}")]
    Stalled(Duration),
    /// The RTSP client failed on the camera's packets (a Retina bug or an
    /// unexpected packet sequence); the session is started again.
    #[error("RTSP client failure on the camera's packets: {0}")]
    ClientFailure(String),
}

impl StreamError {
    /// First line of the message, for state summaries (Retina errors are multi-line).
    pub fn summary(&self) -> String {
        let msg = self.to_string();
        msg.lines().next().unwrap_or_default().trim().to_owned()
    }

    /// The whole message on one line, for logs: Retina adds the connection
    /// and the message position on lines of their own, which journald shows
    /// as separate entries without date nor camera.
    pub fn one_line(&self) -> String {
        self.to_string()
            .lines()
            .map(str::trim)
            .filter(|l| !l.is_empty())
            .collect::<Vec<_>>()
            .join(" · ")
    }

    /// HTTP-like status code of an RTSP error response (e.g. 401), if any.
    pub fn status_code(&self) -> Option<u16> {
        match self {
            StreamError::Rtsp { error, .. } => error.status_code(),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn logs_get_one_line() {
        let e = StreamError::ClientFailure(
            "Error reading from RTSP peer

conn: 192.168.1.211:40540(me)->192.168.1.29:554
msg: 0"
                .into(),
        );
        assert_eq!(
            e.one_line(),
            "RTSP client failure on the camera's packets: Error reading from RTSP peer · conn: 192.168.1.211:40540(me)->192.168.1.29:554 · msg: 0"
        );
        assert_eq!(
            e.summary(),
            "RTSP client failure on the camera's packets: Error reading from RTSP peer"
        );
    }
}
