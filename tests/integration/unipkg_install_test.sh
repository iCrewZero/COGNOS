#!/usr/bin/env bash
# Test: unipkg install flow
# Owner: iCrewZero
#
# Verifies the install flow: trust check → HAL gate → execute.
# This is a shell test because unipkg is a Rust binary.

set -euo pipefail

echo "unipkg install test (v0 — structural check only)"

# Check that the security agent rejects untrusted sources
python3 -c "
from agents.security import SecurityAgent
import asyncio

async def test():
    agent = SecurityAgent()
    msg = type('AgentMessage', (), {
        'type': 'INSTALL_TRUST',
        'payload': {'package': 'test-pkg', 'source': 'unknown-ftp'},
        'sender': 'coordinator',
    })()
    result = await agent.handle_message(msg)
    assert result['approved'] == False, 'Should reject untrusted source'
    print('  PASS: untrusted source rejected')

    msg2 = type('AgentMessage', (), {
        'type': 'INSTALL_TRUST',
        'payload': {'package': 'firefox', 'source': 'apt'},
        'sender': 'coordinator',
    })()
    result2 = await agent.handle_message(msg2)
    assert result2['approved'] == True, 'Should accept apt source'
    print('  PASS: trusted source accepted'

asyncio.run(test())
"

echo "All unipkg install tests passed!"
