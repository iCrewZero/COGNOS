/// HAL Trust Calibration — per-user interrupt frequency tuning.
///
/// THIS FILE IS HUMAN-WRITTEN ONLY. Zero AI authorship.
///
/// The problem: HAL interrupt frequency has no universally correct answer.
/// This module lets the system learn what each user considers worth interrupting for.
/// All calibration changes are logged; the global model is never modified.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use chrono::Utc;
use serde::{Deserialize, Serialize};

// ─── Action classes ───────────────────────────────────────────────────────────

/// The distinct classes of action HAL can interrupt for.
/// Trust calibration is per-class — deletes don't affect app-launch thresholds.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ActionClass {
    FileDelete,
    FileMove,
    PackageInstall,
    AppLaunch,
    SystemConfig,
    NetworkChange,
    KernelAdjacent,
    AiGeneratedCode,
}

impl ActionClass {
    /// Conservative defaults for a new install.
    /// Higher threshold = interrupted more easily (more conservative).
    fn default_threshold(&self) -> f32 {
        match self {
            Self::FileDelete      => 0.40,
            Self::FileMove        => 0.25,
            Self::PackageInstall  => 0.50,
            Self::AppLaunch       => 0.10,
            Self::SystemConfig    => 0.60,
            Self::NetworkChange   => 0.60,
            Self::KernelAdjacent  => 0.80,
            Self::AiGeneratedCode => 0.80,
        }
    }

    /// The floor below which a threshold can never go, regardless of feedback.
    /// Kernel and AI code must always receive some scrutiny.
    fn minimum_threshold(&self) -> f32 {
        match self {
            Self::KernelAdjacent  => 0.60,
            Self::AiGeneratedCode => 0.60,
            _                     => 0.10,
        }
    }
}

// ─── Feedback types ───────────────────────────────────────────────────────────

/// User feedback on a HAL interrupt.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum Feedback {
    /// "That interruption was unnecessary." — lower threshold for this class.
    UnnecessaryInterrupt,
    /// "This was the right call." — no change (reinforces current threshold).
    CorrectInterrupt,
    /// "Always ask me about this." — raise threshold permanently to 0.9.
    AlwaysAskForThis,
}

// ─── Calibration record ───────────────────────────────────────────────────────

/// A single calibration change event, written to the audit log.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct CalibrationEvent {
    timestamp: String,
    action_class: ActionClass,
    feedback: Feedback,
    old_threshold: f32,
    new_threshold: f32,
}

// ─── Persisted state ──────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
struct PersistedCalibration {
    thresholds: HashMap<String, f32>,
}

// ─── TrustCalibration ─────────────────────────────────────────────────────────

/// Per-user trust calibration for HAL interrupt frequency.
///
/// Loaded at daemon startup. Saved on every change.
/// Thread-safe: wrap in Arc<Mutex<TrustCalibration>> for multi-threaded use.
pub struct TrustCalibration {
    thresholds: HashMap<ActionClass, f32>,
    calibration_path: PathBuf,
    audit_log_path: PathBuf,
}

impl TrustCalibration {
    /// Load from ~/.cognos/hal/calibration.json, or create with defaults.
    pub fn load() -> Self {
        let home = dirs::home_dir().unwrap_or_else(|| PathBuf::from("/tmp"));
        let calibration_path = home.join(".cognos/hal/calibration.json");
        let audit_log_path   = home.join(".cognos/audit.log");
        Self::load_from(&calibration_path, &audit_log_path)
    }

    /// Load from explicit paths (useful for testing).
    pub fn load_from(calibration_path: &Path, audit_log_path: &Path) -> Self {
        let thresholds = if calibration_path.exists() {
            Self::load_persisted(calibration_path)
        } else {
            Self::default_thresholds()
        };

        let cal = Self {
            thresholds,
            calibration_path: calibration_path.to_path_buf(),
            audit_log_path: audit_log_path.to_path_buf(),
        };
        // Write defaults on first run so the file exists for inspection
        if !calibration_path.exists() {
            let _ = cal.persist();
        }
        cal
    }

