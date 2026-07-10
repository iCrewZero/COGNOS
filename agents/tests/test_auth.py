"""Unit tests for agents/auth.py — the Python side of the COGNOS IPC HMAC auth.

Proves the token scheme matches Rust `ipc/grpc/src/auth.rs` byte-for-byte
(via the committed golden token) and that verification enforces agent,
expiry, and signature. Cross-language verification lives in the Rust test
`ipc/grpc/tests/cross_auth.rs`.
"""
import base64
import hashlib
import hmac
import os
import time

import pytest

import auth

# Fixed vector — MUST equal the values used to generate the golden token
# (see ipc/grpc/tests/fixtures/golden_token.txt).
AGENT = "agent.coordinator"
EXPIRY = 4102444800  # 2100-01-01 UTC — far future, never expired in tests
SECRET = "cognos-cross-auth-test-secret"

_ROOT = os.path.abspath(os.path.join(os.path.dirname(__file__), "..", ".."))
GOLDEN_PATH = os.path.join(_ROOT, "ipc", "grpc", "tests", "fixtures", "golden_token.txt")


def _read_golden() -> str:
    with open(GOLDEN_PATH, "r", encoding="ascii") as f:
        return f.read().strip()


def test_create_token_matches_golden():
    assert auth.create_token(AGENT, EXPIRY, SECRET) == _read_golden()


def test_token_is_urlsafe_no_padding():
    token = auth.create_token(AGENT, EXPIRY, SECRET)
    assert token.count(".") == 2
    for ch in ("+", "/", "="):
        assert ch not in token, f"token must be URL-safe/no-pad, found {ch!r}"


def test_message_construction_is_pipe_separated():
    """Independently recompute the HMAC over `agent|expiry` and compare with
    the signature embedded in the token — locks the exact signed message."""
    token = auth.create_token(AGENT, EXPIRY, SECRET)
    sig_b64 = token.split(".")[2]
    embedded_sig = auth._b64url_nopad_decode(sig_b64)

    message = AGENT.encode() + b"|" + str(EXPIRY).encode()
    expected = hmac.new(SECRET.encode(), message, hashlib.sha256).digest()
    assert embedded_sig == expected


def test_roundtrip_verify_ok():
    expiry = int(time.time()) + 600
    token = auth.create_token(AGENT, expiry, SECRET)
    ctx = auth.verify_token(token, AGENT, SECRET)
    assert ctx["agent_id"] == AGENT
    assert ctx["expiry"] == expiry


def test_golden_token_verifies():
    ctx = auth.verify_token(_read_golden(), AGENT, SECRET)
    assert ctx["agent_id"] == AGENT
    assert ctx["expiry"] == EXPIRY


def test_wrong_secret_rejected():
    token = auth.create_token(AGENT, int(time.time()) + 600, SECRET)
    with pytest.raises(auth.SignatureMismatch):
        auth.verify_token(token, AGENT, "the-wrong-secret")


def test_wrong_agent_rejected():
    token = auth.create_token("agent.evil", int(time.time()) + 600, SECRET)
    with pytest.raises(auth.UnknownAgent):
        auth.verify_token(token, AGENT, SECRET)


def test_expired_token_rejected():
    token = auth.create_token(AGENT, int(time.time()) - 10, SECRET)
    with pytest.raises(auth.TokenExpired):
        auth.verify_token(token, AGENT, SECRET)


def test_malformed_token_rejected():
    with pytest.raises(auth.InvalidToken):
        auth.verify_token("only.two", AGENT, SECRET)


def test_tampered_signature_rejected():
    token = auth.create_token(AGENT, int(time.time()) + 600, SECRET)
    head, _, sig_b64 = token.rpartition(".")
    sig = bytearray(auth._b64url_nopad_decode(sig_b64))
    sig[0] ^= 0x01  # flip one bit
    tampered = head + "." + auth._b64url_nopad_encode(bytes(sig))
    with pytest.raises(auth.SignatureMismatch):
        auth.verify_token(tampered, AGENT, SECRET)


def test_sign_envelope_matches_manual():
    trace_id, source, target, cap = "t-1", "agent.a", "hal.gate", "fs.block.write"
    payload = b'{"op":"write"}'
    got = auth.sign_envelope(SECRET, trace_id, source, target, cap, payload)

    msg = b"|".join([trace_id.encode(), source.encode(), target.encode(), cap.encode()]) + b"|" + payload
    expected = hmac.new(SECRET.encode(), msg, hashlib.sha256).digest()
    assert got == expected
    assert len(got) == 32  # SHA-256 digest size


def test_resolve_secret_precedence(monkeypatch):
    monkeypatch.setenv(auth.SECRET_ENV_VAR, "from-env")
    assert auth.resolve_secret("explicit") == b"explicit"   # explicit wins
    assert auth.resolve_secret(None) == b"from-env"          # env fallback
    monkeypatch.delenv(auth.SECRET_ENV_VAR, raising=False)
    assert auth.resolve_secret(None) == b""                  # empty default
