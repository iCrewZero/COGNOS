//! Agent execution — invokes Python file_agent for filesystem side effects.

use std::path::PathBuf;
use std::process::Stdio;

use serde::{Deserialize, Serialize};
use tokio::io::AsyncWriteExt;
use tokio::process::Command;
use tracing::{debug, info, warn};

use crate::task_graph::{TaskNode, TaskResult};

/// JSON payload sent to `agents/file_task.py` on stdin.
#[derive(Debug, Serialize)]
struct FileTaskRequest {
    action: String,
    target: String,
    grant_token: String,
    trace_id: String,
}

/// JSON payload read from `agents/file_task.py` stdout.
#[derive(Debug, Deserialize)]
struct FileTaskResponse {
    success: bool,
    message: String,
    #[serde(default)]
    hal_status: Option<String>,
    #[serde(default)]
    hal_risk_score: Option<f64>,
}

/// Resolve the repo `agents/` directory for subprocess invocation.
fn agents_dir() -> PathBuf {
    if let Ok(dir) = std::env::var("COGNOS_AGENTS_DIR") {
        return PathBuf::from(dir);
    }
    // Walk up from the executable to find agents/ (dev builds).
    if let Ok(exe) = std::env::current_exe() {
        let mut dir = exe.parent().map(PathBuf::from).unwrap_or_default();
        for _ in 0..6 {
            let candidate = dir.join("agents");
            if candidate.join("file_task.py").is_file() {
                return candidate;
            }
            if !dir.pop() {
                break;
            }
        }
    }
    PathBuf::from("agents")
}

/// Execute a task node by delegating to the Python file agent subprocess.
pub async fn execute_node(
    node: &TaskNode,
    grant_token: &str,
    trace_id: &str,
) -> TaskResult {
    let action = node
        .intent
        .get("action")
        .and_then(|v| v.as_str())
        .unwrap_or("execute")
        .to_string();
    let target = node
        .intent
        .get("target")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();

    debug!(%action, %target, agent = %node.agent.0, "executing task via file_task.py");

    let exec_started = std::time::Instant::now();
    let result = if action == "create_dir" || action == "create_file" || action.starts_with("file.") {
        run_file_task(&action, &target, grant_token, trace_id).await
    } else {
        TaskResult {
            output: serde_json::json!({
                "action": action,
                "target": target,
                "status": "skipped",
                "reason": "no executor for action",
            }),
            error: None,
        }
    };
    let exec_ms = exec_started.elapsed().as_millis() as u64;
    info!(
        trace_id = %trace_id,
        stage = "execution",
        latency_ms = exec_ms,
        action = %action,
        target = %target,
        success = result.error.is_none(),
        "pipeline stage"
    );
    result
}

async fn run_file_task(
    action: &str,
    target: &str,
    grant_token: &str,
    trace_id: &str,
) -> TaskResult {
    let agents = agents_dir();
    let script = agents.join("file_task.py");
    if !script.is_file() {
        return TaskResult {
            output: serde_json::Value::Null,
            error: Some(format!("file_task.py not found at {}", script.display())),
        };
    }

    let python = std::env::var("COGNOS_PYTHON").unwrap_or_else(|_| "python3".to_string());
    let req = FileTaskRequest {
        action: action.to_string(),
        target: target.to_string(),
        grant_token: grant_token.to_string(),
        trace_id: trace_id.to_string(),
    };
    let payload = match serde_json::to_string(&req) {
        Ok(p) => p,
        Err(e) => {
            return TaskResult {
                output: serde_json::Value::Null,
                error: Some(format!("serialize file task request: {e}")),
            };
        }
    };

    let mut child = match Command::new(&python)
        .arg(&script)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .current_dir(&agents)
        .env("PYTHONPATH", &agents)
        .spawn()
    {
        Ok(c) => c,
        Err(e) => {
            return TaskResult {
                output: serde_json::Value::Null,
                error: Some(format!("spawn file_task.py: {e}")),
            };
        }
    };

    if let Some(mut stdin) = child.stdin.take() {
        if let Err(e) = stdin.write_all(payload.as_bytes()).await {
            warn!(error = %e, "failed to write file_task stdin");
        }
    }

    let output = match child.wait_with_output().await {
        Ok(o) => o,
        Err(e) => {
            return TaskResult {
                output: serde_json::Value::Null,
                error: Some(format!("file_task.py wait failed: {e}")),
            };
        }
    };

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return TaskResult {
            output: serde_json::Value::Null,
            error: Some(format!(
                "file_task.py exited {}: {}",
                output.status, stderr
            )),
        };
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    match serde_json::from_str::<FileTaskResponse>(stdout.trim()) {
        Ok(resp) => {
            if resp.success {
                TaskResult {
                    output: serde_json::json!({
                        "message": resp.message,
                        "hal_status": resp.hal_status,
                        "hal_risk_score": resp.hal_risk_score,
                    }),
                    error: None,
                }
            } else {
                TaskResult {
                    output: serde_json::json!({ "message": resp.message }),
                    error: Some(resp.message),
                }
            }
        }
        Err(e) => TaskResult {
            output: serde_json::Value::Null,
            error: Some(format!("parse file_task output: {e}; stdout={stdout}")),
        },
    }
}