    /// Record user feedback for an action class and adjust the threshold.
    pub fn record_feedback(&mut self, action_class: ActionClass, feedback: Feedback) {
        let old = self.get_threshold(&action_class);
        let floor = action_class.minimum_threshold();

        let new = match feedback {
            Feedback::UnnecessaryInterrupt => {
                // Lower threshold by 0.05, respect floor
                (old - 0.05_f32).max(floor)
            }
            Feedback::CorrectInterrupt => {
                // No change — reinforces current threshold
                old
            }
            Feedback::AlwaysAskForThis => {
                // Raise threshold permanently to 0.9
                0.9_f32
            }
        };

        self.thresholds.insert(action_class.clone(), new);
        self.write_audit(&CalibrationEvent {
            timestamp: Utc::now().to_rfc3339(),
            action_class: action_class.clone(),
            feedback: feedback.clone(),
            old_threshold: old,
            new_threshold: new,
        });
        let _ = self.persist();

        log::info!(
            "[hal calibration] {:?} {:?}: {:.2} → {:.2}",
            action_class, feedback, old, new
        );
    }

    /// Get the current effective threshold for a given action class.
    /// HAL interrupts when the action's risk score >= this threshold.
    pub fn get_threshold(&self, action_class: &ActionClass) -> f32 {
        *self.thresholds.get(action_class)
            .unwrap_or(&action_class.default_threshold())
    }

    /// Returns all current thresholds for display.
    pub fn all_thresholds(&self) -> &HashMap<ActionClass, f32> {
        &self.thresholds
    }

    // ─── Private ─────────────────────────────────────────────────────────────

    fn default_thresholds() -> HashMap<ActionClass, f32> {
        use ActionClass::*;
        [
            FileDelete, FileMove, PackageInstall, AppLaunch,
            SystemConfig, NetworkChange, KernelAdjacent, AiGeneratedCode,
        ]
        .iter()
        .map(|c| (c.clone(), c.default_threshold()))
        .collect()
    }

    fn load_persisted(path: &Path) -> HashMap<ActionClass, f32> {
        let json = match std::fs::read_to_string(path) {
            Ok(s) => s,
            Err(e) => {
                log::warn!("[hal calibration] Failed to read {:?}: {}", path, e);
                return Self::default_thresholds();
            }
        };

        let persisted: PersistedCalibration = match serde_json::from_str(&json) {
            Ok(p) => p,
            Err(e) => {
                log::warn!("[hal calibration] Failed to parse calibration: {}", e);
                return Self::default_thresholds();
            }
        };

        // Merge with defaults to handle new action classes added after initial save
        let mut result = Self::default_thresholds();
        for (class_str, threshold) in &persisted.thresholds {
            if let Ok(class) = serde_json::from_str::<ActionClass>(&format!("\"{}\"", class_str)) {
                let floor = class.minimum_threshold();
                result.insert(class, threshold.clamp(floor, 1.0));
            }
        }
        result
    }

    fn persist(&self) -> std::io::Result<()> {
        let thresholds: HashMap<String, f32> = self.thresholds
            .iter()
            .map(|(k, v)| (format!("{:?}", k), *v))
            .collect();

        let persisted = PersistedCalibration { thresholds };
        let json = serde_json::to_string_pretty(&persisted)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e))?;

        if let Some(parent) = self.calibration_path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(&self.calibration_path, json)
    }

    fn write_audit(&self, event: &CalibrationEvent) {
        let entry = serde_json::json!({
            "ts": event.timestamp,
            "agent": "hal_calibration",
            "action": "threshold_adjusted",
            "action_class": format!("{:?}", event.action_class),
            "feedback": format!("{:?}", event.feedback),
            "old_threshold": event.old_threshold,
            "new_threshold": event.new_threshold,
            "outcome": "calibration_updated",
        });

        if let Some(parent) = self.audit_log_path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        if let Ok(mut f) = std::fs::OpenOptions::new()
            .create(true).append(true)
            .open(&self.audit_log_path)
        {
            use std::io::Write;
            let _ = writeln!(f, "{}", serde_json::to_string(&entry).unwrap_or_default());
        }
    }
}

