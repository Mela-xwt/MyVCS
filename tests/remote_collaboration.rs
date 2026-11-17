use myvcs::{model::Commit, repo::Repo, util::hex_sha256};
use serde_json::{Value, json};
use std::{
    fs,
    io::{Read, Write},
    path::PathBuf,
    process::{Child, Command, Output, Stdio},
    sync::atomic::{AtomicU64, Ordering},
};

static NEXT: AtomicU64 = AtomicU64::new(0);
struct Lab(PathBuf);
impl Lab {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "myvcs-remote-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&path).unwrap();
        Self(path)
    }
    fn output(&self, args: &[&str]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_myvcs"))
            .current_dir(&self.0)
            .args(args)
            .env("MYVCS_TRANSFER_TIMEOUT", "20")
            .env("MYVCS_FORCE_DEEP_DELTA", "1")
            .output()
            .unwrap()
    }
    fn run(&self, args: &[&str]) -> String {
        let out = self.output(args);
        assert!(
            out.status.success(),
            "{args:?}\nstdout={}\nstderr={}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8(out.stdout).unwrap()
    }
    fn fail(&self, args: &[&str], error: &str) {
        let out = self.output(args);
        assert!(!out.status.success(), "{args:?} unexpectedly succeeded");
        assert!(
            String::from_utf8_lossy(&out.stderr).contains(error),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
    }
    fn repo(&self, name: &str) -> Repo {
        Repo::open(self.0.join(name)).unwrap()
    }
    fn tip(&self, name: &str) -> Option<String> {
        self.repo(name).head_commit_id().unwrap()
    }
    fn url(&self) -> String {
        format!("file://{}", self.0.join("server").display())
    }
    fn clone_to(&self, name: &str) {
        self.run(&["clone", &self.url(), name]);
    }
    fn commit(&self, repo: &str, file: &str, bytes: &[u8], message: &str) {
        fs::write(self.0.join(repo).join(file), bytes).unwrap();
        self.run(&["--repo", repo, "add", file]);
        self.run(&["--repo", repo, "commit", "-m", message]);
    }
    fn setup(&self) {
        self.run(&["init", "--bare", "server"]);
        self.clone_to("a");
        self.commit("a", "shared", b"base\n", "base");
        self.run(&["--repo", "a", "push"]);
        self.clone_to("b");
    }
    fn service(&self, read_only: bool) -> Child {
        self.service_mode(read_only, "full-zlib-v1")
    }
    fn service_mode(&self, read_only: bool, mode: &str) -> Child {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_myvcs"));
        cmd.args(["serve", "--root"])
            .arg(&self.0)
            .args(["--repository", "server"]);
        if read_only {
            cmd.arg("--read-only");
        }
        cmd.stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .env("MYVCS_TRANSFER_TIMEOUT", "20");
        let mut child = cmd.spawn().unwrap();
        frame(
            child.stdin.as_mut().unwrap(),
            json!({"Hello":{"version":1,"object_format":1,"hash":"sha256","pack":mode}}),
        );
        assert!(
            read_frame(child.stdout.as_mut().unwrap())
                .get("Hello")
                .is_some()
        );
        assert!(
            read_frame(child.stdout.as_mut().unwrap())
                .get("Refs")
                .is_some()
        );
        child
    }
}
impl Drop for Lab {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
fn frame(w: &mut impl Write, m: Value) {
    let raw = serde_json::to_vec(&m).unwrap();
    w.write_all(&(raw.len() as u32).to_be_bytes()).unwrap();
    w.write_all(&raw).unwrap();
    w.flush().unwrap();
}

#[test]
fn thin_protocol_pins_base_before_gc_and_refuses_unconfirmed_base() {
    use sha2::{Digest, Sha256};
    for collect_before_pin in [false, true] {
        let lab = Lab::new();
        lab.setup();
        let old = lab.tip("server");
        let raw = b"orphan base for network delta".repeat(100);
        let base = hex_sha256(&raw);
        let path = lab
            .0
            .join("server/objects")
            .join(&base[..2])
            .join(&base[2..]);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        let mut z = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::default());
        z.write_all(&raw).unwrap();
        fs::write(&path, z.finish().unwrap()).unwrap();
        let target = [&raw[..], b"!"].concat();
        let bid = hex_sha256(&target);
        let commit = serde_json::to_vec(&Commit {
            tree: [("shared".into(), bid.clone())].into(),
            parents: old.clone().into_iter().collect(),
            parent: None,
            message: "pinned".into(),
            author: "test".into(),
            timestamp: 7,
        })
        .unwrap();
        let tip = hex_sha256(&commit);
        let mut child = lab.service_mode(false, "thin-zlib-v2");
        frame(
            child.stdin.as_mut().unwrap(),
            json!({"Push":{"branch":"main","expected":old,"new":tip}}),
        );
        assert_eq!(read_frame(child.stdout.as_mut().unwrap()), json!("Ok"));
        frame(
            child.stdin.as_mut().unwrap(),
            json!({"Offer":{"ids":[base,bid,tip]}}),
        );
        assert_eq!(
            read_frame(child.stdout.as_mut().unwrap()),
            json!({"Missing":{"ids":[bid,tip]}})
        );
        if collect_before_pin {
            lab.run(&["--repo", "server", "gc"]);
        }
        frame(
            child.stdin.as_mut().unwrap(),
            json!({"Bases":{"ids":[base]}}),
        );
        let pinned = if collect_before_pin {
            vec![]
        } else {
            vec![base.clone()]
        };
        assert_eq!(
            read_frame(child.stdout.as_mut().unwrap()),
            json!({"Pinned":{"ids":pinned}})
        );
        if !collect_before_pin {
            lab.run(&["--repo", "server", "gc"]);
        }
        assert!(!path.exists());
        let mut program = 2u32.to_be_bytes().to_vec();
        program.push(0);
        program.extend(0u64.to_be_bytes());
        program.extend((raw.len() as u64).to_be_bytes());
        program.push(1);
        program.extend(1u64.to_be_bytes());
        program.push(b'!');
        let mut data = b"MYVCSPK2".to_vec();
        data.extend(2u32.to_be_bytes());
        for (id, kind, encoding, len, payload) in [
            (&bid, 0u8, 2u8, target.len(), &program),
            (&tip, 1, 0, commit.len(), &commit),
        ] {
            let mut z = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::default());
            z.write_all(payload).unwrap();
            let payload = z.finish().unwrap();
            data.extend([kind, encoding]);
            data.extend(id.as_bytes());
            data.extend((len as u64).to_be_bytes());
            data.extend((payload.len() as u64).to_be_bytes());
            if encoding == 2 {
                data.extend(base.as_bytes());
            }
            data.extend(payload);
        }
        let sum = Sha256::digest(&data);
        data.extend(sum);
        frame(child.stdin.as_mut().unwrap(), json!("Pack"));
        child.stdin.as_mut().unwrap().write_all(&data).unwrap();
        frame(child.stdin.as_mut().unwrap(), json!("End"));
        let response = read_frame(child.stdout.as_mut().unwrap());
        if collect_before_pin {
            assert!(
                response["Error"]["message"]
                    .as_str()
                    .unwrap()
                    .contains("unconfirmed")
            );
            assert!(!child.wait().unwrap().success());
            assert_eq!(lab.tip("server"), old);
        } else {
            assert_eq!(response, json!("Ok"));
            assert!(child.wait().unwrap().success());
            assert_eq!(lab.tip("server"), Some(tip));
            assert_eq!(lab.repo("server").read_blob_resolved(&bid).unwrap(), target);
            assert!(
                !path.exists(),
                "unreachable transport-only base should not be installed"
            );
        }
        lab.run(&["--repo", "server", "verify"]);
        assert_eq!(
            fs::read_dir(lab.0.join("server/incoming")).unwrap().count(),
            0
        );
    }
}

