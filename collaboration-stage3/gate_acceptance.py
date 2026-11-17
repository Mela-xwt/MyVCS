"""Focused Linux forced-command gate validation with a harmless fixed program."""
import argparse
import json
import os
from pathlib import Path
import subprocess
import tempfile

parser = argparse.ArgumentParser()
parser.add_argument("--gate", default=str(Path(__file__).with_name("ssh-gate.py")))
args = parser.parse_args()
gate = Path(args.gate).resolve()
root = Path(tempfile.mkdtemp(prefix="myvcs-stage3-gate-"))
repos = root / "repositories"
repos.mkdir()
(repos / "team").mkdir()
outside = root / "outside"
outside.mkdir()
(repos / "escape").symlink_to(outside, target_is_directory=True)
stub = root / "fixed-program"
stub.write_text("#!/usr/bin/python3 -I\nimport json,os,sys\nprint(json.dumps({'argv':sys.argv[1:],'env':dict(os.environ)}))\n")
stub.chmod(0o700)


def invoke(command):
    # Controlled values only: a failed sanitization test cannot print user secrets.
    env = {"PATH": "/usr/bin:/bin", "SSH_ORIGINAL_COMMAND": command,
           "MYVCS_PRINCIPAL": "mallory", "MYVCS_REMOTE_PROGRAM": "untrusted",
           "MYVCS_SSH": "untrusted", "MYVCS_PACK_MODE": "untrusted",
           "PYTHONPATH": "/untrusted", "LD_LIBRARY_PATH": "/untrusted"}
    return subprocess.run(["/usr/bin/python3", "-I", str(gate), "--root", str(repos),
                           "--program", str(stub), "--principal", "alice"],
                          env=env, capture_output=True, text=True, timeout=10)


valid = invoke("myvcs serve --repository team")
assert valid.returncode == 0, valid.stderr
result = json.loads(valid.stdout)
assert result["env"]["MYVCS_PRINCIPAL"] == "alice", result
for key in ("MYVCS_REMOTE_PROGRAM", "MYVCS_SSH", "MYVCS_PACK_MODE", "PYTHONPATH", "LD_LIBRARY_PATH", "SSH_ORIGINAL_COMMAND"):
    assert key not in result["env"], (key, result)
assert "serve" in result["argv"] and str(repos) in result["argv"], result
print("PASS fixed principal, executable, repository root and sanitized service environment")
for command in ("echo forged", "myvcs serve --repository team --principal alice",
                "myvcs serve --repository ../outside", "myvcs serve --repository escape",
                "myvcs serve --repository /etc", "myvcs serve --repository team; id",
                "MYVCS_PRINCIPAL=alice myvcs serve --repository team", ""):
    result = invoke(command)
    assert result.returncode != 0, (command, result.stdout)
    assert not result.stdout.strip(), (command, result.stdout)
print("PASS malformed commands, traversal, symlink escape and identity injection rejected")
print("Gate acceptance workspace:", root)
