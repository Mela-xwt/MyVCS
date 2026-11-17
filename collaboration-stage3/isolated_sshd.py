"""Private loopback SSH server for forced-command identity acceptance tests.

Runs as the invoking account, uses only new temporary host/client keys and an
explicit authorized_keys file. Never edits ~/.ssh or the system sshd config.
"""
from pathlib import Path
import shlex
import socket
import subprocess
import time


class IsolatedSshd:
    def __init__(self, directory, forced_commands):
        self.root = Path(directory).resolve()
        self.keys = {}
        try:
            self._start(forced_commands)
        except BaseException:
            self.close()
            raise

    def _start(self, forced_commands):
        self.root.mkdir(mode=0o700)
        for name in ["host", *forced_commands]:
            path = self.root / name
            subprocess.run(["ssh-keygen", "-q", "-t", "ed25519", "-N", "", "-f", str(path)], check=True)
            self.keys[name] = path
        with socket.socket() as sock:
            sock.bind(("127.0.0.1", 0))
            self.port = sock.getsockname()[1]
        authorized = self.root / "authorized_keys"
        lines = []
        for principal, command in forced_commands.items():
            if any(ch in command for ch in '\n\r"\\'):
                raise ValueError("unsupported forced-command quoting")
            lines.append(f'restrict,command="{command}" ' + self.keys[principal].with_suffix(".pub").read_text())
        authorized.write_text("".join(lines))
        authorized.chmod(0o600)
        config = self.root / "sshd_config"
        config.write_text(f"Port {self.port}\nListenAddress 127.0.0.1\n"
                          f"HostKey {self.keys['host']}\nPidFile {self.root / 'sshd.pid'}\n"
                          f"AuthorizedKeysFile {authorized}\nStrictModes no\n"
                          "PasswordAuthentication no\nKbdInteractiveAuthentication no\nUsePAM no\n"
                          "PubkeyAuthentication yes\nPermitUserEnvironment no\nAllowTcpForwarding no\n"
                          "X11Forwarding no\nPermitTTY no\nLogLevel VERBOSE\n")
        self.log = (self.root / "sshd.log").open("wb")
        subprocess.run(["/usr/sbin/sshd", "-t", "-f", str(config)], check=True)
        self.process = subprocess.Popen(["/usr/sbin/sshd", "-D", "-e", "-f", str(config)],
                                        stdin=subprocess.DEVNULL, stdout=self.log, stderr=self.log)
        for _ in range(50):
            if self.process.poll() is not None:
                raise RuntimeError((self.root / "sshd.log").read_text())
            try:
                with socket.create_connection(("127.0.0.1", self.port), timeout=0.2):
                    break
            except OSError:
                time.sleep(0.1)
        else:
            raise RuntimeError("private sshd failed to listen")
        public = self.keys["host"].with_suffix(".pub").read_text().split()
        self.known_hosts = self.root / "known_hosts"
        self.known_hosts.write_text(f"[127.0.0.1]:{self.port} {' '.join(public[:2])}\n")

    def client(self, principal, command, **kwargs):
        return subprocess.run(["ssh", "-F", "/dev/null", "-T", "-p", str(self.port), "-i", str(self.keys[principal]),
                               "-o", "IdentitiesOnly=yes", "-o", "BatchMode=yes", "-o", "StrictHostKeyChecking=yes",
                               "-o", "UserKnownHostsFile="+str(self.known_hosts), "127.0.0.1", command],
                              stdout=subprocess.PIPE, stderr=subprocess.PIPE, timeout=15, **kwargs)

    def wrapper(self, principal):
        path = self.root / (principal + "-ssh")
        path.write_text("#!/bin/sh\nexec /usr/bin/ssh -F /dev/null -i " + shlex.quote(str(self.keys[principal])) +
                        " -o IdentitiesOnly=yes -o BatchMode=yes -o StrictHostKeyChecking=yes -o UserKnownHostsFile=" +
                        shlex.quote(str(self.known_hosts)) + ' "$@"\n')
        path.chmod(0o700)
        return str(path)

    def close(self):
        if hasattr(self, "process"):
            self.process.terminate()
            try:
                self.process.wait(timeout=5)
            except subprocess.TimeoutExpired:
                self.process.kill()
                self.process.wait()
        if hasattr(self, "log"):
            self.log.close()
        for key in self.keys.values():
            key.unlink(missing_ok=True)

    def __enter__(self):
        return self

    def __exit__(self, *args):
        self.close()


if __name__ == "__main__":
    import tempfile
    root = Path(tempfile.mkdtemp(prefix="myvcs-private-sshd-"))
    with IsolatedSshd(root / "ssh", {"alice": "/usr/bin/printf alice", "bob": "/usr/bin/printf bob"}) as server:
        for user in ("alice", "bob"):
            result = server.client(user, "ignored-command")
            assert result.returncode == 0, result.stderr.decode()
            assert result.stdout.decode() == user
        print("PASS isolated SSH key identity mapping without changing system login configuration")
