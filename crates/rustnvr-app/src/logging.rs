//! `tracing` initialization: the console log, and a copy of the latest
//! messages in memory for the web interface ([`buffer`]).

use rustnvr_api::logs::{LogBuffer, LogEntry, LogLevel};
use std::fmt::Write as _;
use std::sync::{Arc, OnceLock};
use std::time::{SystemTime, UNIX_EPOCH};
use tracing::field::{Field, Visit};
use tracing::span::{Attributes, Id};
use tracing::{Event, Level, Subscriber};
use tracing_subscriber::EnvFilter;
use tracing_subscriber::layer::{Context, SubscriberExt as _};
use tracing_subscriber::registry::LookupSpan;
use tracing_subscriber::util::SubscriberInitExt as _;
use tracing_subscriber::{Layer, Registry};

/// Messages kept for the interface.
const KEPT_MESSAGES: usize = 2000;

static BUFFER: OnceLock<Arc<LogBuffer>> = OnceLock::new();

/// The latest log messages, once [`init`] ran.
pub fn buffer() -> Option<Arc<LogBuffer>> {
    BUFFER.get().cloned()
}

/// Log output format.
#[derive(Clone, Copy, Debug, PartialEq, Eq, clap::ValueEnum)]
pub enum LogFormat {
    /// Human-readable lines.
    Text,
    /// One JSON object per line, for log collectors.
    Json,
}

/// Installs the global subscriber. The filter comes from `RUST_LOG`
/// (default `info`) and applies to the console and to the copy kept in
/// memory. Records of the `log` crate (used by the RTSP library) are
/// forwarded to `tracing`.
pub fn init(format: LogFormat) {
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));
    let console = tracing_subscriber::fmt::layer()
        .with_ansi(std::io::IsTerminal::is_terminal(&std::io::stderr()))
        .with_writer(std::io::stderr);
    let console: Box<dyn Layer<Registry> + Send + Sync> = match format {
        LogFormat::Text => Box::new(console),
        LogFormat::Json => Box::new(console.json()),
    };
    let buffer = BUFFER
        .get_or_init(|| Arc::new(LogBuffer::new(KEPT_MESSAGES)))
        .clone();
    // `try_init` also installs the `log` → `tracing` bridge.
    let result = tracing_subscriber::registry()
        .with(console)
        .with(MemoryLayer(buffer))
        .with(filter)
        .try_init();
    if let Err(e) = result {
        eprintln!("logging already initialized: {e}");
    }
}

/// Camera of a span (`stream{camera=…}`), kept in its extensions.
struct SpanCamera(String);

/// Copies each message into the [`LogBuffer`].
struct MemoryLayer(Arc<LogBuffer>);

impl<S> Layer<S> for MemoryLayer
where
    S: Subscriber + for<'a> LookupSpan<'a>,
{
    fn on_new_span(&self, attrs: &Attributes<'_>, id: &Id, ctx: Context<'_, S>) {
        let mut fields = Fields::default();
        attrs.record(&mut fields);
        if let (Some(camera), Some(span)) = (fields.camera, ctx.span(id)) {
            span.extensions_mut().insert(SpanCamera(camera));
        }
    }

    fn on_event(&self, event: &Event<'_>, ctx: Context<'_, S>) {
        let mut fields = Fields::default();
        event.record(&mut fields);
        let camera = fields.camera.take().or_else(|| {
            ctx.event_scope(event)?
                .from_root()
                .find_map(|span| span.extensions().get::<SpanCamera>().map(|c| c.0.clone()))
        });
        let level = match *event.metadata().level() {
            Level::ERROR => LogLevel::Error,
            Level::WARN => LogLevel::Warn,
            Level::INFO => LogLevel::Info,
            Level::DEBUG => LogLevel::Debug,
            Level::TRACE => LogLevel::Trace,
        };
        let time_ms = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |d| i64::try_from(d.as_millis()).unwrap_or(i64::MAX));
        let mut message = fields.message;
        if !fields.rest.is_empty() {
            if !message.is_empty() {
                message.push(' ');
            }
            message.push_str(&fields.rest);
        }
        self.0.push(LogEntry {
            seq: 0,
            time_ms,
            level,
            target: fields
                .log_target
                .take()
                .unwrap_or_else(|| event.metadata().target().to_owned()),
            camera,
            message,
        });
    }
}

/// Message, `camera` field and the other fields as `key=value`.
#[derive(Default)]
struct Fields {
    message: String,
    camera: Option<String>,
    /// Real origin of records of the `log` crate (`log.target`).
    log_target: Option<String>,
    rest: String,
}

impl Visit for Fields {
    fn record_str(&mut self, field: &Field, value: &str) {
        match field.name() {
            "message" => self.message = value.to_owned(),
            "camera" => self.camera = Some(value.to_owned()),
            "log.target" => self.log_target = Some(value.to_owned()),
            // Location fields of `log` records: noise for the reader.
            "log.module_path" | "log.file" => {}
            name => self.push(name, value),
        }
    }

    fn record_debug(&mut self, field: &Field, value: &dyn std::fmt::Debug) {
        let text = format!("{value:?}");
        match field.name() {
            "message" => self.message = text,
            // Displayed identifiers come through `%camera` as Debug of Display.
            "camera" => self.camera = Some(text.trim_matches('"').to_owned()),
            "log.target" => self.log_target = Some(text.trim_matches('"').to_owned()),
            "log.module_path" | "log.file" | "log.line" => {}
            name => self.push(name, &text),
        }
    }
}

impl Fields {
    fn push(&mut self, name: &str, value: &str) {
        if !self.rest.is_empty() {
            self.rest.push(' ');
        }
        let _ = write!(self.rest, "{name}={value}");
    }
}