#[test]
fn gc_and_plain_repositories_sync_both_ways_with_delta_and_restore_all_history() {
    let lab = Lab::new();
    lab.setup();
    let mut state = 993u64;
    let mut bytes: Vec<u8> = (0..128 * 1024)
        .map(|_| {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            state as u8
        })
        .collect();
    let mut history = Vec::new();
    lab.commit("a", "data", &bytes, "base data");
    history.push((lab.tip("a").unwrap(), bytes.clone()));
    lab.run(&["--repo", "a", "push"]);
    lab.run(&["--repo", "b", "pull"]);
    for i in 0..4 {
        let (sender, receiver) = if i % 2 == 0 { ("a", "b") } else { ("b", "a") };
        bytes[500 + i * 100..550 + i * 100].fill(i as u8);
        lab.commit(sender, "data", &bytes, &format!("edit {i}"));
        history.push((lab.tip(sender).unwrap(), bytes.clone()));
        if i % 2 == 0 {
            lab.run(&["--repo", sender, "gc"]);
        } else {
            lab.run(&["--repo", "server", "gc"]);
            lab.run(&["--repo", receiver, "gc"]);
        }
        let output = Command::new(env!("CARGO_BIN_EXE_myvcs"))
            .current_dir(&lab.0)
            .args(["--repo", sender, "push"])
            .env("MYVCS_TRACE_TRANSFER", "1")
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let log = String::from_utf8(output.stderr).unwrap();
        let stats: Value = serde_json::from_str(
            log.lines()
                .find_map(|line| line.strip_prefix("MYVCS_TRANSFER "))
                .unwrap(),
        )
        .unwrap();
        assert!(
            stats["pack"]["external_deltas"].as_u64().unwrap() > 0,
            "{stats}"
        );
        assert!(
            stats["pack"]["pack_bytes"].as_u64().unwrap() < 10000,
            "{stats}"
        );
        lab.run(&["--repo", receiver, "fetch"]);
        lab.run(&["--repo", receiver, "gc"]);
        lab.run(&["--repo", receiver, "pull"]);
        assert_eq!(lab.tip(sender), lab.tip(receiver));
    }
    lab.clone_to("fresh");
    for name in ["a", "b", "fresh"] {
        lab.run(&["--repo", name, "verify"]);
        for (id, expected) in &history {
            let c = lab.repo(name).read_commit(id).unwrap();
            assert_eq!(
                lab.repo(name).read_blob_resolved(&c.tree["data"]).unwrap(),
                *expected
            );
        }
    }
    // Force the old full-only wire mode to the new service, including download.
    let out = Command::new(env!("CARGO_BIN_EXE_myvcs"))
        .current_dir(&lab.0)
        .args(["clone", &lab.url(), "full-client"])
        .env("MYVCS_PACK_MODE", "full")
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(lab.tip("full-client"), lab.tip("fresh"));
}
fn read_frame(r: &mut impl Read) -> Value {
    let mut n = [0; 4];
    r.read_exact(&mut n).unwrap();
    let len = u32::from_be_bytes(n) as usize;
    assert!(len <= 1024 * 1024);
    let mut b = vec![0; len];
    r.read_exact(&mut b).unwrap();
    serde_json::from_slice(&b).unwrap()
}
fn proposed(parent: Option<String>, unsafe_path: bool) -> (String, Vec<(String, u8, Vec<u8>)>) {
    let raw = b"new content\n".to_vec();
    let blob = hex_sha256(&raw);
    let c = Commit {
        tree: [(
            if unsafe_path { "../escape" } else { "shared" }.into(),
            blob.clone(),
        )]
        .into(),
        parents: parent.into_iter().collect(),
        parent: None,
        message: "wire".into(),
        author: "test".into(),
        timestamp: 5,
    };
    let bytes = serde_json::to_vec(&c).unwrap();
    let id = hex_sha256(&bytes);
    (id.clone(), vec![(blob, 0, raw), (id, 1, bytes)])
}
fn pack(objects: &[(String, u8, Vec<u8>)]) -> Vec<u8> {
    use sha2::{Digest, Sha256};
    let mut out = b"MYVCSPK1".to_vec();
    out.extend_from_slice(&(objects.len() as u32).to_be_bytes());
    for (id, kind, raw) in objects {
        let mut z = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::default());
        z.write_all(raw).unwrap();
        let z = z.finish().unwrap();
        out.extend_from_slice(&[*kind, 0]);
        out.extend_from_slice(id.as_bytes());
        out.extend_from_slice(&(raw.len() as u64).to_be_bytes());
        out.extend_from_slice(&(z.len() as u64).to_be_bytes());
        out.extend_from_slice(&z);
    }
    let checksum = Sha256::digest(&out);
    out.extend_from_slice(&checksum);
    out
}
fn begin_push(
    child: &mut Child,
    old: &Option<String>,
    new: &str,
    objects: &[(String, u8, Vec<u8>)],
) {
    frame(
        child.stdin.as_mut().unwrap(),
        json!({"Push":{"branch":"main","expected":old,"new":new}}),
    );
    assert_eq!(read_frame(child.stdout.as_mut().unwrap()), json!("Ok"));
    frame(
        child.stdin.as_mut().unwrap(),
        json!({"Offer":{"ids":objects.iter().map(|o| &o.0).collect::<Vec<_>>()}}),
    );
    assert_eq!(
        read_frame(child.stdout.as_mut().unwrap())["Missing"]["ids"]
            .as_array()
            .unwrap()
            .len(),
        objects.len()
    );
}

