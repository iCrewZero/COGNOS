//! Golden corpus tests for the intent prompt + schema contract.
//!
//! Each `tests/golden/*.json` file is `{ input, context, expected_intent }`.
//! For every case we assert two things:
//!
//! 1. `build_prompt(input, context)` injects the live session context and
//!    stays within the token budget.
//! 2. `expected_intent` is an *exact*, schema-valid intent — it parses through
//!    the real validator (`parse_llm_output`) and satisfies every documented
//!    cross-field rule (disambiguation, cloud escalation, candidate presence).
//!
//! These goldens are the fixtures a real model will be scored against later, so
//! the `expected_intent`s must be correct, not approximate.

use std::fs;
use std::path::PathBuf;

use cognos_intent_engine::prompt::{build_prompt, estimate_tokens, MAX_PROMPT_TOKENS};
use cognos_intent_engine::schema_validator::{parse_llm_output, IntentSchema, SessionContext};

/// One decoded golden case.
struct GoldenCase {
    name: String,
    input: String,
    context: SessionContext,
    /// The `expected_intent` object, re-serialized so it can be fed to the
    /// real parser/validator exactly as an LLM's output would be.
    expected_intent_json: String,
}

fn golden_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("golden")
}

fn load_cases() -> Vec<GoldenCase> {
    let dir = golden_dir();
    let mut cases = Vec::new();

    let mut entries: Vec<_> = fs::read_dir(&dir)
        .unwrap_or_else(|e| panic!("cannot read golden dir {}: {e}", dir.display()))
        .filter_map(Result::ok)
        .map(|e| e.path())
        .filter(|p| p.extension().map(|x| x == "json").unwrap_or(false))
        .collect();
    entries.sort();

    for path in entries {
        let name = path
            .file_name()
            .and_then(|f| f.to_str())
            .unwrap_or("<?>")
            .to_string();
        let raw = fs::read_to_string(&path)
            .unwrap_or_else(|e| panic!("cannot read {name}: {e}"));
        let value: serde_json::Value = serde_json::from_str(&raw)
            .unwrap_or_else(|e| panic!("{name} is not valid JSON: {e}"));

        let input = value["input"]
            .as_str()
            .unwrap_or_else(|| panic!("{name}: missing string `input`"))
            .to_string();
        let context: SessionContext = serde_json::from_value(value["context"].clone())
            .unwrap_or_else(|e| panic!("{name}: `context` is not a SessionContext: {e}"));
        let expected = value
            .get("expected_intent")
            .unwrap_or_else(|| panic!("{name}: missing `expected_intent`"));
        let expected_intent_json = serde_json::to_string(expected)
            .unwrap_or_else(|e| panic!("{name}: cannot re-serialize expected_intent: {e}"));

        cases.push(GoldenCase {
            name,
            input,
            context,
            expected_intent_json,
        });
    }
    cases
}

/// The prompt must inject the concrete session context and stay bounded.
fn assert_prompt_contract(case: &GoldenCase) {
    let prompt = build_prompt(&case.input, &case.context);

    // Bounded size.
    let tokens = estimate_tokens(&prompt);
    assert!(
        tokens < MAX_PROMPT_TOKENS,
        "{}: prompt is {tokens} tokens (budget {MAX_PROMPT_TOKENS})",
        case.name
    );

    // The user input is present verbatim.
    assert!(
        prompt.contains(&case.input),
        "{}: prompt does not contain the user input",
        case.name
    );

    // Injected session context is present.
    assert!(
        prompt.contains(&case.context.current_time),
        "{}: prompt is missing current_time `{}`",
        case.name,
        case.context.current_time
    );
    if let Some(domain) = &case.context.last_active_domain {
        assert!(
            prompt.contains(domain),
            "{}: prompt is missing active domain `{domain}`",
            case.name
        );
    }
    for file in &case.context.last_active_files {
        assert!(
            prompt.contains(file),
            "{}: prompt is missing recent file `{file}`",
            case.name
        );
    }

    // The contract itself must be described (fields + bounds + rules).
    for token in ["candidate_actions", "disambiguation_required", "[0.0, 1.0]"] {
        assert!(
            prompt.contains(token),
            "{}: prompt must describe `{token}`",
            case.name
        );
    }
}

