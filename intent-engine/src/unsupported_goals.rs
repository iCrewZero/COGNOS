//! Goals recognized by the parser but not executable in v1 (no HAL route).

/// Network goals: parsed and schema-valid, but blocked before dispatch/HAL.
pub const NON_EXECUTABLE_NETWORK_GOALS_V1: &[&str] = &["network_download", "network_send"];

/// Human-readable rejection when a goal is recognized but not executable.
pub const NON_EXECUTABLE_V1_MESSAGE: &str = "goal reconnu mais non supporté en v1";

/// Returns `Some(message)` when `goal` must not be dispatched.
pub fn non_executable_reason(goal: &str) -> Option<&'static str> {
    if NON_EXECUTABLE_NETWORK_GOALS_V1.contains(&goal) {
        Some(NON_EXECUTABLE_V1_MESSAGE)
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn network_goals_are_non_executable() {
        assert_eq!(
            non_executable_reason("network_download"),
            Some(NON_EXECUTABLE_V1_MESSAGE)
        );
        assert_eq!(
            non_executable_reason("network_send"),
            Some(NON_EXECUTABLE_V1_MESSAGE)
        );
    }

    #[test]
    fn benign_goals_are_executable() {
        assert_eq!(non_executable_reason("create_dir"), None);
        assert_eq!(non_executable_reason("open_file"), None);
    }
}
