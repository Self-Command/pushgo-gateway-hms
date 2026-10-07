#!/usr/bin/env python3
"""Exercise the published binary's metadata and total-request boundaries."""
import json
import sys
import urllib.error
import urllib.request

base, limit = sys.argv[1], int(sys.argv[2])

def request(value, extra_body=""):
    payload = {"channel_id": "00000000000000000000000000",
               "password": "ci-smoke-password", "title": "Metadata boundary probe",
               "body": extra_body, "metadata": {"task_card": value}}
    raw = json.dumps(payload, ensure_ascii=False).encode()
    req = urllib.request.Request(base + "/message", raw,
        {"Authorization": "Bearer ci-smoke", "Content-Type": "application/json"})
    try:
        response = urllib.request.urlopen(req, timeout=15)
    except urllib.error.HTTPError as response:
        return response.code, response.read().decode()
    with response:
        return response.status, response.read().decode()

control_status, control = request("x")
assert control_status in (400, 404), (control_status, control)
assert "channel" in control.lower() and "metadata value is too long" not in control
probe_status, probe = request("测" * 512)  # Exactly 1536 UTF-8 bytes.
if limit == 512:
    assert probe_status == 400 and "maximum 512 UTF-8 bytes" in probe, (probe_status, probe)
else:
    assert probe_status == control_status and "channel" in probe.lower(), (probe_status, probe)
    assert "metadata value is too long" not in probe
status, error = request("x" * (limit + 1))
assert status == 400 and f"maximum {limit} UTF-8 bytes" in error, (status, error)
status, error = request("x", "x" * (33 * 1024))
assert status == 400 and "invalid_request_body" in error and "length limit exceeded" in error, (status, error)
print(json.dumps({"metadata_limit": limit, "probe_utf8_bytes": 1536,
                  "oversized_scalar_rejected": True, "total_request_limit": 32768,
                  "result": "passed"}))