#[test]
fn empty_clone_first_push_fetch_gc_pull_and_incremental_negotiation() {
    let lab = Lab::new();
    lab.run(&["init", "--bare", "server"]);
    lab.clone_to("a");
    lab.clone_to("b");
    assert_eq!(lab.tip("a"), None);
    assert_eq!(lab.tip("b"), None);
    lab.commit("a", "shared", b"first\n", "first");
    assert!(lab.run(&["--repo", "a", "push"]).contains("(2 objects)"));
    lab.run(&["--repo", "b", "fetch"]);
    assert_eq!(lab.tip("b"), None);
    assert!(!lab.0.join("b/shared").exists());
    let expected = lab.tip("a").unwrap();
    lab.run(&["--repo", "b", "gc"]);
    lab.run(&["--repo", "b", "verify"]);
    assert!(lab.repo("b").read_commit(&expected).is_ok());
    lab.run(&["--repo", "b", "pull", "--ff-only"]);
    assert_eq!(lab.tip("a"), lab.tip("b"));
    assert_eq!(
        fs::read(lab.0.join("a/shared")).unwrap(),
        fs::read(lab.0.join("b/shared")).unwrap()
    );
    assert!(lab.run(&["--repo", "a", "push"]).contains("(0 objects)"));
    lab.commit("a", "shared", b"second\n", "second");
    assert!(lab.run(&["--repo", "a", "push"]).contains("(2 objects)"));
    lab.run(&["--repo", "b", "pull"]);
    assert_eq!(lab.tip("a"), lab.tip("b"));
}