// ─── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    fn cal(dir: &std::path::Path) -> TrustCalibration {
        TrustCalibration::load_from(
            &dir.join("calibration.json"),
            &dir.join("audit.log"),
        )
    }

    #[test]
    fn defaults_are_conservative() {
        let dir = tempdir().unwrap();
        let c = cal(dir.path());
        // New installs should interrupt fairly eagerly for dangerous actions
        assert!(c.get_threshold(&ActionClass::FileDelete) >= 0.3);
        assert!(c.get_threshold(&ActionClass::KernelAdjacent) >= 0.6);
        assert!(c.get_threshold(&ActionClass::AiGeneratedCode) >= 0.6);
    }

    #[test]
    fn unnecessary_interrupt_lowers_threshold() {
        let dir = tempdir().unwrap();
        let mut c = cal(dir.path());
        let before = c.get_threshold(&ActionClass::FileMove);
        c.record_feedback(ActionClass::FileMove, Feedback::UnnecessaryInterrupt);
        let after = c.get_threshold(&ActionClass::FileMove);
        assert!(after < before, "Unnecessary feedback should lower threshold");
    }

    #[test]
    fn always_ask_raises_to_0_9() {
        let dir = tempdir().unwrap();
        let mut c = cal(dir.path());
        c.record_feedback(ActionClass::AppLaunch, Feedback::AlwaysAskForThis);
        assert!((c.get_threshold(&ActionClass::AppLaunch) - 0.9).abs() < 0.001);
    }

    #[test]
    fn correct_interrupt_makes_no_change() {
        let dir = tempdir().unwrap();
        let mut c = cal(dir.path());
        let before = c.get_threshold(&ActionClass::PackageInstall);
        c.record_feedback(ActionClass::PackageInstall, Feedback::CorrectInterrupt);
        let after = c.get_threshold(&ActionClass::PackageInstall);
        assert!((before - after).abs() < 0.001);
    }

    #[test]
    fn kernel_and_ai_floors_respected() {
        let dir = tempdir().unwrap();
        let mut c = cal(dir.path());
        // Apply many "unnecessary" feedbacks to try to push below floor
        for _ in 0..100 {
            c.record_feedback(ActionClass::KernelAdjacent, Feedback::UnnecessaryInterrupt);
            c.record_feedback(ActionClass::AiGeneratedCode, Feedback::UnnecessaryInterrupt);
        }
        assert!(
            c.get_threshold(&ActionClass::KernelAdjacent) >= 0.60,
            "KernelAdjacent below floor: {}",
            c.get_threshold(&ActionClass::KernelAdjacent)
        );
        assert!(
            c.get_threshold(&ActionClass::AiGeneratedCode) >= 0.60,
            "AiGeneratedCode below floor: {}",
            c.get_threshold(&ActionClass::AiGeneratedCode)
        );
    }

    #[test]
    fn persists_and_reloads() {
        let dir = tempdir().unwrap();
        {
            let mut c = cal(dir.path());
            c.record_feedback(ActionClass::FileMove, Feedback::UnnecessaryInterrupt);
        }
        // Reload
        let c2 = cal(dir.path());
        // The lowered threshold should survive reload
        assert!(c2.get_threshold(&ActionClass::FileMove) < ActionClass::FileMove.default_threshold());
    }

    #[test]
    fn audit_log_written_on_change() {
        let dir = tempdir().unwrap();
        let mut c = cal(dir.path());
        c.record_feedback(ActionClass::FileDelete, Feedback::UnnecessaryInterrupt);
        let log_path = dir.path().join("audit.log");
        assert!(log_path.exists(), "Audit log not written");
        let content = std::fs::read_to_string(&log_path).unwrap();
        assert!(content.contains("threshold_adjusted"));
    }
}
