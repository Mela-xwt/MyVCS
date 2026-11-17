#!/usr/bin/env python3
"""Linux add/pipeline comparison. Fresh repos per variant; deterministic data.

Measures add, commit and GC separately. Both binaries verify each repository
before/after GC and restore every historical file after GC. No cache dropping.
Successful temporary repos are removed; failures and all logs are retained.
"""
import argparse
import hashlib
import json
import os
from pathlib import Path
import platform
import shutil
import statistics
import subprocess
import sys
import tempfile

from compare_gc_versions import CASES, MIB, cleanup, digest, object_bytes, run, save


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--baseline", type=Path, required=True)
    parser.add_argument("--candidate", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--rounds", type=int, default=2)
    parser.add_argument("--cases", nargs="+", choices=CASES, default=list(CASES))
    parser.add_argument("--candidate-policy", choices=["auto", "fast", "default"], default="auto")
    parser.add_argument("--work-root", type=Path)
    args = parser.parse_args()
    if not sys.platform.startswith("linux") or args.rounds < 1:
        parser.error("Requires Linux and at least one round")
    binaries = {"v2": args.baseline.resolve(), "v3": args.candidate.resolve()}
    for binary in [*binaries.values(), Path("/usr/bin/time")]:
        if not binary.is_file() or not os.access(binary, os.X_OK):
            parser.error("Missing executable: " + str(binary))
    output = args.output.resolve()
    output.mkdir(parents=True, exist_ok=False)
    work = Path(tempfile.mkdtemp(prefix="myvcs-add-compare-", dir=args.work_root))
    timer = Path(__file__).resolve().with_name("measure_command.py")
    env = {k: v for k, v in os.environ.items() if not k.startswith("MYVCS_")}
    env["MYVCS_FORCE_DEEP_DELTA"] = "1"
    save(output / "manifest.json", {
        "platform": platform.platform(), "python": sys.version,
        "os_release": Path("/etc/os-release").read_text(),
        "binaries": {k: {"path": str(v), "sha256": digest(v)} for k, v in binaries.items()},
        "scripts": {p.name: digest(p) for p in [Path(__file__), timer,
                    Path(__file__).with_name("compare_gc_versions.py")]},
        "rounds": args.rounds, "cases": args.cases, "workspace": str(work),
        "candidate_policy": args.candidate_policy,
        "environment_overrides": {"MYVCS_FORCE_DEEP_DELTA": "1"},
        "method": "fresh repos, alternating order, warm cache, same synthetic SHAKE-256 inputs "
                  "as attempts 1/2; not formal-main01 original inputs; add/commit/gc timed separately",
    })
    records = []
    for case in args.cases:
        size, updates, patch_size = CASES[case]
        initial = work / (case + ".bin")
        initial.write_bytes(hashlib.shake_256(("myvcs-gc-v1-" + case).encode()).digest(size * MIB)
                            if case.startswith("random") else
                            b"A" * (size * MIB // 2) + b"B" * (size * MIB // 2))
        save(output / (case + "-input.json"), {
            "initial_sha256": digest(initial), "size_bytes": size * MIB,
            "patches": [{"offset": version * MIB, "size": patch_size,
                         "sha256": hashlib.sha256(hashlib.shake_256(
                             f"myvcs-gc-v1-patch-{size}-{version}".encode()).digest(patch_size)).hexdigest()}
                        for version in range(1, updates + 1)],
        })
        for trial in range(1, args.rounds + 1):
            order = list(binaries) if trial % 2 else list(reversed(binaries))
            for variant in order:
                label = f"{case}-{trial:02d}-{variant}"
                repo = work / label
                repo.mkdir()
                log = output / (label + "-validation.log")
                binary = binaries[variant]
                variant_env = dict(env)
                if variant == "v3":
                    variant_env["MYVCS_LARGE_FILE_COMPRESSION"] = args.candidate_policy
                run(binary, repo, ["init"], variant_env, log)
                shutil.copyfile(initial, repo / "data.bin")
                save(output / (label + "-system-before.json"),
                     {p: Path("/proc/" + p).read_text() for p in ("loadavg", "meminfo", "vmstat")})

                def measure(stage, command):
                    result = output / (label + "-" + stage + ".json")
                    subprocess.run([sys.executable, str(timer), "--output", str(result),
                                    "--label", label + "-" + stage, "--cwd", str(repo), "--",
                                    str(binary)] + command, env=variant_env, check=True,
                                   stdout=subprocess.DEVNULL)
                    return json.loads(result.read_text())

                versions, adds, commits = [], [], []
                for version in range(updates + 1):
                    if version:
                        patch = hashlib.shake_256(
                            f"myvcs-gc-v1-patch-{size}-{version}".encode()).digest(patch_size)
                        with (repo / "data.bin").open("r+b") as stream:
                            stream.seek(version * MIB)
                            stream.write(patch)
                    adds.append(measure(f"add-{version}", ["add", "data.bin"]))
                    commits.append(measure(f"commit-{version}", ["commit", "-m", f"version {version}"]))
                    oid = digest(repo / "data.bin")
                    versions.append({"commit": (repo / ".myvcs/refs/heads/main").read_text().strip(),
                                     "sha256": oid,
                                     "stored_bytes_before": (repo / ".myvcs/objects" / oid[:2] / oid[2:]).stat().st_size})
                before = object_bytes(repo)
                for reader in binaries.values():
                    run(reader, repo, ["verify"], env, log)
                gc = measure("gc", ["gc"])
                after = object_bytes(repo)
                save(output / (label + "-system-after.json"),
                     {p: Path("/proc/" + p).read_text() for p in ("loadavg", "meminfo", "vmstat")})
                for reader in binaries.values():
                    run(reader, repo, ["verify"], env, log)
                    for version in versions:
                        run(reader, repo, ["checkout", version["commit"]], env, log)
                        if digest(repo / "data.bin") != version["sha256"]:
                            raise RuntimeError("Restored content differs: " + label)
                    run(reader, repo, ["checkout", "main"], env, log)
                record = {"case": case, "trial": trial, "variant": variant, "order": order,
                          "add_total_ms": sum(r["elapsed_ms"] for r in adds),
                          "add_peak_rss_MiB": max(r["max_rss_KiB"] for r in adds) / 1024,
                          "commit_total_ms": sum(r["elapsed_ms"] for r in commits),
                          "gc_ms": gc["elapsed_ms"], "gc_peak_rss_MiB": gc["max_rss_KiB"] / 1024,
                          "blob_bytes_before": sum(v["stored_bytes_before"] for v in versions),
                          "blob_bytes_after": sum((repo / ".myvcs/objects" / v["sha256"][:2] / v["sha256"][2:]).stat().st_size for v in versions),
                          "object_bytes_before": before, "object_bytes_after": after,
                          "versions": versions, "restores_checked": 2 * len(versions), "verify_checks": 4}
                save(output / (label + "-validated.json"), record)
                records.append(record)
                print(f"{label}: add total {record['add_total_ms']:.2f} ms; "
                      f"blob bytes {record['blob_bytes_before']}; both readers verified", flush=True)
                cleanup(repo, work)
        initial.unlink()
    metrics = ["add_total_ms", "add_peak_rss_MiB", "commit_total_ms", "gc_ms", "gc_peak_rss_MiB",
               "blob_bytes_before", "blob_bytes_after", "object_bytes_before", "object_bytes_after"]
    summary = []
    for case in args.cases:
        for variant in binaries:
            rows = [r for r in records if r["case"] == case and r["variant"] == variant]
            summary.append({"case": case, "variant": variant, "rounds": len(rows),
                            **{k + "_mean": statistics.mean(r[k] for r in rows) for k in metrics},
                            "restores_checked": sum(r["restores_checked"] for r in rows),
                            "verify_checks": sum(r["verify_checks"] for r in rows)})
    save(output / "summary.json", summary)
    work.rmdir()
    print(json.dumps(summary, indent=2))


if __name__ == "__main__":
    main()