#[test]
fn stale_push_conflict_resolution_preserves_both_parents_and_clean_changes() {
    let lab = Lab::new();
    lab.setup();
    lab.commit("a", "shared", b"A\n", "A");
    lab.commit("a", "only-a", b"keep me\n", "clean addition");
    lab.run(&["--repo", "a", "push"]);
    lab.commit("b", "shared", b"B\n", "B");
    let ours = lab.tip("b").unwrap();
    let theirs = lab.tip("a").unwrap();
    lab.fail(&["--repo", "b", "push"], "non-fast-forward");
    assert_eq!(lab.tip("server").as_deref(), Some(theirs.as_str()));
    lab.fail(&["--repo", "b", "pull", "--ff-only"], "divergent");
    assert_eq!(lab.tip("b").as_deref(), Some(ours.as_str()));
    lab.run(&["--repo", "b", "pull"]);
    lab.fail(
        &["--repo", "b", "commit", "-m", "unresolved"],
        "unresolved merge conflict",
    );
    assert!(
        fs::read_to_string(lab.0.join("b/shared"))
            .unwrap()
            .contains("<<<<<<<")
    );
    lab.run(&["--repo", "b", "gc"]);
    lab.commit("b", "shared", b"A and B resolved\n", "resolved");
    let merged = lab.repo("b").read_commit(&lab.tip("b").unwrap()).unwrap();
    assert_eq!(merged.parents, vec![ours, theirs]);
    assert!(merged.tree.contains_key("only-a"));
    lab.run(&["--repo", "b", "push"]);
    lab.run(&["--repo", "a", "pull"]);
    assert_eq!(lab.tip("a"), lab.tip("b"));
    assert_eq!(fs::read(lab.0.join("a/only-a")).unwrap(), b"keep me\n");
    lab.clone_to("c");
    assert_eq!(
        lab.repo("c")
            .read_commit(&lab.tip("c").unwrap())
            .unwrap()
            .parents,
        merged.parents
    );
}

