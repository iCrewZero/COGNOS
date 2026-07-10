"""COGNOS IPC authentication — Python side.

Exact port of the Rust HMAC-SHA256 auth in `ipc/grpc/src/auth.rs`
(`create_token` / `verify_token`) and `ipc/grpc/src/client.rs`
(`build_envelope`). A token minted here MUST verify with the Rust
`auth::verify_token`, and vice-versa — see `ipc/grpc/tests/cross_auth.rs`.

Crypto is stdlib only (`hmac` + `hashlib`); no home-grown primitives.
Signature comparison is constant-time (`hmac.compare_digest`).

── Session token (auth.rs::create_token) ──────────────────────────────────
  message = agent_id + "|" + str(expiry)          # '|' separator, bytes
  sig     = HMAC-SHA256(secret, message)
  token   = b64url_nopad(agent_id) + "." +
            b64url_nopad(str(expiry)) + "." +
            b64url_nopad(sig)
  base64  = URL-safe, NO padding (Rust base64 URL_SAFE_NO_PAD)
  expiry  = Unix epoch **seconds**

── Envelope signature (client.rs::build_envelope) ─────────────────────────
  message = trace_id|source|target|capability|payload   # '|' separators
  sig     = HMAC-SHA256(secret, message)                # raw bytes
"""
from __future__ import annotations

import base64
import hashlib
import hmac
import os
import time

# Same resolution source both sides must agree on. Rust currently injects the
# secret via ClientConfig.signing_secret; deployments should populate that from
# this env var so both ends share one key.
SECRET_ENV_VAR = "COGNOS_IPC_SECRET"

# Default session-token lifetime in seconds.
DEFAULT_TOKEN_TTL_S = 3600


class AuthError(Exception):
    """Base class for token verification failures."""


class InvalidToken(AuthError):
    """Token is malformed (wrong number of parts, bad base64, non-int expiry)."""


class UnknownAgent(AuthError):
    """Token's embedded agent id does not match the expected agent."""


class TokenExpired(AuthError):
    """Token's expiry is in the past."""


class SignatureMismatch(AuthError):
    """HMAC signature does not match."""


def _as_bytes(secret) -> bytes:
    if isinstance(secret, (bytes, bytearray)):
        return bytes(secret)
    return str(secret).encode("utf-8")


def _b64url_nopad_encode(raw: bytes) -> str:
    """base64 URL-safe, no padding — mirrors Rust URL_SAFE_NO_PAD."""
    return base64.urlsafe_b64encode(raw).rstrip(b"=").decode("ascii")


def _b64url_nopad_decode(text: str) -> bytes:
    """Inverse of `_b64url_nopad_encode`; restores stripped '=' padding."""
    pad = (-len(text)) % 4
    return base64.urlsafe_b64decode(text + ("=" * pad))


def resolve_secret(explicit=None) -> bytes:
    """Resolve the shared HMAC secret.

    Precedence: explicit argument > $COGNOS_IPC_SECRET > empty (matches the
    Rust ClientConfig default of an empty signing_secret).
    """
    if explicit is not None and explicit != "":
        return _as_bytes(explicit)
    env = os.environ.get(SECRET_ENV_VAR)
    if env:
        return env.encode("utf-8")
    return b""


def create_token(agent_id: str, expiry: int, secret) -> str:
    """Mint an HMAC-SHA256 session token identical to Rust
    `auth::create_token(agent_id, expiry, secret)`.

    Args:
        agent_id: logical agent id, e.g. "agent.coordinator".
        expiry:   Unix epoch **seconds** after which the token is invalid.
        secret:   shared key (bytes or str).
    """
    secret = _as_bytes(secret)
    expiry_str = str(int(expiry))

    message = agent_id.encode("utf-8") + b"|" + expiry_str.encode("utf-8")
    sig = hmac.new(secret, message, hashlib.sha256).digest()

    agent_b64 = _b64url_nopad_encode(agent_id.encode("utf-8"))
    expiry_b64 = _b64url_nopad_encode(expiry_str.encode("utf-8"))
    sig_b64 = _b64url_nopad_encode(sig)
    return f"{agent_b64}.{expiry_b64}.{sig_b64}"


def verify_token(token: str, expected_agent: str, secret) -> dict:
    """Verify a session token, mirroring Rust `auth::verify_token`.

    Returns a dict {agent_id, expiry, token} on success. Raises the matching
    AuthError subclass on failure. Signature check is constant-time.
    """
    secret = _as_bytes(secret)

    parts = token.split(".")
    if len(parts) != 3:
        raise InvalidToken("token did not have 3 dot-separated parts")

    try:
        agent_id = _b64url_nopad_decode(parts[0]).decode("utf-8")
        expiry_str = _b64url_nopad_decode(parts[1]).decode("utf-8")
        supplied_sig = _b64url_nopad_decode(parts[2])
    except (ValueError, UnicodeDecodeError) as exc:
        raise InvalidToken(f"base64/utf-8 decode failed: {exc}") from exc

    # Agent match is checked before expiry (same order as the Rust code).
    if agent_id != expected_agent:
        raise UnknownAgent(agent_id)

    try:
        expiry = int(expiry_str)
    except ValueError as exc:
        raise InvalidToken(f"expiry is not an integer: {expiry_str!r}") from exc

    now = int(time.time())
    if expiry <= now:
        raise TokenExpired(f"expiry={expiry} <= now={now}")

    message = agent_id.encode("utf-8") + b"|" + str(expiry).encode("utf-8")
    expected_sig = hmac.new(secret, message, hashlib.sha256).digest()
    if not hmac.compare_digest(supplied_sig, expected_sig):
        raise SignatureMismatch("HMAC signature mismatch")

    return {"agent_id": agent_id, "expiry": expiry, "token": token}


def sign_envelope(
    secret,
    trace_id: str,
    source: str,
    target: str,
    capability: str,
    payload: bytes,
) -> bytes:
    """Compute the Envelope HMAC exactly like Rust `client.rs::build_envelope`.

    message = trace_id|source|target|capability|payload  ('|' separated)
    Returns the raw 32-byte HMAC-SHA256 digest (as stored in Envelope.signature).
    """
    secret = _as_bytes(secret)
    mac = hmac.new(secret, digestmod=hashlib.sha256)
    mac.update(trace_id.encode("utf-8"))
    mac.update(b"|")
    mac.update(source.encode("utf-8"))
    mac.update(b"|")
    mac.update(target.encode("utf-8"))
    mac.update(b"|")
    mac.update(capability.encode("utf-8"))
    mac.update(b"|")
    mac.update(payload if isinstance(payload, (bytes, bytearray)) else str(payload).encode("utf-8"))
    return mac.digest()
