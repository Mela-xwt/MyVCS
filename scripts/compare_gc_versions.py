#!/usr/bin/env python3
"""Paired Linux GC comparison, using identical pre-GC repositories.

Inputs are deterministic synthetic fixtures, not the formal-main01 input files.
Only GC is timed. Every history version is restored and SHA-256 checked afterward.
Verified working copies are removed; logs, manifests and failed copies are kept.
"""
import argparse
import hashlib
import json
import os
from pathlib import Path
import platform
import re
import shutil
import statistics
import subprocess
import sys
import tempfile

MIB = 1024 * 1024
CASES = {"random20": (20, 5, 4096), "ab20": (20, 5, 4096),
         "random128": (128, 2, 65536), "ab128": (128, 2, 65536)}


def digest(path):
    result = hashlib.sha256()
    with Path(path).open("rb") as stream:
        for chunk in iter(lambda: stream.read(MIB), b""):
            result.update(chunk)
    return result.hexdigest()


def save(path, value):
    with path.open("x", encoding="utf-8") as stream:
        json.dump(value, stream, ensure_ascii=False, indent=2)
        stream.write("\n")


def run(binary, repo, arguments, env, log):
    with log.open("a", encoding="utf-8") as stream:
        stream.write("\n$ " + " ".join([str(binary)] + arguments) + "\n")
        stream.flush()
        subprocess.run([str(binary)] + arguments, cwd=repo, env=env,
                       stdout=stream, stderr=subprocess.STDOUT, check=True)


def object_bytes(repo):
    return sum(p.stat().st_size for p in (repo / ".myvcs/objects").rglob("*")
               if p.is_file())