/// The expected_intent must be schema-valid and satisfy every documented rule.
fn assert_expected_intent_exact(case: &GoldenCase) {
    let schema: IntentSchema = parse_llm_output(&case.expected_intent_json).unwrap_or_else(|e| {
        panic!("{}: expected_intent failed schema validation: {e}", case.name)
    });

    // raw_input must echo the golden input exactly.
    assert_eq!(
        schema.raw_input, case.input,
        "{}: expected_intent.raw_input must equal the input",
        case.name
    );

    // goal is non-empty (the parser enforces this; assert for clarity).
    assert!(!schema.goal.trim().is_empty(), "{}: empty goal", case.name);

    // All bounded floats are in range.
    for (field, v) in [
        ("confidence", schema.confidence),
        ("ambiguity_score", schema.ambiguity_score),
        ("risk_estimate", schema.risk_estimate),
        ("hal_pre_score", schema.hal_pre_score),
    ] {
        assert!(
            (0.0..=1.0).contains(&v),
            "{}: {field} = {v} out of [0,1]",
            case.name
        );
    }
    for c in &schema.candidate_actions {
        assert!(
            (0.0..=1.0).contains(&c.confidence) && (0.0..=1.0).contains(&c.recency_score),
            "{}: candidate `{}` has out-of-range score",
            case.name,
            c.target
        );
    }

    // Disambiguation rule: ambiguity_score > 0.6  <=>  disambiguation_required.
    assert_eq!(
        schema.ambiguity_score > 0.6,
        schema.disambiguation_required,
        "{}: disambiguation_required must track ambiguity_score > 0.6",
        case.name
    );

    // A required disambiguation needs a question AND >= 2 candidates to choose.
    if schema.disambiguation_required {
        assert!(
            schema.disambiguation_question.is_some(),
            "{}: disambiguation_required but no question",
            case.name
        );
        assert!(
            schema.candidate_actions.len() >= 2,
            "{}: disambiguation_required needs >= 2 candidate_actions",
            case.name
        );
    }

    // Cloud escalation: LLM path — low confidence or explicit cloud_reasoning.
    // Keyword fallback (offline registry) must never escalate.
    let expect_escalate = if schema.source.as_deref() == Some("keyword_fallback") {
        false
    } else {
        schema.confidence < 0.75
            || schema
                .required_context
                .iter()
                .any(|c| c == "cloud_reasoning")
    };
    assert_eq!(
        schema.escalate_to_cloud, expect_escalate,
        "{}: escalate_to_cloud must follow the LLM-path escalation rule",
        case.name
    );
}

#[test]
fn golden_corpus_has_expected_size() {
    let cases = load_cases();
    assert_eq!(
        cases.len(),
        15,
        "expected 15 golden cases, found {}",
        cases.len()
    );
}

#[test]
fn golden_prompts_are_well_formed() {
    for case in load_cases() {
        assert_prompt_contract(&case);
    }
}

#[test]
fn golden_expected_intents_are_schema_exact() {
    for case in load_cases() {
        assert_expected_intent_exact(&case);
    }
}

#[test]
fn golden_corpus_covers_required_scenarios() {
    // Guard the corpus composition so the scenario coverage can't silently
    // regress: simple file action, multi-step, ambiguous/disambiguation,
    // out-of-scope, network, dangerous delete, empty/noise, EN + FR.
    let names: Vec<String> = load_cases().into_iter().map(|c| c.name).collect();
    let has = |needle: &str| names.iter().any(|n| n.contains(needle));

    assert!(has("simple_file"), "missing a simple file action case");
    assert!(has("multistep"), "missing a multi-step case");
    assert!(has("ambiguous"), "missing an ambiguous/disambiguation case");
    assert!(has("out_of_scope"), "missing an out-of-scope case");
    assert!(has("network"), "missing a network case");
    assert!(has("dangerous_delete"), "missing a dangerous delete case");
    assert!(has("empty_input"), "missing an empty-input case");
    assert!(has("noise_input"), "missing a noise-input case");
    assert!(has("_en"), "missing English cases");
    assert!(has("_fr"), "missing French cases");
}
