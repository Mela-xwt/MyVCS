use std::fs;
use std::io::{Read, Write};
use std::path::PathBuf;
use std::process::{Command, Output};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use flate2::Compression;
use flate2::read::ZlibDecoder;
use flate2::write::ZlibEncoder;
use serde_json::{Value, json};

static NEXT_REPO: AtomicU64 = AtomicU64::new(0);

struct TestRepo(PathBuf);

impl TestRepo {
    fn new() -> Self {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = std::env::temp_dir().join(format!(
            "myvcs-regression-{}-{}-{}",
            std::process::id(),
            nonce,
            NEXT_REPO.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&path).unwrap();
        let repo = Self(path);
        repo.run(&["init"]);
        repo
    }

    // Each command starts a fresh process, exercising persisted merge state.
    fn output(&self, args: &[&str]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_myvcs"))
            .args(args)
            .env("MYVCS_FORCE_DEEP_DELTA", "1")
            .env_remove("MYVCS_LARGE_FILE_COMPRESSION")
            .current_dir(&self.0)
            .output()
            .unwrap()
    }

    fn run(&self, args: &[&str]) -> String {
        let output = self.output(args);
        assert!(
            output.status.success(),
            "{args:?}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8(output.stdout).unwrap()
    }

    fn write(&self, path: &str, content: &str) {
        fs::write(self.0.join(path), content).unwrap();
    }

    fn commit_file(&self, path: &str, content: &str, message: &str) -> String {
        self.write(path, content);
        self.run(&["add", path]);
        self.run(&["commit", "-m", message]);
        self.head()
    }

    fn head(&self) -> String {
        let raw = fs::read_to_string(self.0.join(".myvcs/HEAD")).unwrap();
        match raw.trim().strip_prefix("ref: ") {
            Some(reference) => fs::read_to_string(self.0.join(".myvcs").join(reference))
                .unwrap()
                .trim()
                .to_owned(),
            None => raw.trim().to_owned(),
        }
    }

    fn commit(&self, id: &str) -> Value {
        let path = self.0.join(".myvcs/objects").join(&id[..2]).join(&id[2..]);
        serde_json::from_reader(ZlibDecoder::new(fs::File::open(path).unwrap())).unwrap()
    }

    fn conflicting_merge(&self) -> (String, String) {
        self.commit_file("hello.txt", "base\n", "base");
        self.run(&["branch", "feature"]);
        let ours = self.commit_file("hello.txt", "ours\n", "ours");
        self.run(&["checkout", "feature"]);
        let theirs = self.commit_file("hello.txt", "theirs\n", "theirs");
        self.run(&["checkout", "main"]);
        assert!(
            self.run(&["merge", "feature"])
                .contains("merge has conflicts")
        );
        (ours, theirs)
    }
}

fn object_path(repo: &TestRepo, oid: &str) -> PathBuf {
    repo.0
        .join(".myvcs/objects")
        .join(&oid[..2])
        .join(&oid[2..])
}

fn compress_for_test(bytes: &[u8], level: Compression) -> Vec<u8> {
    let mut encoder = ZlibEncoder::new(Vec::new(), level);
    encoder.write_all(bytes).unwrap();
    encoder.finish().unwrap()
}

fn add_with_policy(repo: &TestRepo, policy: &str) -> Output {
    Command::new(env!("CARGO_BIN_EXE_myvcs"))
        .args(["add", "data.bin"])
        .env("MYVCS_LARGE_FILE_COMPRESSION", policy)
        .current_dir(&repo.0)
        .output()
        .unwrap()
}

#[test]
fn large_add_policy_preserves_content_and_existing_objects() {
    use sha2::{Digest, Sha256};
    let data = vec![b'A'; 10 * 1024 * 1024];
    let oid = format!("{:x}", Sha256::digest(&data));
    let mut sizes = Vec::new();
    for policy in ["default", "fast", "auto"] {
        let repo = TestRepo::new();
        fs::write(repo.0.join("data.bin"), &data).unwrap();
        let output = add_with_policy(&repo, policy);
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let stored = fs::read(object_path(&repo, &oid)).unwrap();
        sizes.push(stored.len());
        let mut decoded = Vec::new();
        ZlibDecoder::new(stored.as_slice())
            .read_to_end(&mut decoded)
            .unwrap();
        assert_eq!(decoded, data);
        assert!(
            add_with_policy(&repo, if policy == "fast" { "default" } else { "fast" })
                .status
                .success()
        );
        assert_eq!(fs::read(object_path(&repo, &oid)).unwrap(), stored);
        repo.run(&["commit", "-m", "large file"]);
        repo.run(&["verify"]);
    }
    assert!(
        sizes[1] > sizes[0] * 2,
        "explicit fast should use a different compression level: {sizes:?}"
    );
    assert_eq!(
        sizes[2], sizes[0],
        "auto must retain default compression for repetitive input"
    );
}

#[test]
fn invalid_large_add_policy_leaves_index_and_objects_unchanged() {
    let repo = TestRepo::new();
    let index = fs::read(repo.0.join(".myvcs/index.json")).unwrap();
    let objects_before: Vec<_> = walkdir::WalkDir::new(repo.0.join(".myvcs/objects"))
        .into_iter()
        .map(|entry| entry.unwrap().into_path())
        .collect();
    fs::write(repo.0.join("data.bin"), vec![b'A'; 10 * 1024 * 1024]).unwrap();
    let output = add_with_policy(&repo, "invalid");
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("MYVCS_LARGE_FILE_COMPRESSION"));
    assert_eq!(fs::read(repo.0.join(".myvcs/index.json")).unwrap(), index);
    let objects_after: Vec<_> = walkdir::WalkDir::new(repo.0.join(".myvcs/objects"))
        .into_iter()
        .map(|entry| entry.unwrap().into_path())
        .collect();
    assert_eq!(objects_after, objects_before);
}

#[test]
fn large_add_policy_does_not_change_small_file_path() {
    let repo = TestRepo::new();
    repo.write("data.bin", "small file");
    assert!(add_with_policy(&repo, "invalid").status.success());
    repo.run(&["commit", "-m", "small file"]);
    repo.run(&["verify"]);
}

#[test]
fn gc_compares_against_actual_stored_size_and_writes_final_compression() {
    let repo = TestRepo::new();
    let base = "A".repeat(4096);
    let original_commit = repo.commit_file("file.txt", &base, "base");
    let target = format!("{}B", "A".repeat(4095));
    let target_commit = repo.commit_file("file.txt", &target, "target");
    let commit = repo.commit(&target_commit);
    let oid = commit["tree"]["file.txt"].as_str().unwrap();
    let path = object_path(&repo, oid);

    // A valid loose object may use any zlib level. Its real size, not a
    // hypothetical recompression at the default level, determines savings.
    let stored = compress_for_test(target.as_bytes(), Compression::none());
    fs::write(&path, &stored).unwrap();
    repo.run(&["verify"]);
    let output = repo.run(&["gc"]);
    assert!(output.contains("optimized 1 blob(s)"), "{output}");

    let compressed = fs::read(&path).unwrap();
    assert!(compressed.len() + 16 < stored.len());
    let mut raw = Vec::new();
    ZlibDecoder::new(&compressed[..])
        .read_to_end(&mut raw)
        .unwrap();
    assert!(raw.starts_with(b"MYVCS_DELTA_V1\\n"));
    assert_eq!(compressed, compress_for_test(&raw, Compression::best()));
    repo.run(&["verify"]);
    for (id, content) in [(&original_commit, &base), (&target_commit, &target)] {
        repo.run(&["checkout", id]);
        assert_eq!(
            fs::read_to_string(repo.0.join("file.txt")).unwrap(),
            *content
        );
    }
    let output = repo.run(&["gc"]);
    assert!(output.contains("optimized 0 blob(s)"), "{output}");
    assert_eq!(fs::read(path).unwrap(), compressed);
    repo.run(&["verify"]);
}

#[test]
fn gc_keeps_original_object_when_delta_does_not_save_enough_space() {
    let repo = TestRepo::new();
    repo.commit_file("file.txt", &"A".repeat(4096), "base");
    let target = format!("{}B", "A".repeat(4095));
    let id = repo.commit_file("file.txt", &target, "target");
    let commit = repo.commit(&id);
    let path = object_path(&repo, commit["tree"]["file.txt"].as_str().unwrap());
    let original = compress_for_test(target.as_bytes(), Compression::best());
    fs::write(&path, &original).unwrap();
    let output = repo.run(&["gc"]);
    assert!(output.contains("optimized 0 blob(s)"), "{output}");
    assert_eq!(fs::read(path).unwrap(), original);
    repo.run(&["verify"]);
}

#[test]
fn gc_preserves_multiple_versions_and_is_idempotent() {
    let repo = TestRepo::new();
    let mut seed = 123456789u64;
    let mut content: Vec<u8> = (0..32768)
        .map(|_| {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            b'a' + (seed % 26) as u8
        })
        .collect();
    let mut versions = Vec::new();
    for n in 0..3 {
        content[1024 * (n + 1)..1024 * (n + 1) + 64].fill(b'0' + n as u8);
        let text = String::from_utf8(content.clone()).unwrap();
        let id = repo.commit_file("file.txt", &text, &format!("version {n}"));
        versions.push((id, text));
    }
    let output = repo.run(&["gc"]);
    assert!(output.contains("optimized 2 blob(s)"), "{output}");
    repo.run(&["verify"]);
    let output = repo.run(&["gc"]);
    assert!(output.contains("optimized 0 blob(s)"), "{output}");
    repo.run(&["verify"]);
    for (id, content) in versions {
        repo.run(&["checkout", &id]);
        assert_eq!(
            fs::read_to_string(repo.0.join("file.txt")).unwrap(),
            content
        );
    }
}

impl Drop for TestRepo {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[test]
fn resolved_merge_records_both_parents_and_preserves_history_after_gc() {
    let repo = TestRepo::new();
    let (ours, theirs) = repo.conflicting_merge();
    let merged = repo.commit_file("hello.txt", "resolved\n", "resolve conflict");
    assert_eq!(repo.commit(&merged)["parents"], json!([ours, theirs]));
    assert!(!repo.run(&["status"]).contains("Merge conflicts:"));
    assert!(
        repo.run(&["merge", "feature"])
            .contains("already up to date")
    );
    assert_eq!(repo.head(), merged);
    repo.run(&["branch", "-d", "feature"]);
    repo.run(&["gc"]);
    repo.run(&["verify"]);
    assert_eq!(repo.commit(&theirs)["message"], "theirs");
    let next = repo.commit_file("hello.txt", "next\n", "ordinary commit");
    assert_eq!(repo.commit(&next)["parents"], json!([merged]));
}

#[test]
fn staged_deletion_is_not_reported_as_unstaged() {
    let repo = TestRepo::new();
    repo.commit_file("file.txt", "tracked\n", "base");
    repo.run(&["rm", "file.txt"]);
    let status = repo.run(&["status"]);
    assert!(
        status.contains("Changes to be committed:\n  deleted: file.txt"),
        "{status}"
    );
    assert!(status.contains("No unstaged changes"), "{status}");
    assert_eq!(status.matches("deleted: file.txt").count(), 1, "{status}");
    repo.run(&["commit", "-m", "delete"]);
    assert!(
        repo.commit(&repo.head())["tree"]
            .as_object()
            .unwrap()
            .is_empty()
    );
    assert!(repo.run(&["status"]).contains("No staged changes"));
}

#[test]
fn unstaged_deletion_is_still_reported() {
    let repo = TestRepo::new();
    repo.commit_file("file.txt", "tracked\n", "base");
    fs::remove_file(repo.0.join("file.txt")).unwrap();
    let status = repo.run(&["status"]);
    assert!(status.contains("No staged changes"), "{status}");
    assert!(
        status.contains("Changes not staged for commit:\n  deleted: file.txt"),
        "{status}"
    );
}

#[test]
fn another_merge_cannot_discard_pending_merge_parent() {
    let repo = TestRepo::new();
    let (ours, theirs) = repo.conflicting_merge();
    let output = repo.output(&["merge", "main"]);
    assert!(
        !output.status.success(),
        "a pending merge must not be overwritten"
    );
    let merged = repo.commit_file("hello.txt", "resolved\n", "resolve");
    assert_eq!(repo.commit(&merged)["parents"], json!([ours, theirs]));
}

#[test]
fn unresolved_conflicts_cannot_be_committed_with_unrelated_staged_changes() {
    let repo = TestRepo::new();
    let (ours, theirs) = repo.conflicting_merge();
    repo.write("other.txt", "unrelated\n");
    repo.run(&["add", "other.txt"]);
    let output = repo.output(&["commit", "-m", "unfinished"]);
    assert!(
        !output.status.success(),
        "unresolved merge must not create a commit"
    );
    assert_eq!(repo.head(), ours);
    let merged = repo.commit_file("hello.txt", "resolved\n", "resolve");
    assert_eq!(repo.commit(&merged)["parents"], json!([ours, theirs]));
}

#[test]
fn resolving_conflict_by_deleting_file_keeps_both_parents() {
    let repo = TestRepo::new();
    let (ours, theirs) = repo.conflicting_merge();
    repo.run(&["rm", "hello.txt"]);
    repo.run(&["commit", "-m", "resolve by deletion"]);
    let commit = repo.commit(&repo.head());
    assert_eq!(commit["parents"], json!([ours, theirs]));
    assert!(commit["tree"].as_object().unwrap().is_empty());
}

#[test]
fn checkout_and_reset_discard_pending_merge_state() {
    for action in ["branch", "detached", "reset"] {
        let repo = TestRepo::new();
        let (ours, _) = repo.conflicting_merge();
        match action {
            "branch" => repo.run(&["checkout", "main"]),
            "detached" => repo.run(&["checkout", &ours]),
            _ => repo.run(&["reset", "--hard", &ours]),
        };
        let next = repo.commit_file("hello.txt", "after abort\n", "ordinary");
        assert_eq!(repo.commit(&next)["parents"], json!([ours]), "{action}");
        assert!(!repo.run(&["status"]).contains("Merge conflicts:"));
    }
}

#[test]
fn automatic_merge_and_fast_forward_keep_their_existing_parent_semantics() {
    let repo = TestRepo::new();
    repo.run(&["branch", "feature"]);
    let ours = repo.commit_file("ours.txt", "ours\n", "ours");
    repo.run(&["checkout", "feature"]);
    let theirs = repo.commit_file("theirs.txt", "theirs\n", "theirs");
    repo.run(&["checkout", "main"]);
    repo.run(&["merge", "feature"]);
    let merged = repo.head();
    assert_eq!(repo.commit(&merged)["parents"], json!([ours, theirs]));
    repo.run(&["checkout", "feature"]);
    assert!(repo.run(&["merge", "main"]).contains("fast-forward"));
    assert_eq!(repo.head(), merged);
    let next = repo.commit_file("next.txt", "next\n", "ordinary");
    assert_eq!(repo.commit(&next)["parents"], json!([merged]));
}