def cleanup(path, work):
    if path.is_symlink() or path.resolve().parent != work.resolve():
        raise RuntimeError("Refusing cleanup outside benchmark workspace")
    shutil.rmtree(path)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--baseline", required=True, type=Path)
    parser.add_argument("--candidate", required=True, type=Path)
    parser.add_argument("--baseline-name", default="v0", choices=["v0", "v1", "v2", "baseline"])
    parser.add_argument("--candidate-name", default="v1", choices=["v1", "v2", "v3", "candidate"])
    parser.add_argument("--output", required=True, type=Path)
    parser.add_argument("--rounds", type=int, default=2)
    parser.add_argument("--cases", nargs="+", choices=CASES, default=list(CASES))
    parser.add_argument("--work-root", type=Path, help="Parent for fresh working directory")
    args = parser.parse_args()
    if not sys.platform.startswith("linux") or args.rounds < 1:
        parser.error("Requires Linux and at least one round")
    if args.baseline_name == args.candidate_name:
        parser.error("Baseline and candidate names must differ")
    binaries = {args.baseline_name: args.baseline.resolve(), args.candidate_name: args.candidate.resolve()}
    for path in binaries.values():
        if not path.is_file() or not os.access(path, os.X_OK):
            parser.error("Missing executable: " + str(path))
    if not os.access("/usr/bin/time", os.X_OK):
        parser.error("Missing /usr/bin/time")
    timer = Path(__file__).resolve().with_name("measure_command.py")
    output = args.output.resolve()
    output.mkdir(parents=True, exist_ok=False)
    work = Path(tempfile.mkdtemp(prefix="myvcs-gc-compare-", dir=args.work_root))
    env = {k: v for k, v in os.environ.items() if not k.startswith("MYVCS_")}
    env["MYVCS_FORCE_DEEP_DELTA"] = "1"
    save(output / "manifest.json", {
        "platform": platform.platform(), "python": sys.version,
        "os_release": Path("/etc/os-release").read_text(),
        "meminfo": Path("/proc/meminfo").read_text(),
        "binaries": {name: {"path": str(path), "sha256": digest(path)}
                     for name, path in binaries.items()},
        "script_sha256": digest(Path(__file__)), "timer_sha256": digest(timer),
        "rounds": args.rounds, "cases": args.cases, "workspace": str(work),
        "environment_overrides": {"MYVCS_FORCE_DEEP_DELTA": "1"},
        "method": "Identical template copies; alternating baseline/candidate order; warm caches; "
                  "only GC timed; synthetic SHAKE-256 inputs, not formal inputs",
    })
    records = []
    for case in args.cases:
        size, updates, patch_size = CASES[case]
        template = work / (case + "-template")
        template.mkdir()
        log = output / (case + "-setup.log")
        run(binaries[args.baseline_name], template, ["init"], env, log)
        if case.startswith("random"):
            initial = hashlib.shake_256(("myvcs-gc-v1-" + case).encode()).digest(size * MIB)
        else:
            initial = b"A" * (size * MIB // 2) + b"B" * (size * MIB // 2)
        data = template / "data.bin"
        data.write_bytes(initial)
        del initial
        versions = []
        patches = []
        for version in range(updates + 1):
            if version:
                # Same patch for random/A-B cases of the same size.
                patch = hashlib.shake_256(f"myvcs-gc-v1-patch-{size}-{version}".encode()).digest(patch_size)
                with data.open("r+b") as stream:
                    stream.seek(version * MIB)
                    stream.write(patch)
                patches.append({"offset": version * MIB, "size": patch_size,
                                "sha256": hashlib.sha256(patch).hexdigest()})
            run(binaries[args.baseline_name], template, ["add", "data.bin"], env, log)
            run(binaries[args.baseline_name], template, ["commit", "-m", f"version {version}"], env, log)
            versions.append({"commit": (template / ".myvcs/refs/heads/main").read_text().strip(),
                             "sha256": digest(data)})
        before = object_bytes(template)
        save(output / (case + "-input.json"), {"versions": versions, "patches": patches,
                                               "object_bytes_before": before})
        for trial in range(1, args.rounds + 1):
            order = list(binaries) if trial % 2 else list(reversed(binaries))
            for name in order:
                label = f"{case}-{trial:02d}-{name}"
                repo = work / label
                shutil.copytree(template, repo)
                if object_bytes(repo) != before:
                    raise RuntimeError("Repository copy size mismatch")
                measurement = output / (label + ".json")
                save(output / (label + "-system-before.json"),
                     {p: Path("/proc/" + p).read_text() for p in ("loadavg", "meminfo", "vmstat")})
                subprocess.run([sys.executable, str(timer), "--output", str(measurement),
                                "--label", label, "--cwd", str(repo), "--",
                                str(binaries[name]), "gc"], env=env, check=True,
                               stdout=subprocess.DEVNULL)
                save(output / (label + "-system-after.json"),
                     {p: Path("/proc/" + p).read_text() for p in ("loadavg", "meminfo", "vmstat")})
                record = json.loads(measurement.read_text())
                gc_log = measurement.with_suffix(".log").read_text()
                match = re.search(r"optimized (\d+) blob\(s\)", gc_log)
                if not match or int(match.group(1)) != updates:
                    raise RuntimeError("Unexpected GC optimization count: " + label)
                record.update(case=case, trial=trial, variant=name, order=order,
                              object_bytes_before=before, object_bytes_after=object_bytes(repo),
                              optimized_blobs=int(match.group(1)))
                validation_log = output / (label + "-validation.log")
                # Both readers must understand the resulting objects.
                for reader in binaries.values():
                    run(reader, repo, ["verify"], env, validation_log)
                restored = []
                for version in versions:
                    run(binaries[name], repo, ["checkout", version["commit"]], env, validation_log)
                    actual = digest(repo / "data.bin")
                    if actual != version["sha256"]:
                        raise RuntimeError("Restored content differs: " + label)
                    restored.append({"commit": version["commit"], "sha256": actual})
                run(binaries[name], repo, ["checkout", "main"], env, validation_log)
                record["restored_versions"] = restored
                save(output / (label + "-validated.json"), record)
                records.append(record)
                print(f"{label}: {record['elapsed_ms']:.2f} ms, "
                      f"RSS {record['max_rss_KiB'] / 1024:.2f} MiB; history verified", flush=True)
                cleanup(repo, work)
        cleanup(template, work)
    summary = []
    for case in args.cases:
        for name in binaries:
            rows = [r for r in records if r["case"] == case and r["variant"] == name]
            summary.append({"case": case, "variant": name, "rounds": len(rows),
                            "gc_mean_ms": statistics.mean(r["elapsed_ms"] for r in rows),
                            "gc_median_ms": statistics.median(r["elapsed_ms"] for r in rows),
                            "rss_mean_MiB": statistics.mean(r["max_rss_KiB"] / 1024 for r in rows),
                            "object_bytes_after": sorted(set(r["object_bytes_after"] for r in rows))})
    save(output / "summary.json", summary)
    work.rmdir()
    print(json.dumps(summary, indent=2))


if __name__ == "__main__":
    main()
