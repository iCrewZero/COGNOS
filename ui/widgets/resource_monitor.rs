//! Resource monitor widget — renders CPU/GPU/RAM/disk/network gauges fed
//! by eBPF telemetry. Updates at 1Hz, renders via the configured UI
//! toolkit (GTK4 or custom).
//!
//! The widget is pure-data: it ingests [`SystemMetrics`] snapshots pushed
//! by the scheduler telemetry daemon, retains a rolling history for
//! spark-lines, and emits a toolkit-agnostic [`WidgetTree`] each render.
//! The actual backend (GTK4 `GtkBox`/`GtkLevelBar`, or a custom GPU
//! compositor pass) is selected by the shell — this module never touches
//! a toolkit directly.
//!
//! v0: stub implementation

use std::collections::VecDeque;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use thiserror::Error;
use tracing::debug;
use uuid::Uuid;

// ─── Metrics model ───────────────────────────────────────────────────────────

/// Snapshot of all system resources at a single instant.
///
/// Feeds the gauges and the rolling history buffer used for spark-lines.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SystemMetrics {
    /// Per-core CPU utilisation in `[0.0, 1.0]`. Length equals core count.
    pub cpu_per_core: Vec<f32>,
    /// Aggregate GPU utilisation in `[0.0, 1.0]` (averaged across devices).
    pub gpu: f32,
    /// Resident RAM currently in use, in bytes.
    pub ram_used: u64,
    /// Total physical RAM, in bytes.
    pub ram_total: u64,
    /// Disk read/write throughput, in bytes/sec.
    pub disk_io: IoRate,
    /// Network send/receive throughput, in bytes/sec.
    pub net_io: IoRate,
    /// Hottest sensor temperature, in degrees Celsius.
    pub temp: f32,
}

impl Default for SystemMetrics {
    fn default() -> Self {
        Self {
            cpu_per_core: Vec::new(),
            gpu: 0.0,
            ram_used: 0,
            ram_total: 0,
            disk_io: IoRate::default(),
            net_io: IoRate::default(),
            temp: 0.0,
        }
    }
}

/// Directional I/O rate (read + write, or rx + tx).
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize)]
pub struct IoRate {
    /// Inbound rate (read / rx), bytes/sec.
    pub inbound: u64,
    /// Outbound rate (write / tx), bytes/sec.
    pub outbound: u64,
}

// ─── Ring buffer ─────────────────────────────────────────────────────────────

/// Fixed-capacity ring buffer used to retain metric history for spark-lines.
///
/// Newer samples push older ones out once `cap` is reached (FIFO).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RingBuffer<T> {
    /// Backing storage; the oldest element is at the front.
    pub buf: VecDeque<T>,
    /// Maximum number of samples to retain.
    pub cap: usize,
}

impl<T> RingBuffer<T> {
    /// Build a new ring buffer with the supplied capacity.
    pub fn new(cap: usize) -> Self {
        Self {
            buf: VecDeque::with_capacity(cap),
            cap,
        }
    }

    /// Push a new sample, evicting the oldest if at capacity.
    pub fn push(&mut self, item: T) {
        if self.buf.len() >= self.cap {
            self.buf.pop_front();
        }
        self.buf.push_back(item);
    }

    /// Current number of stored samples.
    pub fn len(&self) -> usize {
        self.buf.len()
    }

    /// Whether the buffer holds zero samples.
    pub fn is_empty(&self) -> bool {
        self.buf.is_empty()
    }
}

impl<T> Default for RingBuffer<T> {
    fn default() -> Self {
        Self::new(60)
    }
}

// ─── Widget tree ─────────────────────────────────────────────────────────────

/// Toolkit-agnostic tree of UI nodes produced by every widget's `render`.
///
/// A backend (GTK4, custom renderer, …) walks this tree to materialise
/// native widgets. Keeping the representation declarative lets the same
/// widget target multiple backends.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct WidgetTree {
    /// Flat list of nodes in pre-order traversal.
    pub nodes: Vec<WidgetNode>,
}

/// A single node in the [`WidgetTree`].
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct WidgetNode {
    /// Stable identifier (UUID) for diffing between frames.
    pub id: Uuid,
    /// Logical kind (gauge, label, container, …).
    pub kind: WidgetKind,
    /// Inline text, if any.
    pub label: Option<String>,
    /// Numeric value in `[0.0, 1.0]` for gauges.
    pub value: Option<f32>,
    /// Indices into [`WidgetTree::nodes`] of direct children.
    pub children: Vec<usize>,
}

/// Kinds of widget nodes the renderer understands.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum WidgetKind {
    /// Root container.
    #[default]
    Container,
    /// Vertical or horizontal box.
    Box,
    /// Radial or bar gauge.
    Gauge,
    /// Text label.
    Label,
    /// Spark-line chart.
    Sparkline,
}

// ─── ResourceMonitor ─────────────────────────────────────────────────────────

/// Top-level resource monitor widget.
///
/// Holds the latest [`SystemMetrics`] and a rolling history window. The
/// shell calls [`ResourceMonitor::update`] at 1Hz (default) with fresh
/// telemetry, then [`ResourceMonitor::render`] to produce a [`WidgetTree`].
#[derive(Debug, Clone)]
pub struct ResourceMonitor {
    /// Most recent telemetry snapshot.
    pub metrics: SystemMetrics,
    /// Rolling history of past snapshots.
    pub history: RingBuffer<SystemMetrics>,
    /// Update cadence; defaults to 1Hz.
    pub update_interval: Duration,
}

impl ResourceMonitor {
    /// Build a new monitor with the default 60-sample history and 1Hz cadence.
    pub fn new() -> Self {
        Self {
            metrics: SystemMetrics::default(),
            history: RingBuffer::new(60),
            update_interval: Duration::from_secs(1),
        }
    }

    /// Ingest a fresh telemetry snapshot, also pushing the previous one
    /// into history so spark-lines can be drawn.
    pub fn update(&mut self, metrics: SystemMetrics) {
        debug!(
            cores = metrics.cpu_per_core.len(),
            ram_used = metrics.ram_used,
            ram_total = metrics.ram_total,
            gpu = metrics.gpu,
            temp_c = metrics.temp,
            "resource monitor ingested telemetry sample"
        );
        self.history.push(self.metrics.clone());
        self.metrics = metrics;
    }

    /// Render the current state as a toolkit-agnostic widget tree.
    ///
    /// v0: returns an empty tree — the actual gauge layout lands in v1
    /// once the eBPF telemetry schema is finalised.
    pub fn render(&self) -> WidgetTree {
        // TODO(v1): emit a Box container holding one Gauge per CPU core,
        // a RAM bar, a GPU gauge, spark-lines for disk/net I/O, and a
        // temperature label.
        WidgetTree::default()
    }
}

impl Default for ResourceMonitor {
    fn default() -> Self {
        Self::new()
    }
}

// ─── Errors ──────────────────────────────────────────────────────────────────

/// Errors returned by the resource monitor (reserved for v1).
#[derive(Debug, Error)]
pub enum ResourceMonitorError {
    /// Telemetry channel reported a malformed sample.
    #[error("malformed telemetry: {0}")]
    MalformedTelemetry(String),
}

// v0: stub implementation
