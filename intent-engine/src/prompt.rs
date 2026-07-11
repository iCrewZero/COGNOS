//! Prompt construction for the intent LLM.
//!
//! [`build_prompt`] renders the compact system prompt that teaches a local
//! model the [`IntentSchema`](crate::schema_validator::IntentSchema) contract
//! and injects the live [`SessionContext`]. The model's raw output is never
//! trusted: it always flows back through
//! [`parse_llm_output`](crate::schema_validator::parse_llm_output).
//!
//! The prompt is intentionally small (< [`MAX_PROMPT_TOKENS`]). It describes
//! every field, its type and bounds, the disambiguation rule
//! (`ambiguity_score > 0.6`), and the cloud-escalation rule
//! (`confidence < 0.75` on the LLM path only — keyword/offline fallback never
//! escalates), all of which mirror the parser/validator exactly so the golden
//! corpus can score a real model against the same contract.

use crate::schema_validator::SessionContext;

/// Upper bound on the rendered prompt size, in estimated tokens.
pub const MAX_PROMPT_TOKENS: usize = 800;

/// The fixed system-prompt body: role + schema contract + rules.
///
/// Kept as a single `const` so its size is stable and auditable. Session
/// context and user input are appended by [`build_prompt`].
const SYSTEM_PROMPT: &str = "\
You are the COGNOS/OS intent parser. Output EXACTLY ONE JSON object (IntentSchema). \
JSON only — no prose, no fences.

SCHEMA (all *_score/confidence/*_estimate floats in [0.0, 1.0]):
- intent_id: UUID v4; raw_input: verbatim user text; goal: canonical string; \
domain: string|null; confidence; ambiguity_score; risk_estimate; required_context[].
- candidate_actions[{action,target,confidence,recency_score}] — concrete steps; \
MUST be [] for out_of_scope, await_input, empty/noise; never invent actions.
- disambiguation_required; disambiguation_question|null; session_context object; \
hal_pre_score; escalate_to_cloud.

RULES:
- ambiguity_score>0.6 => disambiguation_required=true, disambiguation_question set, \
>=2 candidate_actions.
- ambiguity_score<=0.6 => disambiguation_required=false, disambiguation_question=null.
- confidence<0.75 => escalate_to_cloud=true when unsure; else false unless \
cloud_reasoning in required_context.
- Deletes/format/overwrite => high risk_estimate and hal_pre_score.

GOALS (pick one): open_file, search_files, open_workspace, package_and_convert, \
code_task, network_download, network_send, delete_path, out_of_scope, await_input, \
create_dir, install_package.

MULTI-STEP (\"then\"/\"puis\"/\"and run\"): composite goal (package_and_convert or \
code_task) + >=2 candidate_actions (one per step); disambiguation_required=false if clear.

DISAMBIGUATION: vague project reference + 2+ plausible recent_files => \
ambiguity_score>0.6, disambiguation_required=true, question, >=2 open_files candidates.

EDGE: \"\"=>await_input+[] (NOT out_of_scope); noise/gibberish=>await_input; chit-chat=>out_of_scope+[]; \
download=>network_download; email/send=>network_send.

EXAMPLES:
- \"install ffmpeg puis convertis ma vidéo\" => package_and_convert; install_package/ffmpeg; \
convert_media/~/media/clip.mov.
- \"open the robotics project\" + [motor.py,pid.py] => open_workspace; ambiguity>0.6; \
disambiguation_required=true; question; 2 open_files targets.
- \"ouvre le projet robotique\" + [bras.py,rover.py] => même règle (open_workspace + disambiguation).";

/// Build the full intent prompt for `input` under session `ctx`.
///
/// The returned string is `SYSTEM_PROMPT` followed by the injected session
/// context (active domain, recent files, current time, idle gap) and the user
/// input. Guaranteed to describe the schema contract and to stay within
/// [`MAX_PROMPT_TOKENS`] for realistic inputs.
pub fn build_prompt(input: &str, ctx: &SessionContext) -> String {
    let domain = ctx.last_active_domain.as_deref().unwrap_or("none");
    let files = if ctx.last_active_files.is_empty() {
        "none".to_string()
    } else {
        ctx.last_active_files.join(", ")
    };
    let idle = ctx.time_since_last_session.as_deref().unwrap_or("unknown");

    format!(
        "{system}\n\nSESSION CONTEXT:\n\
- active_domain: {domain}\n\
- recent_files: {files}\n\
- current_time: {time}\n\
- time_since_last_session: {idle}\n\n\
USER INPUT:\n{input}\n\n\
Respond with the JSON object now.",
        system = SYSTEM_PROMPT,
        domain = domain,
        files = files,
        time = ctx.current_time,
        idle = idle,
        input = input,
    )
}

/// Rough token estimate for budgeting.
///
/// Uses the conventional ~4-characters-per-token heuristic for English and
/// takes the larger of that and the whitespace word count, so the estimate is
/// conservative (never under-counts a word-dense prompt). This is a budgeting
/// aid, not a real tokenizer.
pub fn estimate_tokens(text: &str) -> usize {
    let char_estimate = (text.chars().count() + 3) / 4;
    let word_estimate = text.split_whitespace().count();
    char_estimate.max(word_estimate)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ctx() -> SessionContext {
        SessionContext {
            last_active_domain: Some("robotics".into()),
            last_active_files: vec!["motor.py".into(), "config.yaml".into()],
            current_time: "14:32".into(),
            time_since_last_session: Some("2h".into()),
        }
    }

    #[test]
    fn prompt_injects_session_context() {
        let p = build_prompt("open my robotics work", &ctx());
        assert!(p.contains("robotics"));
        assert!(p.contains("motor.py"));
        assert!(p.contains("config.yaml"));
        assert!(p.contains("14:32"));
        assert!(p.contains("2h"));
        assert!(p.contains("open my robotics work"));
    }

    #[test]
    fn prompt_describes_the_contract() {
        let p = build_prompt("x", &ctx());
        for key in [
            "intent_id",
            "goal",
            "confidence",
            "ambiguity_score",
            "risk_estimate",
            "candidate_actions",
            "disambiguation_required",
            "escalate_to_cloud",
            "hal_pre_score",
        ] {
            assert!(p.contains(key), "prompt must mention field `{key}`");
        }
        assert!(p.contains("[0.0, 1.0]"), "prompt must state the float bounds");
        assert!(p.contains("0.6"), "prompt must state the disambiguation rule");
        assert!(p.contains("0.75"), "prompt must state the escalation rule");
    }

    #[test]
    fn prompt_stays_within_budget() {
        let p = build_prompt("open my robotics work", &ctx());
        assert!(
            estimate_tokens(&p) < MAX_PROMPT_TOKENS,
            "prompt is {} tokens, budget is {}",
            estimate_tokens(&p),
            MAX_PROMPT_TOKENS
        );
    }

    #[test]
    fn empty_context_renders_none() {
        let empty = SessionContext {
            last_active_domain: None,
            last_active_files: vec![],
            current_time: "10:00".into(),
            time_since_last_session: None,
        };
        let p = build_prompt("", &empty);
        assert!(p.contains("active_domain: none"));
        assert!(p.contains("recent_files: none"));
        assert!(p.contains("current_time: 10:00"));
    }
}
