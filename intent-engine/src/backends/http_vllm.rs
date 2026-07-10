//! vLLM HTTP inference backend (OpenAI-compatible `/v1/completions`).
//!
//! Drives a local `vllm serve` instance with XGrammar structured output
//! constrained to the committed production JSON Schema
//! (`intent-llm-output.schema.json` v2). Same contract as the POC harness
//! (`scripts/eval_golden_quality.py`).
//!
//! Design mirrors [`super::http_llama::HttpLlamaBackend`]:
//! mandatory timeout, one schema-validation retry, distinct transport errors.

use std::time::Duration;

use async_trait::async_trait;
use serde::Deserialize;
use serde_json::{json, Value};

use crate::llm_output_schema::SCHEMA_REL_PATH;
use crate::parser::InferenceBackend;
use crate::prompt::build_prompt;
use crate::schema_validator::{parse_llm_output_with_context, SessionContext};

/// Embedded production schema (loaded once at construction).
const EMBEDDED_SCHEMA: &str = include_str!("../../schema/intent-llm-output.schema.json");

/// Matches the validated POC harness (`eval_golden_quality.py`).
const DEFAULT_MAX_TOKENS: u32 = 448;

/// Sampling temperature — deterministic structured JSON.
const TEMPERATURE: f32 = 0.0;

/// Provenance stamp set by the daemon after a successful vLLM inference.
pub const VLLM_SOURCE: &str = "vllm";

/// Backend that drives a local vLLM OpenAI-compatible server.
pub struct HttpVllmBackend {
    endpoint: String,
    model: String,
    timeout: Duration,
    client: reqwest::Client,
    json_schema: Value,
    max_tokens: u32,
}

#[derive(Deserialize)]
struct CompletionChoice {
    #[serde(default)]
    text: String,
}

#[derive(Deserialize)]
struct CompletionResponse {
    #[serde(default)]
    choices: Vec<CompletionChoice>,
}

impl HttpVllmBackend {
    /// Build a backend against `endpoint`, serving `model`, with the given
    /// request `timeout`. Loads the embedded production JSON Schema once.
    pub fn new(
        endpoint: impl Into<String>,
        model: impl Into<String>,
        timeout: Duration,
    ) -> Result<Self, String> {
        Self::with_schema_json(endpoint, model, timeout, load_embedded_schema()?)
    }

    /// Like [`HttpVllmBackend::new`], but with an explicit schema file path
    /// (e.g. from `intent.toml`). Falls back to the embedded schema when the
    /// file is missing or invalid.
    pub fn with_schema_path(
        endpoint: impl Into<String>,
        model: impl Into<String>,
        timeout: Duration,
        schema_path: impl AsRef<std::path::Path>,
    ) -> Result<Self, String> {
        let schema = match std::fs::read_to_string(schema_path.as_ref()) {
            Ok(raw) => serde_json::from_str(&raw).map_err(|e| {
                format!(
                    "invalid JSON schema at {}: {e}",
                    schema_path.as_ref().display()
                )
            })?,
            Err(e) => {
                log::warn!(
                    "schema file {} unreadable ({e}); using embedded {}",
                    schema_path.as_ref().display(),
                    SCHEMA_REL_PATH
                );
                load_embedded_schema()?
            }
        };
        Self::with_schema_json(endpoint, model, timeout, schema)
    }

    fn with_schema_json(
        endpoint: impl Into<String>,
        model: impl Into<String>,
        timeout: Duration,
        json_schema: Value,
    ) -> Result<Self, String> {
        let client = reqwest::Client::builder()
            .timeout(timeout)
            .build()
            .map_err(|e| format!("failed to build HTTP client: {e}"))?;

        Ok(Self {
            endpoint: endpoint.into().trim_end_matches('/').to_string(),
            model: model.into(),
            timeout,
            client,
            json_schema,
            max_tokens: DEFAULT_MAX_TOKENS,
        })
    }

