"""Build/test a new isolated stage 3 deployment; credentials are input only."""
import argparse
import getpass
from pathlib import Path
import shlex
import sys
import paramiko

folder = Path(__file__).resolve().parent
parser = argparse.ArgumentParser()
parser.add_argument("--acceptance", action="store_true")
parser.add_argument("--inspect", action="store_true")
args = parser.parse_args()
client = paramiko.SSHClient()
client.load_host_keys(str(folder.parent / "collaboration-stage2/known_hosts"))
client.connect("192.168.220.129", username="oslab", password=getpass.getpass("SSH password: "),
               allow_agent=False, look_for_keys=False, timeout=15)


def run(command, log):
    _, stdout, _ = client.exec_command("{ " + command + "; } 2>&1")
    with (folder / log).open("w", encoding="utf-8") as out:
        for line in stdout:
            print(line, end="", flush=True)
            out.write(line)
            out.flush()
    return stdout.channel.recv_exit_status()


try:
    sftp = client.open_sftp()
    if args.acceptance or args.inspect:
        remote = (folder / "server-directory.txt").read_text().strip()
        assert remote.startswith("/home/oslab/os-lab/myvcs-stage3.")
    else:
        _, stdout, _ = client.exec_command("mktemp -d /home/oslab/os-lab/myvcs-stage3.XXXXXX")
        remote = stdout.read().decode().strip()
        assert remote.startswith("/home/oslab/os-lab/myvcs-stage3.")
        (folder / "server-directory.txt").write_text(remote + "\n")
        sftp.put(str(folder / "candidate-source.tar.gz"), remote + "/source.tar.gz")
    q = shlex.quote(remote)
    if args.inspect:
        sys.exit(run(f"cd {q} && sha256sum source.tar.gz target/release/myvcs && ./target/release/myvcs --help", "server-artifacts.log"))
    if args.acceptance:
        # Guard against validating scripts against an obsolete binary source tree.
        for path in [folder.parent / "Cargo.toml", folder.parent / "Cargo.lock", *sorted((folder.parent / "src").rglob("*.rs")), *sorted((folder.parent / "tests").rglob("*.rs")), folder / "ssh-gate.py"]:
            with sftp.file(remote + "/" + path.relative_to(folder.parent).as_posix(), "rb") as inp:
                assert inp.read() == path.read_bytes(), "deployed source differs: " + str(path)
        try:
            sftp.mkdir(remote + "/collaboration-stage3")
        except OSError:
            pass
        for name in ("isolated_sshd.py", "ssh_acceptance.py", "ssh-gate.py"):
            sftp.put(str(folder / name), remote + "/collaboration-stage3/" + name)
        code = run(f"cd {q} && python3 collaboration-stage3/ssh_acceptance.py --bin {q}/target/release/myvcs", "server-ssh-acceptance.log")
        (folder / "server-ssh-exit-code.txt").write_text(str(code) + "\n")
        sys.exit(code)
    code = run(f'cd {q} && tar -mxzf source.tar.gz && export PATH="$HOME/.cargo/bin:$PATH" && '
               'rustc --version && cargo test --locked --offline && cargo build --release --locked --offline', "server-build-tests.log")
    (folder / "server-build-exit-code.txt").write_text(str(code) + "\n")
    sys.exit(code)
finally:
    client.close()
