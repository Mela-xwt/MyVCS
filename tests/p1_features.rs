use myvcs::{
    model::{Commit, DeltaBlob},
    repo::Repo,
    util::{build_delta, compress_bytes, encode_delta_blob, hex_sha256},
};
use serde_json::Value;
use std::{
    fs,
    path::PathBuf,
    process::{Command, Output},
};
struct Lab(PathBuf);
impl Lab {
    fn new() -> Self {
        let p = std::env::temp_dir().join(format!(
            "p1-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&p).unwrap();
        let l = Self(p);
        l.ok(&["init", "--empty"]);
        l
    }
    fn run(&self, a: &[&str]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_myvcs"))
            .args(a)
            .current_dir(&self.0)
            .env("MYVCS_FORCE_DEEP_DELTA", "1")
            .output()
            .unwrap()
    }
    fn ok(&self, a: &[&str]) -> String {
        let o = self.run(a);
        assert!(
            o.status.success(),
            "{:?}: {} {}",
            a,
            String::from_utf8_lossy(&o.stdout),
            String::from_utf8_lossy(&o.stderr)
        );
        String::from_utf8(o.stdout).unwrap()
    }
    fn bad(&self, a: &[&str], s: &str) {
        let o = self.run(a);
        assert!(!o.status.success(), "unexpected success {:?}", a);
        assert!(
            String::from_utf8_lossy(&o.stderr).contains(s),
            "{:?}: {}",
            a,
            String::from_utf8_lossy(&o.stderr)
        );
    }
    fn write(&self, p: &str, s: &[u8]) {
        let f = self.0.join(p);
        fs::create_dir_all(f.parent().unwrap()).unwrap();
        fs::write(f, s).unwrap();
    }
    fn read(&self, p: &str) -> Vec<u8> {
        fs::read(self.0.join(p)).unwrap()
    }
    fn commit(&self, p: &str, s: &[u8]) -> String {
        self.write(p, s);
        self.ok(&["add", p]);
        self.ok(&["commit", "-m", "version"]);
        Repo::open(&self.0)
            .unwrap()
            .head_commit_id()
            .unwrap()
            .unwrap()
    }
    fn stats(&self) -> Value {
        serde_json::from_str(&self.ok(&["stats", "--json"])).unwrap()
    }
}
impl Drop for Lab {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
#[test]
fn ignore_consistent_negation_tracked_and_source() {
    let l = Lab::new();
    l.commit("tracked.log", b"old");
    l.write(
        ".myvcsignore",
        b"# comment\n*.log\ntarget/\n!important.log\n",
    );
    for n in [
        "ignored.log",
        "important.log",
        "target/bin/app",
        "src/good.txt",
    ] {
        l.write(n, b"data");
    }
    l.write("tracked.log", b"changed");
    let status = l.ok(&["status"]);
    assert!(!status.contains("ignored.log"));
    assert!(!status.contains("target/bin"));
    assert!(status.contains("tracked.log"));
    assert!(status.contains("important.log"));
    assert!(
        l.ok(&["check-ignore", "target/bin/app"])
            .contains(".myvcsignore:3\ttarget/")
    );
    l.bad(&["add", "ignored.log"], "ignored");
    l.ok(&["add", "."]);
    let d = l.ok(&["diff", "--staged"]);
    assert!(d.contains("src/good.txt"));
    assert!(d.contains("important.log"));
    assert!(!d.contains("diff --staged -- ignored.log"));
    l.bad(&["check-ignore", "../escape"], "invalid");
    l.bad(&["check-ignore", ".myvcs/HEAD"], "protected");
}
#[test]
fn ignore_root_rules_unicode_and_explicit_target() {
    let l = Lab::new();
    l.write(
        ".myvcsignore",
        b"/root.txt\n*.tmp\ncache/\n!cache/keep.txt\n",
    );
    for n in [
        "root.txt",
        "sub/root.txt",
        "sub/a.tmp",
        "cache/keep.txt",
        "cache/no.txt",
        "target/code",
        "中文.txt",
    ] {
        l.write(n, b"x");
    }
    l.ok(&["add", "."]);
    let d = l.ok(&["diff", "--staged"]);
    assert!(d.contains("sub/root.txt"));
    assert!(d.contains("cache/keep.txt"));
    assert!(d.contains("target/code"));
    assert!(d.contains("中文.txt"));
    assert!(!d.contains("diff --staged -- root.txt"));
    assert!(!d.contains("cache/no.txt"));
}
#[test]
fn staged_diff_and_restore_preserve_work_index_head() {
    let l = Lab::new();
    let first = l.commit("a", b"base\n");
    l.commit("b", b"other\n");
    l.write("a", b"staged\n");
    l.ok(&["add", "a"]);
    l.write("a", b"unstaged\n");
    let d = l.ok(&["diff", "--staged", "a"]);
    assert!(d.contains("+staged"));
    assert!(!d.contains("unstaged"));
    let head = Repo::open(&l.0).unwrap().head_commit_id().unwrap();
    let index = l.read(".myvcs/index.json");
    l.ok(&["restore", "--source", &first, "a"]);
    assert_eq!(l.read("a"), b"base\n");
    assert_eq!(l.read("b"), b"other\n");
    assert_eq!(index, l.read(".myvcs/index.json"));
    assert_eq!(head, Repo::open(&l.0).unwrap().head_commit_id().unwrap());
    l.ok(&["restore", "--staged", "a"]);
    assert!(l.ok(&["diff", "--staged"]).is_empty());
    l.write("new", b"new\n");
    l.ok(&["add", "new"]);
    l.ok(&["rm", "b"]);
    let d = l.ok(&["diff", "--staged"]);
    assert!(d.contains("+new"));
    assert!(d.contains("-other"));
    l.ok(&["restore", "--staged", "new"]);
    assert_eq!(l.read("new"), b"new\n");
}
#[test]
fn restore_rejects_untracked_and_unsafe_paths() {
    let l = Lab::new();
    let first = l.commit("a", b"old");
    l.ok(&["rm", "a"]);
    l.ok(&["commit", "-m", "delete"]);
    l.write("a", b"precious");
    l.bad(&["restore", "--source", &first, "a"], "untracked");
    assert_eq!(l.read("a"), b"precious");
    l.bad(&["restore", "../escape"], "invalid");
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink("a", l.0.join("link")).unwrap();
        l.bad(&["restore", "link"], "symlink");
    }
}
#[test]
fn restore_crash_continue_abort_and_new_edit_protection() {
    for phase in ["prepared", "file", "index"] {
        for abort in [false, true] {
            let l = Lab::new();
            l.commit("a", b"base");
            l.write("a", b"local");
            l.write(".myvcs/test-work-crash", phase.as_bytes());
            let o = l.run(&["restore", "a"]);
            assert_eq!(o.status.code(), Some(86));
            fs::remove_file(l.0.join(".myvcs/test-work-crash")).unwrap();
            l.bad(&["add", "a"], "recover-worktree");
            if abort {
                l.ok(&["recover-worktree", "--abort"]);
            } else {
                l.ok(&["recover-worktree"]);
            }
            assert_eq!(
                l.read("a"),
                if abort {
                    b"local".as_slice()
                } else {
                    b"base".as_slice()
                }
            );
            l.ok(&["verify"]);
        }
    }
    let l = Lab::new();
    l.commit("a", b"base");
    l.write("a", b"local");
    l.write(".myvcs/test-work-crash", b"file");
    l.run(&["restore", "a"]);
    fs::remove_file(l.0.join(".myvcs/test-work-crash")).unwrap();
    l.write("a", b"new edits");
    l.bad(&["recover-worktree"], "new edits");
    assert_eq!(l.read("a"), b"new edits");
}
#[test]
fn stash_roundtrip_exact_index_untracked_ignore_and_gc() {
    let l = Lab::new();
    l.commit("a", b"base");
    l.commit("gone", b"gone");
    l.write("a", b"staged");
    l.ok(&["add", "a"]);
    l.write("a", b"unstaged");
    l.ok(&["rm", "gone"]);
    l.write("new", b"new staged");
    l.ok(&["add", "new"]);
    l.write(".myvcsignore", b"*.log\n");
    l.write("scratch", b"untracked");
    l.write("skip.log", b"ignore");
    let index = l.read(".myvcs/index.json");
    l.ok(&["stash", "push", "--include-untracked", "-m", "snapshot"]);
    assert_eq!(l.read("a"), b"base");
    assert!(l.0.join("gone").exists());
    assert!(!l.0.join("scratch").exists());
    assert!(l.0.join("skip.log").exists());
    assert!(l.ok(&["stash", "show"]).contains("scratch"));
    l.ok(&["gc"]);
    l.ok(&["verify"]);
    l.ok(&["stash", "apply"]);
    assert_eq!(index, l.read(".myvcs/index.json"));
    assert_eq!(l.read("a"), b"unstaged");
    assert!(!l.0.join("gone").exists());
    assert_eq!(l.read("scratch"), b"untracked");
    assert_eq!(l.stats()["stashes"], 1);
}
#[test]
fn stash_pop_conflicts_drop_and_stable_ids() {
    let l = Lab::new();
    l.commit("a", b"base");
    l.write("a", b"one");
    l.ok(&["stash", "push", "-m", "first"]);
    let id = l
        .ok(&["stash", "list"])
        .split_whitespace()
        .nth(1)
        .unwrap()
        .to_owned();
    l.write("a", b"two");
    l.ok(&["stash", "push", "-m", "second"]);
    l.ok(&["stash", "drop", "0"]);
    assert!(l.ok(&["stash", "show", &id]).contains("first"));
    l.write("a", b"new edits");
    l.bad(&["stash", "pop"], "local changes");
    assert_eq!(l.stats()["stashes"], 1);
    l.ok(&["restore", "a"]);
    l.ok(&["stash", "pop"]);
    assert_eq!(l.read("a"), b"one");
    assert_eq!(l.stats()["stashes"], 0);
    l.ok(&["gc"]);
    l.ok(&["verify"]);
}
#[test]
fn stash_three_way_nonconflicting_and_conflicting_branch() {
    let l = Lab::new();
    l.commit("a", b"base");
    l.write("a", b"saved");
    l.ok(&["stash", "push"]);
    l.commit("other", b"new commit");
    l.ok(&["stash", "apply"]);
    assert_eq!(l.read("a"), b"saved");
    assert_eq!(l.read("other"), b"new commit");
    l.ok(&["restore", "a"]);
    l.commit("a", b"conflicting");
    l.bad(&["stash", "pop"], "stash conflict");
    assert_eq!(l.read("a"), b"conflicting");
    assert_eq!(l.stats()["stashes"], 1);
}
#[test]
fn stash_push_interruption_retains_snapshot_and_can_recover() {
    for phase in ["prepared", "file", "index"] {
        let l = Lab::new();
        l.commit("a", b"base");
        l.write("a", b"saved");
        l.write(".myvcs/test-work-crash", phase.as_bytes());
        assert_eq!(l.run(&["stash", "push"]).status.code(), Some(86));
        fs::remove_file(l.0.join(".myvcs/test-work-crash")).unwrap();
        l.ok(&["recover-worktree"]);
        l.ok(&["gc"]);
        l.ok(&["stash", "pop"]);
        assert_eq!(l.read("a"), b"saved");
        l.ok(&["verify"]);
    }
}
#[test]
fn stash_unborn_and_untracked_collision() {
    let l = Lab::new();
    l.write("a", b"staged");
    l.ok(&["add", "a"]);
    l.write("scratch", b"saved");
    l.ok(&["stash", "push", "--include-untracked"]);
    l.ok(&["gc"]);
    l.write("scratch", b"precious");
    l.bad(&["stash", "pop"], "untracked stash conflict");
    assert_eq!(l.read("scratch"), b"precious");
    fs::remove_file(l.0.join("scratch")).unwrap();
    l.ok(&["stash", "pop"]);
    l.ok(&["commit", "-m", "first"]);
    l.ok(&["verify"]);
}
#[test]
fn diagnostics_stats_are_readonly_and_count_namespaces() {
    let l = Lab::new();
    let before = l.read(".myvcs/index.json");
    let empty = l.stats();
    assert_eq!(empty["commits_reachable"], 0);
    assert_eq!(empty["objects_total"], 0);
    let d: Value = serde_json::from_str(&l.ok(&["doctor", "--json"])).unwrap();
    assert!(d["capabilities"]["etmem"].is_null());
    assert_eq!(d["system"]["architecture"], std::env::consts::ARCH);
    assert_eq!(before, l.read(".myvcs/index.json"));
    let id = l.commit("a", b"data");
    l.ok(&["branch", "topic"]);
    l.ok(&["tag", "v1"]);
    let r = Repo::open(&l.0).unwrap();
    r.update_ref("refs/remotes/origin/main", None, &id).unwrap();
    let stat = l.stats();
    assert_eq!(stat["local_branches"], 2);
    assert_eq!(stat["remote_branches"], 1);
    assert_eq!(stat["tags"], 1);
    assert_eq!(stat["objects_total"], stat["objects_reachable"]);
    l.ok(&["gc"]);
    assert_eq!(l.stats()["commits_reachable"], 1);
}

#[test]
fn staged_restore_limit_failure_preserves_index() {
    let l = Lab::new();
    let name = "x".repeat(100);
    let old = l.commit(&name, b"saved");
    l.ok(&["rm", &name]);
    l.ok(&["commit", "-m", "delete"]);
    for n in 0..10 {
        let other = format!("other-{n}-{}", "y".repeat(100));
        l.write(&other, b"pending");
        l.ok(&["add", &other]);
    }
    let before = l.read(".myvcs/index.json");
    let repo = Repo::open(&l.0)
        .unwrap()
        .with_read_limits(myvcs::repo::ReadLimits {
            metadata_bytes: before.len(),
            ..Default::default()
        })
        .unwrap();
    let error = repo
        .with_repo_lock(|r| r.restore_file(&name, Some(&old), true))
        .unwrap_err();
    assert!(error.to_string().contains("index size limit"), "{error:#}");
    assert_eq!(before, l.read(".myvcs/index.json"));
    assert!(!l.0.join(name).exists());
    l.ok(&["verify"]);
}
fn store(l: &Lab, oid: &str, bytes: &[u8]) {
    l.write(
        &format!(".myvcs/objects/{}/{}", &oid[..2], &oid[2..]),
        &compress_bytes(bytes, flate2::Compression::default()).unwrap(),
    );
}
#[test]
fn gc_new_chains_are_bounded_and_all_history_restores() {
    let l = Lab::new();
    let mut data: Vec<u8> = (0..16384)
        .map(|i| ((i * 73 + i / 251) % 256) as u8)
        .collect();
    let mut versions = Vec::new();
    for i in 0..20 {
        data[i * 100 + 7] ^= 55;
        let id = l.commit("data", &data);
        versions.push((id, hex_sha256(&data)));
    }
    l.ok(&["gc"]);
    l.ok(&["gc"]);
    l.ok(&["verify"]);
    let stat = l.stats();
    assert!(stat["reachable_delta_objects"].as_u64().unwrap() > 0);
    assert!(stat["max_reachable_chain_nodes"].as_u64().unwrap() <= 8);
    for (id, hash) in versions {
        l.ok(&["restore", "--source", &id, "data"]);
        assert_eq!(hex_sha256(&l.read("data")), hash);
    }
}
#[test]
fn legacy_deep_chain_readable_missing_and_corrupt_bases_fail() {
    let l = Lab::new();
    let mut bytes = vec![b'A'; 4096];
    let mut previous = hex_sha256(&bytes);
    store(&l, &previous, &bytes);
    let first = previous.clone();
    for i in 0..15 {
        let mut next = bytes.clone();
        next[i] = b'B';
        let id = hex_sha256(&next);
        let delta = DeltaBlob {
            base: previous,
            ops: build_delta(&bytes, &next),
            size: next.len(),
        };
        store(&l, &id, &encode_delta_blob(&delta).unwrap());
        previous = id;
        bytes = next;
    }
    let commit = Commit {
        tree: std::collections::BTreeMap::from([("deep".into(), previous.clone())]),
        parents: vec![],
        parent: None,
        message: "legacy".into(),
        author: "test".into(),
        timestamp: 0,
    };
    let raw = serde_json::to_vec(&commit).unwrap();
    let id = hex_sha256(&raw);
    store(&l, &id, &raw);
    Repo::open(&l.0)
        .unwrap()
        .update_ref("refs/heads/main", None, &id)
        .unwrap();
    l.ok(&["verify"]);
    l.ok(&["gc"]);
    assert_eq!(
        Repo::open(&l.0)
            .unwrap()
            .read_blob_resolved(&previous)
            .unwrap(),
        bytes
    );
    assert_eq!(l.stats()["max_reachable_chain_nodes"], 16);
    store(&l, &first, b"corrupt");
    l.bad(&["gc"], "hash");
    fs::remove_file(l.0.join(format!(".myvcs/objects/{}/{}", &first[..2], &first[2..]))).unwrap();
    assert!(!l.run(&["verify"]).status.success());
}
#[test]
fn stash_index_only_new_file_does_not_delete_recreated_untracked() {
    let l = Lab::new();
    l.commit("a", b"base");
    l.write("new", b"staged");
    l.ok(&["add", "new"]);
    fs::remove_file(l.0.join("new")).unwrap();
    l.ok(&["stash", "push"]);
    l.write("new", b"precious");
    l.bad(&["stash", "apply"], "untracked path");
    assert_eq!(l.read("new"), b"precious");
}
#[test]
fn stash_corrupt_object_and_pop_crash_keep_snapshot() {
    let l = Lab::new();
    l.commit("a", b"base");
    l.write("a", b"saved");
    l.ok(&["stash", "push"]);
    l.write(".myvcs/test-work-crash", b"file");
    assert_eq!(l.run(&["stash", "pop"]).status.code(), Some(86));
    fs::remove_file(l.0.join(".myvcs/test-work-crash")).unwrap();
    l.ok(&["recover-worktree", "--abort"]);
    assert_eq!(l.read("a"), b"base");
    assert_eq!(l.stats()["stashes"], 1);
    let oid = hex_sha256(b"saved");
    store(&l, &oid, b"broken");
    assert!(!l.run(&["stash", "apply"]).status.success());
    assert_eq!(l.read("a"), b"base");
    assert_eq!(
        Repo::open(&l.0)
            .unwrap()
            .list_refs()
            .unwrap()
            .keys()
            .filter(|n| n.starts_with("refs/pins/stash/"))
            .count(),
        1
    );
}
#[test]
fn team_stash_drop_retained_until_explicit_reflog_expiry() {
    let l = Lab::new();
    l.ok(&["upgrade", "--quiescent"]);
    l.commit("a", b"base");
    l.write("a", b"saved");
    l.ok(&["stash", "push"]);
    l.ok(&["gc"]);
    l.ok(&["stash", "apply"]);
    l.ok(&["restore", "a"]);
    l.ok(&["stash", "drop"]);
    let kept = l.stats()["objects_reachable"].as_u64().unwrap();
    l.ok(&["reflog", "expire", "--keep", "0"]);
    l.ok(&["gc"]);
    assert!(l.stats()["objects_total"].as_u64().unwrap() < kept);
    l.ok(&["verify"]);
}
