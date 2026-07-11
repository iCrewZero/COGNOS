//! llama-server HTTP inference backend.
//!
//! Drives a local [llama.cpp](https://github.com/ggml-org/llama.cpp)
//! `llama-server` through its `/completion` endpoint, constraining the output
//! with the embedded GBNF grammar so the model can only emit an
//! `IntentSchema`-shaped JSON object. Targets Qwen3 7B Q4_K_M but is
//! model-agnostic.
//!
//! Design notes:
//! - `reqwest` is configured with a **mandatory** request timeout.
//! - The GBNF grammar is embedded at build time and loaded **once** (in
//!   [`HttpLlamaBackend::new`]), never rebuilt per request.
//! - Transport / HTTP / empty-body failures map to **distinct** `Err(String)`.
//! - Exactly **one** retry, and only when the returned JSON fails schema
//!   validation; the retry appends a corrective note to the prompt.
//! - No FFI: everything is a pure-Rust HTTP call.

use std::time::Duration;

use async_trait::async_trait;
use serde::Deserialize;
use serde_json::json;

use crate::parser::InferenceBackend;
use crate::prompt::build_prompt;
use crate::schema_validator::{parse_llm_output_with_context, SessionContext};

/// The GBNF grammar, embedded at build time and loaded once (never per request).
const INTENT_GRAMMAR: &str = include_str!("../../grammar/intent.gbnf");

/// Default number of tokens to predict for one intent object.
const DEFAULT_N_PREDICT: u32 = 512;

/// Sampling temperature — low, because we want deterministic, valid JSON.
const TEMPERATURE: f32 = 0.1;

/// Backend that drives a local llama-server (`/completion`, GBNF-aware).
pub struct HttpLlamaBackend {
    /// Base URL of the running llama-server, e.g. `http://127.0.0.1:8080`.
    endpoint: String,
    /// Model identifier passed through to the server.
    model: String,
    /// Per-request timeout (also applied at the client level).
    timeout: Duration,
    /// Reqwest client, built once with the mandatory timeout.
    client: reqwest::Client,
    /// The GBNF grammar, loaded at construction time.
    grammar: String,
    /// Token budget per completion.
    n_predict: u32,
}

/// The subset of the llama-server `/completion` response we consume.
#[derive(Deserialize)]
struct CompletionResponse {
    #[serde(default)]
    content: String,
}

impl HttpLlamaBackend {
    /// Build a backend against `endpoint`, serving `model`, with the given
    /// request `timeout`.
    ///
    /// The reqwest client is created with a mandatory timeout and the GBNF
    /// grammar is loaded now (at startup), not per request.
    pub fn new(
        endpoint: impl Into<String>,
        model: impl Into<String>,
        timeout: Duration,
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
            grammar: INTENT_GRAMMAR.to_string(),
            n_predict: DEFAULT_N_PREDICT,
        })
    }

    /// Like [`HttpLlamaBackend::new`], but with an explicit grammar string
    /// (e.g. loaded from the configured grammar path at startup). The grammar
    /// is still loaded once here, never per request.
    pub fn with_grammar(
        endpoint: impl Into<String>,
        model: impl Into<String>,
        timeout: Duration,
        grammar: impl Into<String>,
    ) -> Result<Self, String> {
        let mut backend = Self::new(endpoint, model, timeout)?;
        backend.grammar = grammar.into();
        Ok(backend)
    }

    /// The GBNF grammar embedded in the binary.
    pub fn grammar() -> &'static str {
        INTENT_GRAMMAR
    }

    fn completion_url(&self) -> String {
        format!("{}/completion", self.endpoint)
    }

    /// One HTTP round-trip against `/completion`.
    ///
    /// Maps timeout, connection-refused, non-200, and empty-body failures to
    /// distinct error strings.
    async fn complete(&self, prompt: &str) -> Result<String, String> {
        let body = json!({
            "prompt": prompt,
            "grammar": self.grammar,
            "model": self.model,
            "temperature": TEMPERATURE,
            "n_predict": self.n_predict,
            "cache_prompt": true,
            "stream": false,
        });

        let resp = self
            .client
            .post(self.completion_url())
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

        let content = parsed.content.trim().to_string();
        if content.is_empty() {
            return Err("empty response: llama-server returned no content".to_string());
        }
        Ok(content)
    }
}