    fn completions_url(&self) -> String {
        format!("{}/v1/completions", self.endpoint)
    }

    async fn complete(&self, prompt: &str) -> Result<String, String> {
        let body = json!({
            "model": self.model,
            "prompt": prompt,
            "temperature": TEMPERATURE,
            "max_tokens": self.max_tokens,
            "stream": false,
            "structured_outputs": {
                "json": self.json_schema
            }
        });

        let resp = self
            .client
            .post(self.completions_url())
            .timeout(self.timeout)
            .json(&body)
            .send()
            .await
            .map_err(|e| classify_reqwest_error(&e))?;

        let status = resp.status();
        if status != reqwest::StatusCode::OK {
            let detail = resp.text().await.unwrap_or_default();
            return Err(format!(
                "http status {}: {}",
                status.as_u16(),
                truncate(&detail, 200)
            ));
        }

        let parsed: CompletionResponse = resp
            .json()
            .await
            .map_err(|e| format!("malformed completion response: {e}"))?;

        let content = parsed
            .choices
            .first()
            .map(|c| c.text.trim().to_string())
            .unwrap_or_default();
        if content.is_empty() {
            return Err("empty response: vLLM returned no completion text".to_string());
        }
        Ok(content)
    }
}

fn load_embedded_schema() -> Result<Value, String> {
    serde_json::from_str(EMBEDDED_SCHEMA)
        .map_err(|e| format!("invalid embedded schema {}: {e}", SCHEMA_REL_PATH))
}

fn classify_reqwest_error(e: &reqwest::Error) -> String {
    if e.is_timeout() {
        format!("timeout: vLLM did not respond within the deadline ({e})")
    } else if e.is_connect() {
        format!("connection refused: cannot reach vLLM ({e})")
    } else {
        format!("request error: {e}")
    }
}

fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        let mut out: String = s.chars().take(max).collect();
        out.push('…');
        out
    }
}

#[async_trait]
impl InferenceBackend for HttpVllmBackend {
    async fn infer(
        &self,
        normalized_input: &str,
        session: &SessionContext,
    ) -> Result<String, String> {
        let prompt = build_prompt(normalized_input, session);

        let first = self.complete(&prompt).await?;

        if let Err(schema_err) =
            parse_llm_output_with_context(&first, normalized_input, session)
        {
            let corrective = format!(
                "{prompt}\n\n[CORRECTION] Your previous response was rejected by the \
                 schema validator: {schema_err}. Output ONLY a corrected JSON object \
                 that satisfies every field and its bounds. No prose, no code fences."
            );
            let second = self.complete(&corrective).await?;
            return match parse_llm_output_with_context(&second, normalized_input, session) {
                Ok(_) => Ok(second),
                Err(e) => Err(format!("schema validation failed after one retry: {e}")),
            };
        }

        Ok(first)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parse_llm_output;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;

    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, Request, Respond, ResponseTemplate};

    fn session() -> SessionContext {
        SessionContext {
            last_active_domain: Some("robotics".into()),
            last_active_files: vec!["motor.py".into()],
            current_time: "10:00".into(),
            time_since_last_session: Some("2h".into()),
        }
    }

