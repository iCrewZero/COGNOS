//! Resource policy bounds — the safety envelope for everything the
//! scheduler is allowed to write.
//!
//! Per the capability lattice (docs/SPEC.md): the Scheduler agent may
//! adjust cgroup resource weights only "within predefined bounds", may
//! switch CPU governor only via systemd, and may never modify the cgroup
//! hierarchy. These constants are those predefined bounds. Writes outside
//! them are rejected with an explicit violation — never silently clamped —
//! so attempts surface in the audit log.

pub const AI_CPU_WEIGHT_MIN: u32 = 10;
pub const AI_CPU_WEIGHT_MAX: u32 = 400;
pub const AI_MEMORY_HIGH_MIN_GB: f32 = 0.2;
pub const AI_MEMORY_HIGH_MAX_GB: f32 = 4.0;

/// Closed governor allowlist. Adding a governor requires a spec update.
pub const ALLOWED_GOVERNORS: &[&str] = &["performance", "powersave", "schedutil"];

/// The only cgroup slice the scheduler may write to.
pub const AI_SLICE: &str = "cognos.slice/cognos-ai.slice";

#[derive(Debug, Clone, PartialEq)]
pub enum PolicyViolation {
    CpuWeightOutOfBounds(u32),
    MemoryHighOutOfBounds(f32),
    GovernorNotAllowed(String),
    SliceNotAllowed(String),
}

impl std::fmt::Display for PolicyViolation {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::CpuWeightOutOfBounds(w) => write!(
                f,
                "cpu.weight {} outside allowed range {}-{}",
                w, AI_CPU_WEIGHT_MIN, AI_CPU_WEIGHT_MAX
            ),
            Self::MemoryHighOutOfBounds(gb) => write!(
                f,
                "memory.high {}GB outside allowed range {}-{}GB",
                gb, AI_MEMORY_HIGH_MIN_GB, AI_MEMORY_HIGH_MAX_GB
            ),
            Self::GovernorNotAllowed(g) => {
                write!(f, "governor '{}' not in allowlist {:?}", g, ALLOWED_GOVERNORS)
            }
            Self::SliceNotAllowed(s) => {
                write!(f, "cgroup slice '{}' is not the AI slice '{}'", s, AI_SLICE)
            }
        }
    }
}

impl std::error::Error for PolicyViolation {}

pub fn validate_cpu_weight(weight: u32) -> Result<u32, PolicyViolation> {
    if (AI_CPU_WEIGHT_MIN..=AI_CPU_WEIGHT_MAX).contains(&weight) {
        Ok(weight)
    } else {
        Err(PolicyViolation::CpuWeightOutOfBounds(weight))
    }
}

pub fn validate_memory_high_gb(gb: f32) -> Result<f32, PolicyViolation> {
    if (AI_MEMORY_HIGH_MIN_GB..=AI_MEMORY_HIGH_MAX_GB).contains(&gb) {
        Ok(gb)
    } else {
        Err(PolicyViolation::MemoryHighOutOfBounds(gb))
    }
}

pub fn validate_governor(governor: &str) -> Result<&str, PolicyViolation> {
    if ALLOWED_GOVERNORS.contains(&governor) {
        Ok(governor)
    } else {
        Err(PolicyViolation::GovernorNotAllowed(governor.to_string()))
    }
}

pub fn validate_slice(slice: &str) -> Result<&str, PolicyViolation> {
    if slice == AI_SLICE {
        Ok(slice)
    } else {
        Err(PolicyViolation::SliceNotAllowed(slice.to_string()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn weight_bounds_enforced() {
        assert!(validate_cpu_weight(10).is_ok());
        assert!(validate_cpu_weight(400).is_ok());
        assert!(validate_cpu_weight(9).is_err());
        assert!(validate_cpu_weight(401).is_err());
        assert!(validate_cpu_weight(0).is_err());
    }

    #[test]
    fn memory_bounds_enforced() {
        assert!(validate_memory_high_gb(1.2).is_ok());
        assert!(validate_memory_high_gb(0.1).is_err());
        assert!(validate_memory_high_gb(8.0).is_err());
    }

    #[test]
    fn governor_allowlist_is_closed() {
        assert!(validate_governor("performance").is_ok());
        assert!(validate_governor("schedutil").is_ok());
        assert!(validate_governor("userspace").is_err());
        assert!(validate_governor("").is_err());
    }

    #[test]
    fn only_ai_slice_writable() {
        assert!(validate_slice("cognos.slice/cognos-ai.slice").is_ok());
        assert!(validate_slice("system.slice").is_err());
        assert!(validate_slice("user.slice").is_err());
    }
}