/// Classify a reqwest failure into a distinct, human-readable error.
fn classify_reqwest_error(e: &reqwest::Error) -> String {
    if e.is_timeout() {
        format!("timeout: llama-server did not respond within the deadline ({e})")
    } else if e.is_connect() {
        format!("connection refused: cannot reach llama-server ({e})")
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
impl InferenceBackend for HttpLlamaBackend {
    async fn infer(
        &self,
        normalized_input: &str,
        session: &SessionContext,
    ) -> Result<String, String> {
        let prompt = build_prompt(normalized_input, session);

        let first = self.complete(&prompt).await?;

        // The single retry fires ONLY on schema-validation failure. Transport
        // and HTTP errors already short-circuited above via `?`.
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

    /// A minimal intent JSON string that passes `parse_llm_output`.
    fn valid_intent() -> String {
        r#"{
            "intent_id": "550e8400-e29b-41d4-a716-446655440000",
            "raw_input": "open my robotics work",
            "goal": "open_workspace",
            "confidence": 0.9,
            "ambiguity_score": 0.1,
            "risk_estimate": 0.0,
            "hal_pre_score": 0.0,
            "required_context": [],
            "candidate_actions": [],
            "disambiguation_required": false,
            "session_context": {"last_active_files": [], "current_time": "10:00"},
            "escalate_to_cloud": false
        }"#
        .to_string()
    }

    /// Wrap intent text as a llama-server `/completion` response body.
    fn completion_body(inner: &str) -> String {
        json!({ "content": inner }).to_string()
    }

    /// A responder that walks a fixed list of bodies, one per successive call,
    /// counting invocations so tests can assert the exact number of requests.
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
            .and(path("/completion"))
            .respond_with(responder)
            .mount(server)
            .await;
        calls
    }

    fn backend(endpoint: &str, timeout_ms: u64) -> HttpLlamaBackend {
        HttpLlamaBackend::new(endpoint, "qwen3-7b-q4_k_m", Duration::from_millis(timeout_ms))
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

        assert!(parse_llm_output(&out).is_ok(), "returned text must validate");
        assert_eq!(calls.load(Ordering::SeqCst), 1, "exactly one HTTP call");
    }

    #[tokio::test]
    async fn timeout_is_distinct_error() {
        let server = MockServer::start().await;
        // Respond well past the client's deadline.
        Mock::given(method("POST"))
            .and(path("/completion"))
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

        assert!(err.contains("timeout"), "expected a timeout error, got: {err}");
    }

    #[tokio::test]
    async fn unreachable_server_is_transport_error() {
        // Nothing is listening on this port. Depending on the OS/firewall the
        // dead endpoint surfaces as a connect refusal (RST) or, when SYNs are
        // silently dropped, as a timeout — both are transport-level errors,
        // distinct from the HTTP / empty-body / schema error paths, and must
        // never be mistaken for a successful completion.
        let err = backend("http://127.0.0.1:1", 800)
            .infer("open my robotics work", &session())
            .await
            .expect_err("must fail to reach the server");

        assert!(
            err.contains("connection refused")
                || err.contains("request error")
                || err.contains("timeout"),
            "expected a transport error, got: {err}"
        );
        // It is emphatically not an HTTP-status or schema error.
        assert!(!err.contains("http status"), "got: {err}");
        assert!(!err.contains("after one retry"), "got: {err}");
    }

    #[tokio::test]
    async fn http_error_is_distinct_error() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/completion"))
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
        assert_eq!(calls.load(Ordering::SeqCst), 1, "no retry on empty body");
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
        assert_eq!(
            calls.load(Ordering::SeqCst),
            2,
            "one invalid response must trigger exactly one retry"
        );
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
            .expect_err("both attempts invalid → error");

        assert!(
            err.contains("after one retry"),
            "expected post-retry schema error, got: {err}"
        );
        assert_eq!(
            calls.load(Ordering::SeqCst),
            2,
            "must retry exactly once, then give up"
        );
    }
}
