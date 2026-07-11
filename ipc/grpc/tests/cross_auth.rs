//! Cross-language auth conformance test.
//!
//! Proves the Python token implementation (`agents/auth.py`) and the Rust
//! implementation (`ipc/grpc/src/auth.rs`) are byte-for-byte compatible in
//! both directions, anchored by a committed golden token
//! (`tests/fixtures/golden_token.txt`).
//!
//!  1. Rust reproduces the golden byte-for-byte (`create_token`).
//!  2. Rust verifies the golden (`verify_token`, constant-time).
//!  3. Python -> Rust: a Python-minted token verifies with Rust.
//!  4. Rust -> Python: a Rust-minted token is accepted by Python.
//!
//! The subprocess directions (3, 4) are skipped — not failed — when no Python
//! interpreter is present, so the golden-anchored directions (1, 2) still give
//! meaningful cross-language coverage in minimal build environments.

use std::path::{Path, PathBuf};
use std::process::Command;

use cognos_ipc_grpc::auth;

// These MUST match agents/tests/test_auth.py and the golden fixture inputs.
const AGENT: &str = "agent.coordinator";
const GOLDEN_EXPIRY: u64 = 4_102_444_800; // 2100-01-01 UTC — far future
const SECRET: &str = "cognos-cross-auth-test-secret";

fn fixtures_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
}

fn golden_token() -> String {
    std::fs::read_to_string(fixtures_dir().join("golden_token.txt"))
        .expect("read golden_token.txt")
        .trim()
        .to_string()
}

/// First runnable Python interpreter, preferring `python3` (CI/Unix) then
/// `python` (Windows). Returns `None` if neither is available.
fn python() -> Option<&'static str> {
    for cand in ["python3", "python"] {
        let ok = Command::new(cand)
            .arg("--version")
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false);
        if ok {
            return Some(cand);
        }
    }
    None
}

#[test]
fn rust_reproduces_golden_byte_for_byte() {
    let token = auth::create_token(AGENT, GOLDEN_EXPIRY, SECRET.as_bytes());
    assert_eq!(
        token,
        golden_token(),
        "Rust create_token must match the committed golden token"
    );
}

#[test]
fn rust_verifies_golden() {
    let ctx = auth::verify_token(&golden_token(), AGENT, SECRET.as_bytes())
        .expect("golden token must verify with the Rust validator");
    assert_eq!(ctx.agent_id, AGENT);
    assert_eq!(ctx.expiry, GOLDEN_EXPIRY);
}

#[test]
fn python_minted_token_verifies_in_rust() {
    let Some(py) = python() else {
        eprintln!("skipping python_minted_token_verifies_in_rust: no python interpreter");
        return;
    };

    let out = Command::new(py)
        .arg(fixtures_dir().join("gen_token.py"))
        .arg(AGENT)
        .arg(GOLDEN_EXPIRY.to_string())
        .arg(SECRET)
        .output()
        .expect("run gen_token.py");
    assert!(
        out.status.success(),
        "gen_token.py failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );

    let token = String::from_utf8(out.stdout).expect("token is utf-8");
    let token = token.trim();

    let ctx = auth::verify_token(token, AGENT, SECRET.as_bytes())
        .expect("python-minted token must verify in Rust");
    assert_eq!(ctx.agent_id, AGENT);
    assert_eq!(ctx.expiry, GOLDEN_EXPIRY);

    // Same (agent, expiry, secret) => identical bytes as the golden.
    assert_eq!(token, golden_token(), "python token must equal the golden");
}

#[test]
fn rust_minted_token_verifies_in_python() {
    let Some(py) = python() else {
        eprintln!("skipping rust_minted_token_verifies_in_python: no python interpreter");
        return;
    };

    let token = auth::create_token(AGENT, GOLDEN_EXPIRY, SECRET.as_bytes());
    let status = Command::new(py)
        .arg(fixtures_dir().join("verify_token.py"))
        .arg(&token)
        .arg(AGENT)
        .arg(SECRET)
        .status()
        .expect("run verify_token.py");
    assert!(
        status.success(),
        "Python must accept the Rust-minted token"
    );
}