    fn valid_intent() -> String {
        r#"{
            "goal": "open_workspace",
            "domain": "system",
            "confidence": 0.9,
            "ambiguity_score": 0.1,
            "risk_estimate": 0.0,
            "hal_pre_score": 0.0,
            "required_context": [],
            "candidate_actions": [],
            "disambiguation_required": false,
            "disambiguation_question": null,
            "escalate_to_cloud": false
        }"#
        .to_string()
    }

    fn completion_body(inner: &str) -> String {
        json!({
            "choices": [{ "text": inner, "index": 0 }]
        })
        .to_string()
    }

    struct Sequence {
        bodies: Vec<String>,
        calls: Arc<AtomicUsize>,
    }

    impl Respond for Sequence {
        fn respond(&self, _req: &Request) -> ResponseTemplate {
            let n = self.calls.fetch_add(1, Ordering::SeqCst);
            let body = self
                .bodies
                .get(n)
                .or_else(|| self.bodies.last())
                .cloned()
                .unwrap_or_default();
            ResponseTemplate::new(200).set_body_raw(body, "application/json")
        }
    }

    async fn mount_sequence(server: &MockServer, bodies: Vec<String>) -> Arc<AtomicUsize> {
        let calls = Arc::new(AtomicUsize::new(0));
        let responder = Sequence {
            bodies,
            calls: calls.clone(),
        };
        Mock::given(method("POST"))
            .and(path("/v1/completions"))
            .respond_with(responder)
            .mount(server)
            .await;
        calls
    }

    fn backend(endpoint: &str, timeout_ms: u64) -> HttpVllmBackend {
        HttpVllmBackend::new(
            endpoint,
            "Qwen/Qwen2.5-7B-Instruct-AWQ",
            Duration::from_millis(timeout_ms),
        )
        .expect("backend builds")
    }

    #[tokio::test]
    async fn success_single_call() {
        let server = MockServer::start().await;
        let intent = valid_intent();
        let calls = mount_sequence(&server, vec![completion_body(&intent)]).await;

        let out = backend(&server.uri(), 2000)
            .infer("open my robotics work", &session())
            .await
            .expect("infer should succeed");

        assert!(parse_llm_output(&out).is_ok());
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn timeout_is_distinct_error() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/completions"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_raw(completion_body(&valid_intent()), "application/json")
                    .set_delay(Duration::from_millis(1500)),
            )
            .mount(&server)
            .await;

        let err = backend(&server.uri(), 150)
            .infer("open my robotics work", &session())
            .await
            .expect_err("must time out");

        assert!(err.contains("timeout"), "got: {err}");
    }

    #[tokio::test]
    async fn unreachable_server_is_transport_error() {
        let err = backend("http://127.0.0.1:1", 800)
            .infer("open my robotics work", &session())
            .await
            .expect_err("must fail to reach the server");

        assert!(
            err.contains("connection refused")
                || err.contains("request error")
                || err.contains("timeout"),
            "got: {err}"
        );
    }

    #[tokio::test]
    async fn http_error_is_distinct_error() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/completions"))
            .respond_with(ResponseTemplate::new(500).set_body_string("model crashed"))
            .mount(&server)
            .await;

        let err = backend(&server.uri(), 2000)
            .infer("open my robotics work", &session())
            .await
            .expect_err("must surface the HTTP error");

        assert!(err.contains("http status 500"), "got: {err}");
    }

    #[tokio::test]
    async fn empty_response_is_distinct_error() {
        let server = MockServer::start().await;
        let calls = mount_sequence(&server, vec![completion_body("   ")]).await;

        let err = backend(&server.uri(), 2000)
            .infer("open my robotics work", &session())
            .await
            .expect_err("empty content must error");

        assert!(err.contains("empty response"), "got: {err}");
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn retry_then_success() {
        let server = MockServer::start().await;
        let intent = valid_intent();
        let calls = mount_sequence(
            &server,
            vec![
                completion_body("this is not valid json"),
                completion_body(&intent),
            ],
        )
        .await;

        let out = backend(&server.uri(), 2000)
            .infer("open my robotics work", &session())
            .await
            .expect("second attempt should succeed");

        assert!(parse_llm_output(&out).is_ok());
        assert_eq!(calls.load(Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn retry_then_failure() {
        let server = MockServer::start().await;
        let calls = mount_sequence(
            &server,
            vec![
                completion_body("still not json"),
                completion_body("also not json"),
            ],
        )
        .await;

        let err = backend(&server.uri(), 2000)
            .infer("open my robotics work", &session())
            .await
            .expect_err("both attempts invalid");

        assert!(err.contains("after one retry"), "got: {err}");
        assert_eq!(calls.load(Ordering::SeqCst), 2);
    }
}
