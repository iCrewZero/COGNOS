//! Session context — tracks per-user session data for trust and risk scoring.


use chrono::Utc;

use crate::hal_types::{
    SessionContext,
};

pub struct SessionEngine;

impl SessionEngine {
    pub fn active(
        session_id: String,

        workspace: String,

        window_title: String,
    ) -> SessionContext {
        SessionContext {
            session_id,

            user_present: true,

            active_workspace:
                workspace,

            active_window_title:
                window_title,

            requires_confirmation:
                false,

            user_attention_score:
                1.0,
        }
    }

    pub fn unattended(
        session_id: String,
    ) -> SessionContext {
        SessionContext {
            session_id,

            user_present: false,

            active_workspace:
                "none".into(),

            active_window_title:
                "none".into(),

            requires_confirmation:
                true,

            user_attention_score:
                0.0,
        }
    }

    pub fn compute_attention_score(
        idle_ms: i64,

        focused: bool,

        fullscreen: bool,
    ) -> f32 {
        let mut score: f32 = 1.0;

        if idle_ms > 60_000 {
            score -= 0.35;
        }

        if idle_ms > 300_000 {
            score -= 0.45;
        }

        if !focused {
            score -= 0.15;
        }

        if fullscreen {
            score += 0.05;
        }

        score.clamp(0.0, 1.0)
    }

    pub fn should_require_confirmation(
        context:
            &SessionContext,

        action_risk: f32,
    ) -> bool {
        if !context.user_present {
            return true;
        }

        if context
            .user_attention_score
            < 0.35
        {
            return true;
        }

        action_risk > 0.65
    }

    pub fn session_staleness(
        last_activity_ms: i64,
    ) -> f32 {
        let now =
            Utc::now()
                .timestamp_millis();

        let delta =
            (
                now
                    - last_activity_ms
            ) as f32;

        (
            delta
                / 600_000.0
        )
            .clamp(0.0, 1.0)
    }

    pub fn confidence_modifier(
        context:
            &SessionContext,
    ) -> f32 {
        let mut modifier = 1.0;

        if !context.user_present {
            modifier -= 0.45;
        }

        modifier -=
            (
                1.0
                    - context
                        .user_attention_score
            )
                * 0.30;

        modifier.clamp(0.0, 1.0)
    }

    pub fn dangerous_background_execution(
        context:
            &SessionContext,

        action_risk: f32,
    ) -> bool {
        !context.user_present
            && action_risk > 0.70
    }

    pub fn requires_user_presence(
        irreversible: bool,

        context:
            &SessionContext,
    ) -> bool {
        irreversible
            && !context.user_present
    }

    pub fn behavioral_consistency(
        expected_workspace: &str,

        current_workspace: &str,
    ) -> f32 {
        if expected_workspace
            == current_workspace
        {
            1.0
        } else {
            0.45
        }
    }

    pub fn environmental_trust(
        context:
            &SessionContext,
    ) -> f32 {
        let mut trust: f32 = 1.0;

        if !context.user_present {
            trust -= 0.50;
        }

        if context
            .active_window_title
            .contains(
                "unknown"
            )
        {
            trust -= 0.20;
        }

        trust.clamp(0.0, 1.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn attention_score_idle() {
        let score =
            SessionEngine::
                compute_attention_score(
                    600_000,
                    false,
                    false,
                );

        assert!(
            score < 0.5
        );
    }

    #[test]
    fn unattended_requires_confirmation() {
        let context =
            SessionEngine::unattended(
                "session-1"
                    .into()
            );

        assert!(
            SessionEngine::
                should_require_confirmation(
                    &context,
                    0.2,
                )
        );
    }

    #[test]
    fn background_danger_detected() {
        let context =
            SessionEngine::unattended(
                "session-1"
                    .into()
            );

        assert!(
            SessionEngine::
                dangerous_background_execution(
                    &context,
                    0.9,
                )
        );
    }
}