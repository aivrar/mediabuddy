use std::collections::VecDeque;
use std::sync::{Arc, RwLock};

use serde::Serialize;
use tracing::{Event, Subscriber};
use tracing_subscriber::layer::{Context, Layer};

const BUFFER_LIMIT: usize = 1000;

#[derive(Debug, Clone, Serialize)]
pub struct LogEntry {
    pub timestamp: u64,
    pub level: String,
    pub target: String,
    pub message: String,
}

#[derive(Clone)]
pub struct LogBuffer {
    inner: Arc<RwLock<VecDeque<LogEntry>>>,
}

impl LogBuffer {
    pub fn new() -> Self {
        Self {
            inner: Arc::new(RwLock::new(VecDeque::with_capacity(BUFFER_LIMIT))),
        }
    }

    pub fn snapshot(&self, since: Option<u64>, level_min: &str) -> Vec<LogEntry> {
        let min_ord = level_ord(level_min);
        let buf = self.inner.read().unwrap();
        buf.iter()
            .filter(|e| level_ord(&e.level) >= min_ord)
            .filter(|e| since.map_or(true, |s| e.timestamp >= s))
            .cloned()
            .collect()
    }

    pub fn clear(&self) {
        self.inner.write().unwrap().clear();
    }

    fn push(&self, entry: LogEntry) {
        let mut buf = self.inner.write().unwrap();
        while buf.len() >= BUFFER_LIMIT {
            buf.pop_front();
        }
        buf.push_back(entry);
    }
}

fn level_ord(level: &str) -> u8 {
    match level.to_ascii_uppercase().as_str() {
        "TRACE" => 0,
        "DEBUG" => 1,
        "INFO" => 2,
        "WARN" => 3,
        "ERROR" => 4,
        _ => 2,
    }
}

#[derive(Default)]
struct MessageVisitor {
    message: String,
    fields: Vec<(String, String)>,
}

impl tracing::field::Visit for MessageVisitor {
    fn record_str(&mut self, field: &tracing::field::Field, value: &str) {
        if field.name() == "message" {
            self.message = value.to_string();
        } else {
            self.fields.push((field.name().to_string(), value.to_string()));
        }
    }
    fn record_debug(&mut self, field: &tracing::field::Field, value: &dyn std::fmt::Debug) {
        let formatted = format!("{value:?}");
        if field.name() == "message" {
            self.message = formatted;
        } else {
            self.fields.push((field.name().to_string(), formatted));
        }
    }
}

pub struct BufferLayer {
    buffer: LogBuffer,
}

impl BufferLayer {
    pub fn new(buffer: LogBuffer) -> Self {
        Self { buffer }
    }
}

impl<S: Subscriber> Layer<S> for BufferLayer {
    fn on_event(&self, event: &Event<'_>, _ctx: Context<'_, S>) {
        let metadata = event.metadata();
        let mut visitor = MessageVisitor::default();
        event.record(&mut visitor);
        let mut message = visitor.message;
        if !visitor.fields.is_empty() {
            let extras = visitor
                .fields
                .iter()
                .map(|(k, v)| format!("{k}={v}"))
                .collect::<Vec<_>>()
                .join(" ");
            if !message.is_empty() {
                message.push(' ');
            }
            message.push_str(&extras);
        }
        self.buffer.push(LogEntry {
            timestamp: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_secs())
                .unwrap_or(0),
            level: metadata.level().to_string(),
            target: metadata.target().to_string(),
            message,
        });
    }
}
