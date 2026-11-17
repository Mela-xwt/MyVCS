use myvcs::{
    model::{Commit, DeltaBlob, DeltaOp},
    repo::{ReadLimits, ReceiveLimits, RefUpdate, Repo},
    util::{compress_bytes, encode_delta_blob, hex_sha256},
};
use std::{
    fs,
    path::PathBuf,
    sync::atomic::{AtomicU64, Ordering},
};

struct Lab(PathBuf);
impl Lab {
    fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let p = std::env::temp_dir().join(format!(
            "myvcs-receive-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&p).unwrap();
        Self(p)
    }
    fn repo(&self) -> Repo {
        Repo::init_at(self.0.join("server"), true, true).unwrap()
    }
}
impl Drop for Lab {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
fn commit(blob: &str, parent: Option<String>) -> Vec<u8> {
    serde_json::to_vec(&Commit {
        tree: [("file.txt".into(), blob.into())].into(),
        parents: parent.into_iter().collect(),
        parent: None,
        message: "received".into(),
        author: "test".into(),
        timestamp: 1,
    })
    .unwrap()
}
fn update(id: &str, expected: Option<String>) -> RefUpdate {
    RefUpdate {
        name: "refs/heads/main".into(),
        expected,
        new: id.into(),
        allow_non_fast_forward: false,
    }
}
fn object_path(repo: &Repo, oid: &str) -> PathBuf {
    repo.metadata_dir()
        .join("objects")
        .join(&oid[..2])
        .join(&oid[2..])
}
fn legacy(repo: &Repo, oid: &str, raw: &[u8]) {
    let p = object_path(repo, oid);
    fs::create_dir_all(p.parent().unwrap()).unwrap();
    fs::write(
        p,
        compress_bytes(raw, flate2::Compression::default()).unwrap(),
    )
    .unwrap();
}

#[test]
fn receive_validates_closure_and_publishes_bare_ref() {
    let lab = Lab::new();
    let repo = lab.repo();
    let blob = b"hello";
    let bid = hex_sha256(blob);
    let raw = commit(&bid, None);
    let cid = hex_sha256(&raw);
    let mut tx = repo.begin_receive(ReceiveLimits::default()).unwrap();
    tx.stage_object(&cid, &raw[..]).unwrap();
    assert!(tx.commit(update(&cid, None)).is_err());
    assert_eq!(repo.head_commit_id().unwrap(), None);
    let mut tx = repo.begin_receive(ReceiveLimits::default()).unwrap();
    tx.stage_object(&bid, &blob[..]).unwrap();
    tx.stage_object(&cid, &raw[..]).unwrap();
    repo.with_repo_lock(|r| r.gc()).unwrap(); // quarantine must survive GC
    tx.commit(update(&cid, None)).unwrap();
    assert_eq!(
        Repo::open(repo.metadata_dir())
            .unwrap()
            .head_commit_id()
            .unwrap()
            .as_deref(),
        Some(cid.as_str())
    );
    assert!(repo.is_ancestor(&cid, &cid).unwrap());
    repo.verify().unwrap();
}

#[test]
fn receive_limits_hashes_and_stale_cas_are_enforced() {
    let lab = Lab::new();
    let repo = lab.repo();
    let b = b"hello";
    let bid = hex_sha256(b);
    let mut tx = repo
        .begin_receive(ReceiveLimits {
            max_object_bytes: 4,
            ..Default::default()
        })
        .unwrap();
    assert!(tx.stage_object(&bid, &b[..]).is_err());
    let mut tx = repo.begin_receive(ReceiveLimits::default()).unwrap();
    assert!(tx.stage_object(&"a".repeat(64), &b[..]).is_err());
    let mut tx = repo
        .begin_receive(ReceiveLimits {
            max_objects: 1,
            ..Default::default()
        })
        .unwrap();
    tx.stage_object(&bid, &b[..]).unwrap();
    assert!(
        tx.stage_object(&hex_sha256(b"other"), &b"other"[..])
            .is_err()
    );
    let raw = commit(&bid, None);
    let cid = hex_sha256(&raw);
    let mut tx = repo.begin_receive(ReceiveLimits::default()).unwrap();
    tx.stage_object(&bid, &b[..]).unwrap();
    tx.stage_object(&cid, &raw[..]).unwrap();
    tx.commit(update(&cid, None)).unwrap();
    let tx = repo.begin_receive(ReceiveLimits::default()).unwrap();
    assert!(tx.commit(update(&cid, None)).is_err()); // stale create
    let tx = repo.begin_receive(ReceiveLimits::default()).unwrap();
    tx.commit(update(&cid, Some(cid.clone()))).unwrap(); // equality is a fast-forward
}

#[test]
fn legacy_delta_bases_are_gc_roots_and_literal_magic_is_full_content() {
    let lab = Lab::new();
    let repo = lab.repo();
    let base = b"base";
    let bid = hex_sha256(base);
    legacy(&repo, &bid, base);
    let target = b"base!";
    let tid = hex_sha256(target);
    let delta = DeltaBlob {
        base: bid.clone(),
        size: 5,
        ops: vec![
            DeltaOp::Copy { offset: 0, len: 4 },
            DeltaOp::Insert {
                data: b"!".to_vec(),
            },
        ],
    };
    let raw = encode_delta_blob(&delta).unwrap();
    legacy(&repo, &tid, &raw);
    let c = commit(&tid, None);
    let cid = hex_sha256(&c);
    legacy(&repo, &cid, &c);
    repo.update_ref("refs/heads/main", None, &cid).unwrap();
    repo.with_repo_lock(|r| r.gc()).unwrap();
    assert!(object_path(&repo, &bid).exists());
    assert_eq!(repo.read_blob_resolved(&tid).unwrap(), target);
    let literal_id = hex_sha256(&raw);
    legacy(&repo, &literal_id, &raw);
    assert_eq!(repo.read_blob_resolved(&literal_id).unwrap(), raw);
}

#[test]
fn malicious_commit_paths_and_bounded_delta_expansion_fail_closed() {
    let lab = Lab::new();
    let repo = lab.repo();
    let bid = hex_sha256(b"base");
    legacy(&repo, &bid, b"base");
    let mut c: Commit = serde_json::from_slice(&commit(&bid, None)).unwrap();
    c.tree = [("../escape".into(), bid.clone())].into();
    let raw = serde_json::to_vec(&c).unwrap();
    let cid = hex_sha256(&raw);
    legacy(&repo, &cid, &raw);
    assert!(repo.update_ref("refs/heads/main", None, &cid).is_err());
    assert_eq!(repo.head_commit_id().unwrap(), None);
    let tid = hex_sha256(b"basebase");
    let delta = DeltaBlob {
        base: bid,
        size: 4,
        ops: vec![DeltaOp::Copy { offset: 0, len: 4 }; 2],
    };
    legacy(&repo, &tid, &encode_delta_blob(&delta).unwrap());
    assert!(repo.read_blob_resolved(&tid).is_err());
    let small = repo
        .with_read_limits(ReadLimits {
            object_bytes: 16,
            ..Default::default()
        })
        .unwrap();
    assert!(small.read_blob_resolved(&tid).is_err());
}

#[test]
fn compressed_truncation_and_legacy_preallocation_padding() {
    let lab = Lab::new();
    let repo = lab.repo();
    let bytes = b"old padded object";
    let id = hex_sha256(bytes);
    let mut encoded = compress_bytes(bytes, flate2::Compression::default()).unwrap();
    let p = object_path(&repo, &id);
    fs::create_dir_all(p.parent().unwrap()).unwrap();
    // Older EulerFS preallocation may leave zero padding after a valid zlib stream.
    let mut padded = encoded.clone();
    padded.extend_from_slice(&[0; 128]);
    fs::write(&p, padded).unwrap();
    assert_eq!(repo.read_blob_resolved(&id).unwrap(), bytes);
    encoded.truncate(encoded.len() - 2);
    fs::write(&p, encoded).unwrap();
    assert!(
        repo.read_blob_resolved(&id).is_err(),
        "truncated zlib checksum accepted"
    );
}

#[test]
fn merge_head_and_detached_head_are_gc_roots() {
    let lab = Lab::new();
    let repo = lab.repo();
    let bid = hex_sha256(b"merge only");
    legacy(&repo, &bid, b"merge only");
    let raw = commit(&bid, None);
    let cid = hex_sha256(&raw);
    legacy(&repo, &cid, &raw);
    fs::write(repo.metadata_dir().join("MERGE_HEAD"), &cid).unwrap();
    repo.with_repo_lock(|r| r.gc()).unwrap();
    repo.verify().unwrap();
    assert!(object_path(&repo, &bid).exists());
    fs::remove_file(repo.metadata_dir().join("MERGE_HEAD")).unwrap();
    fs::write(repo.metadata_dir().join("HEAD"), &cid).unwrap();
    repo.with_repo_lock(|r| r.gc()).unwrap();
    repo.verify().unwrap();
    assert!(object_path(&repo, &bid).exists());
}

#[test]
fn legacy_parent_and_repository_without_marker_remain_readable() {
    let lab = Lab::new();
    let repo = Repo::init_at(lab.0.join("legacy"), false, false).unwrap();
    let parent = repo.head_commit_id().unwrap().unwrap();
    fs::remove_file(repo.metadata_dir().join("format.json")).unwrap();
    let bid = hex_sha256(b"old");
    legacy(&repo, &bid, b"old");
    let mut c: Commit = serde_json::from_slice(&commit(&bid, None)).unwrap();
    c.parent = Some(parent.clone());
    let raw = serde_json::to_vec(&c).unwrap();
    let cid = hex_sha256(&raw);
    legacy(&repo, &cid, &raw);
    let reopened = Repo::open(repo.metadata_dir()).unwrap();
    reopened
        .update_ref("refs/heads/main", Some(&parent), &cid)
        .unwrap();
    assert!(reopened.is_ancestor(&parent, &cid).unwrap());
    reopened.verify().unwrap();
}

#[test]
fn compare_and_swap_has_one_winner_and_rejects_non_fast_forward() {
    let lab = Lab::new();
    let repo = lab.repo();
    let bid = hex_sha256(b"shared");
    legacy(&repo, &bid, b"shared");
    let raw = commit(&bid, None);
    let old = hex_sha256(&raw);
    legacy(&repo, &old, &raw);
    repo.update_ref("refs/heads/main", None, &old).unwrap();
    assert!(repo.is_ancestor(&old, &old).unwrap());
    let mut ids = Vec::new();
    for message in ["left", "right"] {
        let mut c: Commit = serde_json::from_slice(&commit(&bid, Some(old.clone()))).unwrap();
        c.message = message.into();
        let raw = serde_json::to_vec(&c).unwrap();
        let id = hex_sha256(&raw);
        legacy(&repo, &id, &raw);
        ids.push(id);
    }
    let barrier = std::sync::Arc::new(std::sync::Barrier::new(2));
    let handles: Vec<_> = ids
        .iter()
        .map(|id| {
            let r = repo.clone();
            let old = old.clone();
            let id = id.clone();
            let b = barrier.clone();
            std::thread::spawn(move || {
                b.wait();
                r.update_ref("refs/heads/main", Some(&old), &id).is_ok()
            })
        })
        .collect();
    assert_eq!(
        handles
            .into_iter()
            .filter_map(|h| h.join().ok())
            .filter(|ok| *ok)
            .count(),
        1
    );
    let tip = repo.head_commit_id().unwrap().unwrap();
    assert!(repo.is_ancestor(&old, &tip).unwrap());
    let tx = repo.begin_receive(ReceiveLimits::default()).unwrap();
    assert!(tx.commit(update(&old, Some(tip.clone()))).is_err());
    assert_eq!(repo.head_commit_id().unwrap(), Some(tip));
    repo.verify().unwrap();
}

#[test]
fn abandoned_atomic_temps_do_not_become_references() {
    let lab = Lab::new();
    let repo = lab.repo();
    fs::write(
        repo.metadata_dir().join("refs/heads/.myvcs-tmp-123-456-0"),
        "partial",
    )
    .unwrap();
    assert!(repo.list_refs().unwrap().is_empty());
    repo.with_repo_lock(|r| r.gc()).unwrap();
}

#[test]
fn aggregate_receive_bytes_and_decode_depth_are_bounded() {
    let lab = Lab::new();
    let repo = lab.repo();
    let mut tx = repo
        .begin_receive(ReceiveLimits {
            max_total_bytes: 5,
            ..Default::default()
        })
        .unwrap();
    tx.stage_object(&hex_sha256(b"1234"), &b"1234"[..]).unwrap();
    assert!(tx.stage_object(&hex_sha256(b"56"), &b"56"[..]).is_err());
    let bid = hex_sha256(b"a");
    legacy(&repo, &bid, b"a");
    let tid = hex_sha256(b"aa");
    let delta = DeltaBlob {
        base: bid,
        size: 2,
        ops: vec![DeltaOp::Copy { offset: 0, len: 1 }; 2],
    };
    legacy(&repo, &tid, &encode_delta_blob(&delta).unwrap());
    let limited = repo
        .with_read_limits(ReadLimits {
            delta_depth: 1,
            ..Default::default()
        })
        .unwrap();
    assert!(limited.read_blob_resolved(&tid).is_err());
}

#[cfg(unix)]
#[test]
fn metadata_symlinks_are_rejected_before_read_or_write() {
    let lab = Lab::new();
    let repo = lab.repo();
    let outside = lab.0.join("outside");
    fs::create_dir(&outside).unwrap();
    std::os::unix::fs::symlink(&outside, repo.metadata_dir().join("refs/heads/escape")).unwrap();
    assert!(repo.read_ref("refs/heads/escape/main").is_err());
    assert!(repo.list_refs().is_err());
    assert!(fs::read_dir(outside).unwrap().next().is_none());
}

#[cfg(unix)]
#[test]
fn reference_enumeration_rejects_backslash_aliases() {
    let lab = Lab::new();
    let repo = lab.repo();
    fs::write(
        repo.metadata_dir().join("refs/heads/wrong\\name"),
        "a".repeat(64),
    )
    .unwrap();
    assert!(repo.list_refs().is_err());
    assert!(repo.with_repo_lock(|r| r.gc()).is_err());
}

#[test]
fn checkout_missing_parent_does_not_move_head() {
    let lab = Lab::new();
    let repo = Repo::init_at(lab.0.join("work"), false, false).unwrap();
    let before = repo.head_commit_id().unwrap();
    let c = Commit {
        tree: Default::default(),
        parents: vec!["a".repeat(64)],
        parent: None,
        message: "incomplete".into(),
        author: "test".into(),
        timestamp: 0,
    };
    let raw = serde_json::to_vec(&c).unwrap();
    let id = hex_sha256(&raw);
    legacy(&repo, &id, &raw);
    assert!(repo.with_repo_lock(|r| r.checkout(&id)).is_err());
    assert_eq!(repo.head_commit_id().unwrap(), before);
}
