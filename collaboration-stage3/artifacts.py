"""Source snapshots and review diffs for this workspace without Git metadata."""
import argparse
import difflib
import hashlib
import json
import pathlib
import tarfile

folder = pathlib.Path(__file__).resolve().parent
root = folder.parent
parser = argparse.ArgumentParser()
parser.add_argument("action", choices=["snapshot", "diff", "final"])
parser.add_argument("--name", default="candidate-source")
parser.add_argument("--base", default="baseline-source.tar.gz")
args = parser.parse_args()
paths = [root / "Cargo.toml", root / "Cargo.lock", *sorted((root / "src").rglob("*.rs")),
         *sorted((root / "tests").rglob("*.rs"))]
gate = folder / "ssh-gate.py"
if gate.is_file():
    paths.append(gate)
if args.action == "snapshot":
    with tarfile.open(folder / (args.name + ".tar.gz"), "w:gz") as tar:
        for path in paths:
            tar.add(path, arcname=path.relative_to(root).as_posix())
    print("Snapshot:", args.name)
elif args.action == "diff":
    with tarfile.open(folder / args.base) as tar:
        before = {m.name: tar.extractfile(m).read().decode("utf-8").replace("\r\n", "\n")
                  for m in tar.getmembers() if m.isfile() and (m.name.startswith(("src/", "tests/")) or m.name in ("Cargo.toml", "Cargo.lock", "collaboration-stage3/ssh-gate.py"))}
    after = {p.relative_to(root).as_posix(): p.read_text(encoding="utf-8") for p in paths}
    diff = []
    for name in sorted(before.keys() | after.keys()):
        diff.extend(difflib.unified_diff(before.get(name, "").splitlines(True), after.get(name, "").splitlines(True),
                    fromfile="a/"+name if name in before else "/dev/null", tofile="b/"+name, n=10))
    (folder / (args.name + ".diff")).write_text("".join(diff), encoding="utf-8", newline="\n")
    print("Diff:", args.name)
else:
    with tarfile.open(folder / "candidate-source.tar.gz") as tar:
        expected = {p.relative_to(root).as_posix() for p in paths}
        actual = {m.name for m in tar.getmembers() if m.isfile()}
        assert actual == expected, {"missing": sorted(expected - actual), "extra": sorted(actual - expected)}
        manifest = {}
        for member in tar.getmembers():
            if member.isfile():
                data = tar.extractfile(member).read()
                assert data == (root / member.name).read_bytes(), member.name
                manifest[member.name] = hashlib.sha256(data).hexdigest()
    (folder / "source-manifest.json").write_text(json.dumps(manifest, indent=2)+"\n", encoding="utf-8")
    checks = [f"{hashlib.sha256(p.read_bytes()).hexdigest()}  {p.name}\n" for p in sorted(folder.iterdir())
              if p.is_file() and p.name != "SHA256SUMS.txt"]
    (folder / "SHA256SUMS.txt").write_text("".join(checks), encoding="utf-8", newline="\n")
    print("Verified snapshot and wrote manifest/checksums")
