"""Exercise stage 2 binary compatibility against the stage 3 candidate on Linux."""
import argparse
import json
import os
from pathlib import Path
import shlex
import struct
import subprocess
import tempfile

p = argparse.ArgumentParser()
p.add_argument("--old", required=True)
p.add_argument("--new", required=True)
p.add_argument("--quiescent", action="store_true")
args = p.parse_args()
old, new = str(Path(args.old).resolve()), str(Path(args.new).resolve())
root = Path(tempfile.mkdtemp(prefix="myvcs-stage3-compat-"))


def run(binary, *command, env=None, success=True):
    result = subprocess.run([binary, *map(str, command)], cwd=root, env=env, capture_output=True, text=True, timeout=90)
    assert result.returncode == 0 if success else result.returncode != 0, (command, result.stdout, result.stderr)
    return result


run(old, "init", "--bare", "server")
run(old, "init", "--empty", "legacy")
(root / "legacy/f").write_text("old binary content\n")
run(old, "--repo", "legacy", "add", "f")
run(old, "--repo", "legacy", "commit", "-m", "legacy")
run(old, "--repo", "legacy", "remote", "add", "origin", "file://" + str(root / "server"))
run(old, "--repo", "legacy", "push")
for mode in ("full", "internal", "thin"):
    # Force a true old service subprocess while using the new client.
    wrapper = root / "old-server-ssh"
    wrapper.write_text("#!/bin/sh\nexec " + shlex.quote(old) + " serve --root " + shlex.quote(str(root)) + " --repository server\n")
    wrapper.chmod(0o700)
    env = dict(os.environ, MYVCS_SSH=str(wrapper), MYVCS_PACK_MODE=mode)
    run(new, "clone", "ssh://compat/server", "new-" + mode, env=env)
    assert (root / ("new-"+mode) / "f").read_text() == "old binary content\n"
    run(new, "--repo", "new-" + mode, "verify")
print("PASS stage3 client -> stage2 service: full/internal/thin legacy operations")

upgrade_args = ["--quiescent"] if args.quiescent else []
run(new, "--repo", "server", "upgrade", *upgrade_args)
run(new, "--repo", "server", "verify")
rejected = run(old, "--repo", "server", "verify", success=False)
assert "unsupported repository format" in rejected.stderr
print("PASS stage2 binary rejects upgraded repository; stage3 verifies unchanged history")

wrapper = root / "new-server-ssh"
wrapper.write_text("#!/bin/sh\nexec " + shlex.quote(new) + " serve --root " + shlex.quote(str(root)) + " --repository server\n")
wrapper.chmod(0o700)
env = dict(os.environ, MYVCS_SSH=str(wrapper), MYVCS_PACK_MODE="full")
run(old, "clone", "ssh://compat/server", "old-network-client", env=env)
assert (root / "old-network-client/f").read_text() == "old binary content\n"
(root / "old-network-client/f").write_text("old client through new server\n")
run(old, "--repo", "old-network-client", "add", "f")
run(old, "--repo", "old-network-client", "commit", "-m", "legacy network")
run(old, "--repo", "old-network-client", "push", env=env)
run(new, "--repo", "server", "verify")
print("PASS stage2 client -> stage3 service on upgraded repo: clone and push")

# A real old client must obey policy on the upgraded new service too.
policy = {"version": 1, "users": {
    "reader": {"read": True, "write": [], "force": [], "delete": []},
    "writer": {"read": True, "write": ["refs/heads/*"], "force": [], "delete": []},
}}
(root / "server/policy.json").write_text(json.dumps(policy))
wrapper.write_text("#!/bin/sh\nexec env MYVCS_PRINCIPAL=reader " + shlex.quote(new) +
                   " serve --root " + shlex.quote(str(root)) + " --repository server\n")
# Simulated SSH wrapper supplies the trusted identity and replaces client env.
env["MYVCS_PRINCIPAL"] = "writer"
run(old, "clone", "ssh://compat/server", "old-reader", env=env)
(root / "old-reader/f").write_text("attempted unauthorized old-client write\n")
run(old, "--repo", "old-reader", "add", "f")
run(old, "--repo", "old-reader", "commit", "-m", "legacy unauthorized write")
before = json.loads((root / "server/refs-state.json").read_text())["refs"]
denied = run(old, "--repo", "old-reader", "push", env=env, success=False)
assert "denied" in denied.stderr.lower(), denied.stderr
assert json.loads((root / "server/refs-state.json").read_text())["refs"] == before
assert "reader" in run(new, "--repo", "server", "audit").stdout
print("PASS stage2 client cannot bypass new policy or spoof trusted service identity")

run(old, "init", "--bare", "old-protocol")
hello = json.dumps({"Hello": {"version": 2, "object_format": 1, "hash": "sha256", "pack": "full-zlib-v1"}}).encode()
result = subprocess.run([old, "serve", "--root", str(root), "--repository", "old-protocol"],
                        input=struct.pack(">I", len(hello))+hello, capture_output=True, timeout=10)
assert result.returncode != 0
size = struct.unpack(">I", result.stdout[:4])[0]
error = json.loads(result.stdout[4:4+size])
assert "Error" in error
print("PASS stage2 service explicitly rejects unsupported extended control protocol")
print("Compatibility workspace:", root)