#[test]
fn pull_protects_dirty_staged_untracked_and_structural_collisions() {
    let lab = Lab::new();
    lab.setup();
    lab.commit("a", "new-file", b"remote", "remote");
    lab.run(&["--repo", "a", "push"]);
    let old = lab.tip("b");
    fs::write(lab.0.join("b/shared"), b"local").unwrap();
    lab.fail(&["--repo", "b", "pull"], "local changes");
    assert_eq!(lab.tip("b"), old);
    lab.run(&["--repo", "b", "add", "shared"]);
    lab.fail(&["--repo", "b", "pull"], "staged changes");
    lab.run(&["--repo", "b", "reset", "--hard", old.as_deref().unwrap()]);
    fs::write(lab.0.join("b/new-file"), b"untracked").unwrap();
    lab.fail(&["--repo", "b", "pull"], "untracked path");
    assert_eq!(fs::read(lab.0.join("b/new-file")).unwrap(), b"untracked");
    assert_eq!(lab.tip("b"), old);
    lab.run(&["--repo", "b", "gc"]);
    assert!(lab.repo("b").read_commit(&lab.tip("a").unwrap()).is_ok());
    fs::remove_file(lab.0.join("b/new-file")).unwrap();
    lab.run(&["--repo", "b", "pull"]);
    lab.run(&["--repo", "a", "rm", "new-file"]);
    fs::create_dir(lab.0.join("a/new-file")).unwrap();
    lab.commit("a", "new-file/child", b"child", "structural transition");
    lab.run(&["--repo", "a", "push"]);
    let old = lab.tip("b");
    lab.fail(&["--repo", "b", "pull"], "file/directory conflict");
    assert_eq!(lab.tip("b"), old);
}

#[test]
fn bad_and_interrupted_packs_never_publish() {
    for failure in [
        "checksum",
        "truncated",
        "missing-end",
        "hash",
        "dependency",
        "path",
        "duplicate",
        "limit",
        "encoding",
        "zlib-tail",
    ] {
        let lab = Lab::new();
        lab.setup();
        let old = lab.tip("server");
        let (new, mut objects) = proposed(old.clone(), failure == "path");
        if failure == "hash" {
            objects[0].2.push(1);
        }
        if failure == "dependency" {
            objects.remove(0);
        }
        let mut child = lab.service(false);
        begin_push(&mut child, &old, &new, &objects);
        let mut payload = pack(&objects);
        match failure {
            "checksum" => {
                let n = payload.len();
                payload[n - 1] ^= 1;
            }
            "truncated" => payload.truncate(payload.len() / 2),
            "duplicate" => payload = pack(&[objects[0].clone(), objects[0].clone()]),
            "limit" => payload[78..86].copy_from_slice(&u64::MAX.to_be_bytes()),
            "encoding" => payload[13] = 1,
            "zlib-tail" => {
                let n = u64::from_be_bytes(payload[86..94].try_into().unwrap());
                payload[86..94].copy_from_slice(&(n + 1).to_be_bytes());
            }
            _ => (),
        }
        frame(child.stdin.as_mut().unwrap(), json!("Pack"));
        child.stdin.as_mut().unwrap().write_all(&payload).unwrap();
        if !matches!(failure, "truncated" | "missing-end") {
            frame(child.stdin.as_mut().unwrap(), json!("End"));
        }
        drop(child.stdin.take());
        assert!(!child.wait().unwrap().success(), "{failure}");
        assert_eq!(lab.tip("server"), old, "{failure}");
        lab.run(&["--repo", "server", "verify"]);
    }
}

#[test]
fn competing_network_sessions_compare_old_value_at_publication() {
    let lab = Lab::new();
    lab.setup();
    let old = lab.tip("server");
    let (new, objects) = proposed(old.clone(), false);
    let mut first = lab.service(false);
    let mut second = lab.service(false);
    begin_push(&mut first, &old, &new, &objects);
    begin_push(&mut second, &old, &new, &objects);
    for (index, child) in [&mut first, &mut second].into_iter().enumerate() {
        frame(child.stdin.as_mut().unwrap(), json!("Pack"));
        child
            .stdin
            .as_mut()
            .unwrap()
            .write_all(&pack(&objects))
            .unwrap();
        frame(child.stdin.as_mut().unwrap(), json!("End"));
        let response = read_frame(child.stdout.as_mut().unwrap());
        if index == 0 {
            assert_eq!(response, json!("Ok"));
            assert!(child.wait().unwrap().success());
        } else {
            assert!(
                response["Error"]["message"]
                    .as_str()
                    .unwrap()
                    .contains("changed concurrently")
            );
            assert!(!child.wait().unwrap().success());
        }
    }
    assert_eq!(lab.tip("server"), Some(new));
}

