//! COGNOS adaptive resource scheduler.
//!
//! Reads telemetry (eBPF in Phase 4; /proc + /sys best-effort in v0),
//! detects the current usage scenario, and applies bounded resource policy
//! to the AI cgroup slice. The AI takes only what is allocated — never more.
//!
//! Capability lattice constraints (docs/SPEC.md):
//! - adjust cgroup resource weights only within predefined bounds
//! - switch CPU governor via systemd, not directly
//! - never modify the cgroup hierarchy or isolcpus configuration

pub mod daemon;
pub mod resource_policy;
pub mod telemetry;

pub use daemon::{EbpfReader, ProcessInfo, Scenario, SchedulerDaemon, SystemMetrics};
pub use resource_policy::{
    validate_cpu_weight, validate_governor, validate_memory_high_gb,
    validate_slice, PolicyViolation, AI_CPU_WEIGHT_MAX, AI_CPU_WEIGHT_MIN,
    AI_SLICE, ALLOWED_GOVERNORS,
};
pub use telemetry::{FixedReader, ProcfsReader};

// Runtime and predictor are declared as modules but may need dependency
// fixes before they compile. Wire them in so `cargo check` reports
// real errors instead of silent dead-code.
pub mod runtime;
pub mod predictor;
