// ============================================================================
// HAL — Trust Calibration
// COGNOS/OS Human Approval Layer
//
// THIS FILE IS HUMAN-WRITTEN AND HUMAN-REVIEWED ONLY.
// NO AI AUTHORSHIP. NO AI COMMITS.
//
// Manages per-user, per-action-class interrupt thresholds.
// Every change is audited. Every change is reversible by reading the log.
// ============================================================================

use chrono::Utc;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fmt;
use std::fs;
use std::io::{self, Write};
use std::path::PathBuf;

// ----------------------------------------------------------------------------
// Action classes
// ----------------------------------------------------------------------------

/// The class of action for which HAL may interrupt the user.
/// Each class has an independent threshold that can be calibrated separately.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ActionClass {
    FileDelete,
    FileMove,
    PackageInstall,
    AppLaunch,
    SystemConfig,
    NetworkChange,
    KernelAdjacent,
    AIGeneratedCode,
}

impl fmt::Display for ActionClass {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ActionClass::FileDelete       => write!(f, "FileDelete"),
            ActionClass::FileMove         => write!(f, "FileMove"),
            ActionClass::PackageInstall   => write!(f, "PackageInstall"),
            ActionClass::AppLaunch        => write!(f, "AppLaunch"),
            ActionClass::SystemConfig     => write!(f, "SystemConfig"),
            ActionClass::NetworkChange    => write!(f, "NetworkChange"),
            ActionClass::KernelAdjacent   => write!(f, "KernelAdjacent"),
            ActionClass::AIGeneratedCode  => write!(f, "AIGeneratedCode"),
        }
    }
}

// ----------------------------------------------------------------------------
// Feedback types
// ----------------------------------------------------------------------------

/// User feedback on whether an interrupt was appropriate.
/// This is how the user teaches HAL what is and isn't worth asking about.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum Feedback {
    /// The interrupt was not needed — lower the threshold for this class
    UnnecessaryInterrupt,
    /// The interrupt was correct — keep the threshold where it is
    CorrectInterrupt,
    /// Always ask for this class — raise threshold permanently to 0.9
    AlwaysAskForThis,
}

impl fmt::Display for Feedback {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Feedback::UnnecessaryInterrupt => write!(f, "UnnecessaryInterrupt"),
            Feedback::CorrectInterrupt     => write!(f, "CorrectInterrupt"),
            Feedback::AlwaysAskForThis     => write!(f, "AlwaysAskForThis"),
        }
    }
}

// ----------------------------------------------------------------------------
// Constants
// ----------------------------------------------------------------------------

/// How much to lower the threshold on UnnecessaryInterrupt feedback
const THRESHOLD_DECREMENT: f32 = 0.05;

/// The absolute floor for any action class (except hard-floored classes)
const GLOBAL_FLOOR: f32 = 0.1;

/// The permanent threshold set by AlwaysAskForThis feedback
const ALWAYS_ASK_THRESHOLD: f32 = 0.9;

/// Hard floor for KernelAdjacent — cannot be trained below this
const KERNEL_ADJACENT_HARD_FLOOR: f32 = 0.6;

/// Hard floor for AIGeneratedCode — cannot be trained below this
const AI_GENERATED_CODE_HARD_FLOOR: f32 = 0.6;

// ----------------------------------------------------------------------------
// Default thresholds (conservative — new install)
// ----------------------------------------------------------------------------

/// Returns the conservative default thresholds for a fresh install.
/// These are applied when no calibration file exists.
fn default_thresholds() -> HashMap<ActionClass, f32> {
    let mut map = HashMap::new();
    map.insert(ActionClass::FileDelete,      0.4);
    map.insert(ActionClass::FileMove,        0.25);
    map.insert(ActionClass::PackageInstall,  0.5);
    map.insert(ActionClass::AppLaunch,       0.1);
    map.insert(ActionClass::SystemConfig,    0.6);
    map.insert(ActionClass::NetworkChange,   0.6);
    map.insert(ActionClass::KernelAdjacent,  0.8);
    map.insert(ActionClass::AIGeneratedCode, 0.8);
    map
}

// ----------------------------------------------------------------------------
// TrustCalibration struct
// ----------------------------------------------------------------------------

/// Per-user HAL interrupt threshold calibration.
///
/// Stores one threshold per ActionClass. Thresholds drift based on user
/// feedback. All changes are logged to the audit trail.
///
/// HAL uses these thresholds to decide whether to interrupt the user
/// for a given action. If the computed risk score exceeds the threshold
/// for the action's class, HAL will interrupt.
#[derive(Debug, Serialize, Deserialize)]
pub struct TrustCalibration {
    /// The per-class thresholds. All values ∈ [0.1, 1.0].
    thresholds: HashMap<ActionClass, f32>,

    /// Path to the calibration JSON file on disk
    #[serde(skip)]
    calibration_path: PathBuf,

    /// Path to the system audit log
    #[serde(skip)]
    audit_log_path: PathBuf,
}