#[test]
fn negotiation_spans_multiple_batches_and_fetch_head_is_a_gc_root() {
    use myvcs::repo::{ReceiveLimits, RefUpdate};
    let lab = Lab::new();
    lab.run(&["init", "--bare", "server"]);
    let server = lab.repo("server");
    let mut tx = server.begin_receive(ReceiveLimits::default()).unwrap();
    let mut tree = std::collections::BTreeMap::new();
    for i in 0..1030 {
        let bytes = format!("unique object {i}").into_bytes();
        let id = hex_sha256(&bytes);
        tx.stage_object(&id, bytes.as_slice()).unwrap();
        tree.insert(format!("file-{i}"), id);
    }
    let c = Commit {
        tree,
        parents: vec![],
        parent: None,
        message: "many".into(),
        author: "test".into(),
        timestamp: 0,
    };
    let raw = serde_json::to_vec(&c).unwrap();
    let id = hex_sha256(&raw);
    tx.stage_object(&id, raw.as_slice()).unwrap();
    tx.commit(RefUpdate {
        name: "refs/heads/main".into(),
        expected: None,
        new: id.clone(),
        allow_non_fast_forward: false,
    })
    .unwrap();
    lab.clone_to("a");
    assert_eq!(lab.tip("a"), Some(id.clone()));
    assert!(lab.run(&["--repo", "a", "push"]).contains("(0 objects)"));
    // Remove both ordinary reference roots; FETCH_HEAD alone must retain history.
    fs::remove_file(lab.0.join("a/.myvcs/refs/heads/main")).unwrap();
    fs::remove_file(lab.0.join("a/.myvcs/refs/remotes/origin/main")).unwrap();
    lab.run(&["--repo", "a", "gc"]);
    assert!(lab.repo("a").read_commit(&id).is_ok());
    lab.run(&["--repo", "a", "verify"]);
}

#[test]
fn outgoing_snapshot_survives_gc_during_network_negotiation() {
    use sha2::{Digest, Sha256};
    let lab = Lab::new();
    lab.setup();
    let tip = lab.tip("server");
    let mut child = lab.service(false);
    frame(
        child.stdin.as_mut().unwrap(),
        json!({"Fetch":{"branch":"main","tip":tip}}),
    );
    assert_eq!(read_frame(child.stdout.as_mut().unwrap()), json!("Ok"));
    let offer = read_frame(child.stdout.as_mut().unwrap());
    let ids = offer["Offer"]["ids"].as_array().unwrap();
    fs::remove_file(lab.0.join("server/refs/heads/main")).unwrap();
    lab.run(&["--repo", "server", "gc"]); // must not block behind network I/O
    assert!(
        lab.repo("server")
            .read_commit(tip.as_deref().unwrap())
            .is_err()
    );
    frame(
        child.stdin.as_mut().unwrap(),
        json!({"Missing":{"ids":ids}}),
    );
    assert_eq!(read_frame(child.stdout.as_mut().unwrap()), json!("Pack"));
    let input = child.stdout.as_mut().unwrap();
    let mut bytes = vec![0; 12];
    input.read_exact(&mut bytes).unwrap();
    assert_eq!(&bytes[..8], b"MYVCSPK1");
    let count = u32::from_be_bytes(bytes[8..12].try_into().unwrap());
    assert_eq!(count as usize, ids.len());
    for _ in 0..count {
        let mut entry = vec![0; 82];
        input.read_exact(&mut entry).unwrap();
        let n = u64::from_be_bytes(entry[74..82].try_into().unwrap());
        let mut payload = vec![0; n as usize];
        input.read_exact(&mut payload).unwrap();
        let mut logical = Vec::new();
        flate2::read::ZlibDecoder::new(payload.as_slice())
            .read_to_end(&mut logical)
            .unwrap();
        assert_eq!(hex_sha256(&logical).as_bytes(), &entry[2..66]);
        bytes.extend(entry);
        bytes.extend(payload);
    }
    let mut digest = [0; 32];
    input.read_exact(&mut digest).unwrap();
    assert_eq!(digest[..], Sha256::digest(&bytes)[..]);
    assert_eq!(read_frame(input), json!("End"));
    assert_eq!(read_frame(input), json!("Ok"));
    assert!(child.wait().unwrap().success());
}

