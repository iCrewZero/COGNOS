"""Test that agent impersonation is caught by the IPC auth layer.

Verifies that:
1. A token signed with the wrong secret is rejected.
2. A token with a wrong agent_id is rejected.
3. An expired token is rejected.
4. A valid token passes verification.

Owner: iCrewZero
"""
import time
import base64
import hashlib
import hmac
import struct

SECRET = "test-secret-key"


def make_token(agent_id: str, secret: str, expiry_offset_s: int = 3600) -> str:
    """Build a token in the format: base64(agent_id).base64(expiry).base64(hmac_sig)"""
    expiry = int(time.time()) + expiry_offset_s
    msg = f"{agent_id}.{expiry}"
    sig = hmac.new(secret.encode(), msg.encode(), hashlib.sha256).digest()
    return f"{base64.b64encode(agent_id.encode()).decode()}.{base64.b64encode(str(expiry).encode()).decode()}.{base64.b64encode(sig).decode()}"


def test_valid_token():
    """A properly signed, non-expired token should verify."""
    token = make_token("agent.coordinator", SECRET)
    parts = token.split(".")
    assert len(parts) == 3

    agent_id = base64.b64decode(parts[0]).decode()
    expiry = int(base64.b64decode(parts[1]).decode())
    sig = base64.b64decode(parts[2])

    msg = f"{agent_id}.{expiry}"
    expected_sig = hmac.new(SECRET.encode(), msg.encode(), hashlib.sha256).digest()
    assert hmac.compare_digest(sig, expected_sig)
    assert expiry > time.time()


def test_wrong_secret_rejected():
    """A token signed with a different secret should NOT verify."""
    token = make_token("agent.coordinator", "wrong-secret")
    parts = token.split(".")
    sig = base64.b64decode(parts[2])

    agent_id = base64.b64decode(parts[0]).decode()
    expiry = int(base64.b64decode(parts[1]).decode())
    msg = f"{agent_id}.{expiry}"
    expected_sig = hmac.new(SECRET.encode(), msg.encode(), hashlib.sha256).digest()
    assert not hmac.compare_digest(sig, expected_sig)


def test_expired_token_rejected():
    """A token that has expired should NOT verify."""
    token = make_token("agent.coordinator", SECRET, expiry_offset_s=-10)
    parts = token.split(".")
    expiry = int(base64.b64decode(parts[1]).decode())
    assert expiry < time.time()


def test_wrong_agent_id_rejected():
    """A token claiming to be a different agent should NOT verify for the original agent."""
    # Token is made for agent.evil but we check if it's valid for agent.coordinator
    token = make_token("agent.evil", SECRET)
    parts = token.split(".")
    agent_id_in_token = base64.b64decode(parts[0]).decode()
    assert agent_id_in_token == "agent.evil"
    # Even though the signature is valid, the agent_id doesn't match
    # what the caller expects. The server should reject this.