impl TrustCalibration {
    /// Load calibration from ~/.cognos/hal/calibration.json.
    ///
    /// If the file does not exist, creates it with conservative defaults.
    /// If the file is malformed, logs the error and falls back to defaults.
    pub fn load() -> io::Result<Self> {
        let calibration_path = calibration_file_path()?;
        let audit_log_path = audit_log_path()?;

        let thresholds = if calibration_path.exists() {
            let raw = fs::read_to_string(&calibration_path)?;
            match serde_json::from_str::<HashMap<ActionClass, f32>>(&raw) {
                Ok(loaded) => {
                    // Merge with defaults to handle new action classes added in updates
                    let mut merged = default_thresholds();
                    for (class, value) in loaded {
                        merged.insert(class, value);
                    }
                    merged
                }
                Err(e) => {
                    eprintln!(
                        "[HAL calibration] Failed to parse calibration file: {}. \
                         Using conservative defaults.",
                        e
                    );
                    default_thresholds()
                }
            }
        } else {
            let defaults = default_thresholds();
            // Write defaults to disk immediately so the user can inspect them
            let calibration = TrustCalibration {
                thresholds: defaults,
                calibration_path: calibration_path.clone(),
                audit_log_path: audit_log_path.clone(),
            };
            calibration.persist()?;
            return Ok(calibration);
        };

        Ok(TrustCalibration {
            thresholds,
            calibration_path,
            audit_log_path,
        })
    }

    /// Record user feedback for an action class and adjust the threshold.
    ///
    /// Rules:
    /// - UnnecessaryInterrupt: lower threshold by THRESHOLD_DECREMENT (floor: GLOBAL_FLOOR)
    /// - AlwaysAskForThis: raise threshold permanently to ALWAYS_ASK_THRESHOLD
    /// - CorrectInterrupt: no change
    ///
    /// Hard ceilings on KernelAdjacent and AIGeneratedCode are enforced after
    /// every feedback application.
    ///
    /// Every change is appended to the audit log.
    pub fn record_feedback(
        &mut self,
        action_class: ActionClass,
        feedback: Feedback,
    ) -> io::Result<()> {
        let old_threshold = self.get_threshold(action_class.clone());

        let new_threshold = match feedback {
            Feedback::UnnecessaryInterrupt => {
                let floor = self.hard_floor_for(&action_class);
                (old_threshold - THRESHOLD_DECREMENT).max(floor)
            }
            Feedback::AlwaysAskForThis => {
                ALWAYS_ASK_THRESHOLD
            }
            Feedback::CorrectInterrupt => {
                // No change
                return Ok(());
            }
        };

        // Apply hard floor enforcement after computing new value
        let enforced = self.enforce_hard_floors(&action_class, new_threshold);

        self.thresholds.insert(action_class.clone(), enforced);

        // Write to audit log before persisting to disk
        self.append_audit_log(&action_class, old_threshold, enforced, &feedback)?;

        // Persist calibration to disk
        self.persist()?;

        Ok(())
    }

    /// Returns the current effective threshold for a given action class.
    ///
    /// If the class has no stored threshold (should not happen after load,
    /// but handled defensively), returns the conservative default.
    pub fn get_threshold(&self, action_class: ActionClass) -> f32 {
        self.thresholds
            .get(&action_class)
            .copied()
            .unwrap_or_else(|| {
                // Defensive fallback — return conservative default
                *default_thresholds().get(&action_class).unwrap_or(&0.6)
            })
    }

    /// Returns the hard floor for a specific action class.
    /// KernelAdjacent and AIGeneratedCode have elevated hard floors.
    fn hard_floor_for(&self, class: &ActionClass) -> f32 {
        match class {
            ActionClass::KernelAdjacent   => KERNEL_ADJACENT_HARD_FLOOR,
            ActionClass::AIGeneratedCode  => AI_GENERATED_CODE_HARD_FLOOR,
            _                             => GLOBAL_FLOOR,
        }
    }

    /// Enforce hard floors after any threshold mutation.
    /// Returns the value with hard floors applied.
    fn enforce_hard_floors(&self, class: &ActionClass, value: f32) -> f32 {
        let floor = self.hard_floor_for(class);
        value.max(floor).min(1.0)
    }

    /// Serialize thresholds to JSON and write to calibration file.
    fn persist(&self) -> io::Result<()> {
        if let Some(parent) = self.calibration_path.parent() {
            fs::create_dir_all(parent)?;
        }
        let json = serde_json::to_string_pretty(&self.thresholds)
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
        fs::write(&self.calibration_path, json)?;
        Ok(())
    }

    /// Append a calibration change entry to the audit log.
    ///
    /// Format: ISO8601 timestamp | action_class | old_threshold | new_threshold | feedback
    fn append_audit_log(
        &self,
        class: &ActionClass,
        old: f32,
        new: f32,
        feedback: &Feedback,
    ) -> io::Result<()> {
        if let Some(parent) = self.audit_log_path.parent() {
            fs::create_dir_all(parent)?;
        }

        let timestamp = Utc::now().to_rfc3339();
        let entry = format!(
            "{} | HAL_CALIBRATION | class={} | old_threshold={:.3} | new_threshold={:.3} | feedback={}\n",
            timestamp, class, old, new, feedback
        );

        let mut file = fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.audit_log_path)?;