#[test]
fn remote_config_nested_branch_read_only_and_url_validation() {
    let lab = Lab::new();
    lab.setup();
    lab.run(&["--repo", "a", "branch", "feature/one"]);
    lab.run(&["--repo", "a", "push", "origin", "feature/one"]);
    lab.run(&["clone", &lab.url(), "feature", "--branch", "feature/one"]);
    assert_eq!(lab.tip("feature"), lab.tip("a"));
    lab.fail(&["clone", &lab.url(), "a"], "destination must not exist");
    lab.run(&["--repo", "a", "remote", "add", "extra", &lab.url()]);
    assert!(lab.run(&["--repo", "a", "remote"]).contains("extra"));
    lab.run(&["--repo", "a", "remote", "set-url", "extra", &lab.url()]);
    lab.run(&["--repo", "a", "remote", "remove", "extra"]);
    for url in [
        "ssh://-oProxyCommand=bad/repo",
        "ssh://host/repo;id",
        "ssh://host/../outside",
        "ssh://u@host:0/repo",
        "https://host/repo",
        "ssh://u@host/a%2fb",
    ] {
        assert!(
            !lab.output(&["--repo", "a", "remote", "add", "bad", url])
                .status
                .success(),
            "{url}"
        );
    }
    let mut child = lab.service(true);
    frame(
        child.stdin.as_mut().unwrap(),
        json!({"Push":{"branch":"main","expected":lab.tip("server"),"new":lab.tip("server")}}),
    );
    assert!(
        read_frame(child.stdout.as_mut().unwrap())["Error"]["message"]
            .as_str()
            .unwrap()
            .contains("read-only")
    );
    assert!(!child.wait().unwrap().success());
}

#[test]
fn gc_delta_storage_exports_complete_logical_objects() {
    let lab = Lab::new();
    lab.setup();
    let mut data = (0..16000)
        .map(|i| format!("line-{i:05}: unchanged text\n"))
        .collect::<String>()
        .into_bytes();
    lab.commit("a", "large", &data, "large base");
    data.splice(1000..1005, b"EDIT!".iter().copied());
    lab.commit("a", "large", &data, "large edit");
    lab.run(&["--repo", "a", "gc"]);
    lab.run(&["--repo", "a", "push"]);
    lab.run(&["--repo", "server", "gc"]);
    lab.clone_to("after-gc");
    lab.run(&["--repo", "after-gc", "verify"]);
    assert_eq!(lab.tip("after-gc"), lab.tip("a"));
    assert_eq!(fs::read(lab.0.join("after-gc/large")).unwrap(), data);
}

#[test]
fn bounded_frames_and_handshake_reject_invalid_peers() {
    let lab = Lab::new();
    lab.run(&["init", "--bare", "server"]);
    for input in [
        vec![255, 255, 255, 255],
        vec![0, 0, 0, 0],
        vec![0, 0, 0, 5, b'{'],
    ] {
        let mut child = Command::new(env!("CARGO_BIN_EXE_myvcs"))
            .args(["serve", "--root"])
            .arg(&lab.0)
            .args(["--repository", "server"])
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        child.stdin.take().unwrap().write_all(&input).unwrap();
        assert!(!child.wait().unwrap().success());
        assert_eq!(lab.tip("server"), None);
    }
    lab.fail(
        &[
            "serve",
            "--root",
            lab.0.to_str().unwrap(),
            "--repository",
            "../outside",
        ],
        "invalid repository identifier",
    );
    let mut child = Command::new(env!("CARGO_BIN_EXE_myvcs"))
        .args(["serve", "--root"])
        .arg(&lab.0)
        .args(["--repository", "server"])
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .env("MYVCS_TRANSFER_TIMEOUT", "1")
        .spawn()
        .unwrap();
    let _open_input = child.stdin.take().unwrap();
    assert_eq!(child.wait().unwrap().code(), Some(124));
}