        file.write_all(entry.as_bytes())?;
        Ok(())
    }
}

// ----------------------------------------------------------------------------
// Path helpers
// ----------------------------------------------------------------------------

/// Returns the path to the calibration JSON file.
/// Creates parent directories if they do not exist.
fn calibration_file_path() -> io::Result<PathBuf> {
    let home = dirs_home()?;
    Ok(home.join(".cognos").join("hal").join("calibration.json"))
}

/// Returns the path to the system audit log.
fn audit_log_path() -> io::Result<PathBuf> {
    let home = dirs_home()?;
    Ok(home.join(".cognos").join("audit.log"))
}

/// Returns the user's home directory.
/// Returns an error if HOME is not set (should never happen on a real system).
fn dirs_home() -> io::Result<PathBuf> {
    std::env::var("HOME")
        .map(PathBuf::from)
        .map_err(|_| io::Error::new(io::ErrorKind::NotFound, "HOME environment variable not set"))
}

// ----------------------------------------------------------------------------
// Tests
// ----------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::env;
    use tempfile::TempDir;

    /// Set HOME to a temp dir for test isolation
    fn test_calibration(tmp: &TempDir) -> TrustCalibration {
        let home = tmp.path().to_str().unwrap();
        env::set_var("HOME", home);
        TrustCalibration {
            thresholds: default_thresholds(),
            calibration_path: tmp.path().join(".cognos/hal/calibration.json"),
            audit_log_path: tmp.path().join(".cognos/audit.log"),
        }
    }

    #[test]
    fn test_defaults_are_conservative() {
        let defaults = default_thresholds();
        assert!(*defaults.get(&ActionClass::KernelAdjacent).unwrap() >= 0.7);
        assert!(*defaults.get(&ActionClass::AIGeneratedCode).unwrap() >= 0.7);
    }

    #[test]
    fn test_unnecessary_interrupt_lowers_threshold() {
        let tmp = TempDir::new().unwrap();
        let mut cal = test_calibration(&tmp);
        let before = cal.get_threshold(ActionClass::FileMove);
        cal.record_feedback(ActionClass::FileMove, Feedback::UnnecessaryInterrupt).unwrap();
        let after = cal.get_threshold(ActionClass::FileMove);
        assert!(after < before, "Threshold should decrease: {} -> {}", before, after);
        assert!((before - after - THRESHOLD_DECREMENT).abs() < 0.001);
    }

    #[test]
    fn test_always_ask_raises_to_09() {
        let tmp = TempDir::new().unwrap();
        let mut cal = test_calibration(&tmp);
        cal.record_feedback(ActionClass::AppLaunch, Feedback::AlwaysAskForThis).unwrap();
        assert_eq!(cal.get_threshold(ActionClass::AppLaunch), ALWAYS_ASK_THRESHOLD);
    }

    #[test]
    fn test_correct_interrupt_no_change() {
        let tmp = TempDir::new().unwrap();
        let mut cal = test_calibration(&tmp);
        let before = cal.get_threshold(ActionClass::PackageInstall);
        cal.record_feedback(ActionClass::PackageInstall, Feedback::CorrectInterrupt).unwrap();
        let after = cal.get_threshold(ActionClass::PackageInstall);
        assert_eq!(before, after);
    }

    #[test]
    fn test_kernel_adjacent_hard_floor_respected() {
        let tmp = TempDir::new().unwrap();
        let mut cal = test_calibration(&tmp);
        // Spam UnnecessaryInterrupt 100 times — floor should hold
        for _ in 0..100 {
            cal.record_feedback(ActionClass::KernelAdjacent, Feedback::UnnecessaryInterrupt).unwrap();
        }
        assert!(
            cal.get_threshold(ActionClass::KernelAdjacent) >= KERNEL_ADJACENT_HARD_FLOOR,
            "KernelAdjacent hard floor violated: {}",
            cal.get_threshold(ActionClass::KernelAdjacent)
        );
    }

    #[test]
    fn test_ai_generated_code_hard_floor_respected() {
        let tmp = TempDir::new().unwrap();
        let mut cal = test_calibration(&tmp);
        for _ in 0..100 {
            cal.record_feedback(ActionClass::AIGeneratedCode, Feedback::UnnecessaryInterrupt).unwrap();
        }
        assert!(
            cal.get_threshold(ActionClass::AIGeneratedCode) >= AI_GENERATED_CODE_HARD_FLOOR,
            "AIGeneratedCode hard floor violated: {}",
            cal.get_threshold(ActionClass::AIGeneratedCode)
        );
    }

    #[test]
    fn test_global_floor_respected() {
        let tmp = TempDir::new().unwrap();
        let mut cal = test_calibration(&tmp);
        for _ in 0..100 {
            cal.record_feedback(ActionClass::AppLaunch, Feedback::UnnecessaryInterrupt).unwrap();
        }
        assert!(
            cal.get_threshold(ActionClass::AppLaunch) >= GLOBAL_FLOOR,
            "Global floor violated: {}",
            cal.get_threshold(ActionClass::AppLaunch)
        );
    }
}
