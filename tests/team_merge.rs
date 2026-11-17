use flate2::{Compression, read::ZlibDecoder, write::ZlibEncoder};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::io::Write;
use std::{
    fs,
    path::PathBuf,
    process::{Command, Output},
    sync::atomic::{AtomicU64, Ordering},
};
static NEXT: AtomicU64 = AtomicU64::new(0);
struct Lab(PathBuf);
impl Lab {
    fn new() -> Self {
        let p = std::env::temp_dir().join(format!(
            "myvcs-team-merge-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&p).unwrap();
        let l = Self(p);
        l.run(&["init"]);
        l
    }
    fn output(&self, args: &[&str]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_myvcs"))
            .args(args)
            .current_dir(&self.0)
            .output()
            .unwrap()
    }
    fn run(&self, args: &[&str]) -> String {
        let o = self.output(args);
        assert!(
            o.status.success(),
            "{args:?}: {}",
            String::from_utf8_lossy(&o.stderr)
        );
        String::from_utf8(o.stdout).unwrap()
    }
    fn fail(&self, args: &[&str], needle: &str) {
        let o = self.output(args);
        assert!(!o.status.success(), "unexpected success: {args:?}");
        assert!(
            String::from_utf8_lossy(&o.stderr).contains(needle),
            "{}",
            String::from_utf8_lossy(&o.stderr)
        );
    }
    fn write(&self, p: &str, b: &[u8]) {
        fs::write(self.0.join(p), b).unwrap();
    }
    fn read(&self, p: &str) -> Vec<u8> {
        fs::read(self.0.join(p)).unwrap()
    }
    fn commit(&self, b: &[u8]) -> String {
        self.write("f", b);
        self.run(&["add", "f"]);
        self.run(&["commit", "-m", "change"]);
        self.head()
    }
    fn head(&self) -> String {
        fs::read_to_string(self.0.join(".myvcs/refs/heads/main"))
            .unwrap()
            .trim()
            .into()
    }
    fn diverge(&self, base: &[u8], ours: &[u8], theirs: &[u8]) -> (String, String) {
        self.commit(base);
        self.run(&["branch", "topic"]);
        let o = self.commit(ours);
        self.run(&["checkout", "topic"]);
        self.commit(theirs);
        let t = fs::read_to_string(self.0.join(".myvcs/refs/heads/topic"))
            .unwrap()
            .trim()
            .into();
        self.run(&["checkout", "main"]);
        (o, t)
    }
    fn object(&self, id: &str) -> Value {
        serde_json::from_reader(ZlibDecoder::new(
            fs::File::open(
                self.0
                    .join(format!(".myvcs/objects/{}/{}", &id[..2], &id[2..])),
            )
            .unwrap(),
        ))
        .unwrap()
    }
    fn put(&self, value: Value) -> String {
        let bytes = serde_json::to_vec(&value).unwrap();
        let id = format!("{:x}", Sha256::digest(&bytes));
        let p = self
            .0
            .join(format!(".myvcs/objects/{}/{}", &id[..2], &id[2..]));
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        let mut z = ZlibEncoder::new(Vec::new(), Compression::default());
        z.write_all(&bytes).unwrap();
        fs::write(p, z.finish().unwrap()).unwrap();
        id
    }
}
impl Drop for Lab {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[test]
fn disjoint_lines_merge_and_record_two_parents() {
    let l = Lab::new();
    let (o, t) = l.diverge(b"a\nb\nc\n", b"A\nb\nc\n", b"a\nb\nC\n");
    l.run(&["merge", "topic"]);
    assert_eq!(l.read("f"), b"A\nb\nC\n");
    assert_eq!(l.object(&l.head())["parents"], json!([o, t]));
}
#[test]
fn identical_edits_and_crlf_without_final_newline() {
    let l = Lab::new();
    l.diverge(b"a\r\nb\r\nc", b"A\r\nb\r\nc", b"a\r\nb\r\nC");
    l.run(&["merge", "topic"]);
    assert_eq!(l.read("f"), b"A\r\nb\r\nC");
    let l = Lab::new();
    l.diverge(b"a\nb\nc\n", b"A\nb\nc\n", b"A\nb\nC\n");
    l.run(&["merge", "topic"]);
    assert_eq!(l.read("f"), b"A\nb\nC\n");
}
#[test]
fn overlap_resolution_records_two_parents_and_abort_restores_exact_state() {
    let l = Lab::new();
    let (o, t) = l.diverge(b"a\nb\nc\n", b"A\nb\nc\n", b"T\nb\nC\n");
    let index = l.read(".myvcs/index.json");
    assert!(l.run(&["merge", "topic"]).contains("conflicts"));
    assert!(
        String::from_utf8(l.read("f"))
            .unwrap()
            .contains("<<<<<<< OURS")
    );
    l.write("unrelated", b"keep");
    l.run(&["merge", "--abort"]);
    assert_eq!(l.head(), o);
    assert_eq!(l.read("f"), b"A\nb\nc\n");
    assert_eq!(l.read(".myvcs/index.json"), index);
    assert_eq!(l.read("unrelated"), b"keep");
    l.run(&["merge", "topic"]);
    l.write("f", b"resolved\n");
    l.run(&["add", "f"]);
    l.run(&["commit", "-m", "resolve"]);
    assert_eq!(l.object(&l.head())["parents"], json!([o, t]));
    l.fail(&["merge", "--abort"], "no merge");
}
#[test]
fn binary_modify_delete_preserves_ours_and_aborts() {
    for ours_deleted in [false, true] {
        let l = Lab::new();
        l.commit(b"base\0");
        l.run(&["branch", "topic"]);
        if ours_deleted {
            l.run(&["rm", "f"]);
            l.run(&["commit", "-m", "delete"]);
        } else {
            l.commit(b"ours\0");
        }
        l.run(&["checkout", "topic"]);
        if ours_deleted {
            l.commit(b"theirs\0");
        } else {
            l.run(&["rm", "f"]);
            l.run(&["commit", "-m", "delete"]);
        }
        l.run(&["checkout", "main"]);
        let head = l.head();
        l.run(&["merge", "topic"]);
        assert_eq!(l.0.join("f").exists(), !ours_deleted);
        if !ours_deleted {
            assert_eq!(l.read("f"), b"ours\0");
        }
        let state: Value = serde_json::from_slice(&l.read(".myvcs/merge-state.json")).unwrap();
        assert_eq!(state["conflicts"]["f"]["kind"], "binary");
        l.run(&["merge", "--abort"]);
        assert_eq!(l.head(), head);
        assert_eq!(l.0.join("f").exists(), !ours_deleted);
    }
}
#[test]
fn dirty_staged_and_worktree_are_rejected_before_mutation() {
    for staged in [false, true] {
        let l = Lab::new();
        l.diverge(b"a\nb\nc\n", b"A\nb\nc\n", b"a\nb\nC\n");
        l.write("f", b"local");
        if staged {
            l.run(&["add", "f"]);
        }
        let h = l.head();
        let i = l.read(".myvcs/index.json");
        l.fail(
            &["merge", "topic"],
            if staged {
                "staged changes"
            } else {
                "local changes"
            },
        );
        assert_eq!(l.head(), h);
        assert_eq!(l.read("f"), b"local");
        assert_eq!(l.read(".myvcs/index.json"), i);
        assert!(!l.0.join(".myvcs/merge-state.json").exists());
    }
}
#[test]
fn fast_forward_untracked_and_structural_collisions_are_rejected() {
    for structural in [false, true] {
        let l = Lab::new();
        l.commit(b"a");
        l.run(&["branch", "topic"]);
        l.run(&["checkout", "topic"]);
        if structural {
            fs::create_dir(l.0.join("dir")).unwrap();
            l.write("dir/file", b"tracked");
            l.run(&["add", "dir/file"]);
        } else {
            l.write("new", b"tracked");
            l.run(&["add", "new"]);
        }
        l.run(&["commit", "-m", "new"]);
        l.run(&["checkout", "main"]);
        if structural {
            let _ = fs::remove_dir(l.0.join("dir"));
            l.write("dir", b"local");
        } else {
            l.write("new", b"local");
        }
        let h = l.head();
        l.fail(
            &["merge", "topic"],
            if structural {
                "file/directory"
            } else {
                "untracked"
            },
        );
        assert_eq!(l.head(), h);
        assert_eq!(l.read(if structural { "dir" } else { "new" }), b"local");
    }
}
#[test]
fn abort_recovers_interrupted_restore_and_rejects_changed_head() {
    let l = Lab::new();
    let (o, _) = l.diverge(b"base\n", b"ours\n", b"theirs\n");
    l.run(&["merge", "topic"]);
    let saved = l.read(".myvcs/merge-state.json");
    l.run(&["merge", "--abort"]);
    l.write(".myvcs/merge-state.json", &saved);
    l.run(&["merge", "--abort"]);
    assert_eq!(l.read("f"), b"ours\n");
    l.run(&["merge", "topic"]);
    l.write(
        ".myvcs/refs/heads/main",
        format!("{}\n", l.object(&o)["parents"][0].as_str().unwrap()).as_bytes(),
    );
    l.fail(&["merge", "--abort"], "HEAD changed");
    assert!(l.0.join(".myvcs/merge-state.json").exists());
}
#[test]
fn best_common_ancestor_rejects_criss_cross_without_mutation() {
    let l = Lab::new();
    let base = l.commit(b"a\n");
    let tree = l.object(&base)["tree"].clone();
    let mk = |parents: Vec<String>, m: &str| {
        l.put(json!({"tree":tree,"parents":parents,"author":"test","message":m,"timestamp":1}))
    };
    let a = mk(vec![base.clone()], "a");
    let b = mk(vec![base], "b");
    let x = mk(vec![a.clone(), b.clone()], "x");
    let y = mk(vec![b, a], "y");
    l.write(".myvcs/refs/heads/main", format!("{x}\n").as_bytes());
    l.write(".myvcs/refs/heads/topic", format!("{y}\n").as_bytes());
    l.fail(&["merge", "topic"], "multiple merge bases");
    assert_eq!(l.head(), x);
    assert_eq!(l.read("f"), b"a\n");
    assert!(!l.0.join(".myvcs/merge-state.json").exists());
}

#[test]
fn abort_restores_deleted_and_created_paths_retains_unrelated_files() {
    let l = Lab::new();
    l.commit(b"base\n");
    l.write("gone", b"original");
    l.run(&["add", "gone"]);
    l.run(&["commit", "-m", "base extra"]);
    l.run(&["branch", "topic"]);
    l.commit(b"ours\n");
    l.run(&["checkout", "topic"]);
    l.commit(b"theirs\n");
    l.run(&["rm", "gone"]);
    fs::create_dir(l.0.join("added")).unwrap();
    l.write("added/new", b"merge created");
    l.run(&["add", "added/new"]);
    l.run(&["commit", "-m", "clean changes"]);
    l.run(&["checkout", "main"]);
    let head = l.head();
    let index = l.read(".myvcs/index.json");
    l.run(&["merge", "topic"]);
    assert!(!l.0.join("gone").exists());
    assert_eq!(l.read("added/new"), b"merge created");
    l.write("added/unrelated", b"retain");
    l.write("untracked", b"retain too");
    l.run(&["merge", "--abort"]);
    assert_eq!(l.head(), head);
    assert_eq!(l.read(".myvcs/index.json"), index);
    assert_eq!(l.read("gone"), b"original");
    assert!(!l.0.join("added/new").exists());
    assert_eq!(l.read("added/unrelated"), b"retain");
    assert_eq!(l.read("untracked"), b"retain too");
}

#[test]
fn abort_obstruction_is_non_destructive_and_retryable() {
    let l = Lab::new();
    l.diverge(b"base\n", b"ours\n", b"theirs\n");
    l.run(&["merge", "topic"]);
    fs::remove_file(l.0.join("f")).unwrap();
    fs::create_dir(l.0.join("f")).unwrap();
    l.write("f/unrelated", b"keep");
    l.fail(&["merge", "--abort"], "file/directory conflict");
    assert_eq!(l.read("f/unrelated"), b"keep");
    assert!(l.0.join(".myvcs/merge-state.json").exists());
    fs::rename(l.0.join("f"), l.0.join("saved")).unwrap();
    l.run(&["merge", "--abort"]);
    assert_eq!(l.read("f"), b"ours\n");
    assert_eq!(l.read("saved/unrelated"), b"keep");
}

#[test]
fn bounded_text_merge_preserves_ours_and_records_limit_conflict() {
    let l = Lab::new();
    let base = "x\n".repeat(2100);
    let ours = format!("ours\n{base}");
    let theirs = format!("theirs\n{base}");
    l.diverge(base.as_bytes(), ours.as_bytes(), theirs.as_bytes());
    l.run(&["merge", "topic"]);
    assert_eq!(l.read("f"), ours.as_bytes());
    let state: Value = serde_json::from_slice(&l.read(".myvcs/merge-state.json")).unwrap();
    assert_eq!(state["conflicts"]["f"]["kind"], "text-limit");
    l.run(&["merge", "--abort"]);
}

#[test]
fn journal_roots_survive_gc_before_merge_head_is_written() {
    let l = Lab::new();
    l.diverge(b"base\n", b"ours\n", b"theirs\n");
    l.run(&["merge", "topic"]);
    fs::remove_file(l.0.join(".myvcs/MERGE_HEAD")).unwrap();
    fs::remove_file(l.0.join(".myvcs/refs/heads/topic")).unwrap();
    l.run(&["gc"]);
    l.run(&["verify"]);
    l.run(&["merge", "--abort"]);
    assert_eq!(l.read("f"), b"ours\n");
}

#[test]
fn delete_versus_empty_text_file_is_still_a_conflict() {
    let l = Lab::new();
    l.commit(b"base\n");
    l.run(&["branch", "topic"]);
    l.run(&["rm", "f"]);
    l.run(&["commit", "-m", "delete"]);
    l.run(&["checkout", "topic"]);
    l.commit(b"");
    l.run(&["checkout", "main"]);
    assert!(l.run(&["merge", "topic"]).contains("conflicts"));
    assert!(l.read("f").starts_with(b"<<<<<<< OURS\n"));
    l.run(&["merge", "--abort"]);
    assert!(!l.0.join("f").exists());
}

#[test]
fn binary_deleted_ours_can_resolve_as_deletion_with_both_parents() {
    let l = Lab::new();
    l.commit(b"base\0");
    l.run(&["branch", "topic"]);
    l.run(&["rm", "f"]);
    l.run(&["commit", "-m", "delete"]);
    let ours = l.head();
    l.run(&["checkout", "topic"]);
    l.commit(b"theirs\0");
    let theirs = fs::read_to_string(l.0.join(".myvcs/refs/heads/topic"))
        .unwrap()
        .trim()
        .to_owned();
    l.run(&["checkout", "main"]);
    l.run(&["merge", "topic"]);
    assert!(!l.0.join("f").exists());
    l.fail(&["rm", "unknown"], "path is not tracked");
    l.run(&["rm", "f"]);
    l.run(&["commit", "-m", "resolve as deletion"]);
    assert_eq!(l.object(&l.head())["parents"], json!([ours, theirs]));
    assert!(l.object(&l.head())["tree"].get("f").is_none());
    assert!(!l.0.join(".myvcs/merge-state.json").exists());
}

#[test]
fn journal_prepared_boundary_cannot_commit_an_unmaterialized_merge() {
    for merge_head_written in [false, true] {
        let l = Lab::new();
        l.commit(b"base\n");
        l.run(&["branch", "topic"]);
        let ours = l.commit(b"ours\n");
        l.run(&["checkout", "topic"]);
        l.commit(b"theirs\n");
        l.write("g", b"target only");
        l.run(&["add", "g"]);
        l.run(&["commit", "-m", "target file"]);
        l.run(&["checkout", "main"]);
        let index = l.read(".myvcs/index.json");
        l.run(&["merge", "topic"]);
        let mut state: Value = serde_json::from_slice(&l.read(".myvcs/merge-state.json")).unwrap();
        state["phase"] = json!("prepared");
        // Reconstruct the valid durable boundary after journal, optionally
        // MERGE_HEAD, but before conflict list, file restore, or merge index.
        l.write(
            ".myvcs/merge-state.json",
            &serde_json::to_vec(&state).unwrap(),
        );
        l.write(".myvcs/index.json", &index);
        l.write("f", b"ours\n");
        fs::remove_file(l.0.join("g")).unwrap();
        fs::remove_file(l.0.join(".myvcs/merge-conflicts.txt")).unwrap();
        if !merge_head_written {
            fs::remove_file(l.0.join(".myvcs/MERGE_HEAD")).unwrap();
        }
        l.write("note", b"unrelated staged");
        l.run(&["add", "note"]);
        l.fail(&["commit", "-m", "must refuse"], "incomplete merge");
        assert_eq!(l.head(), ours);
        assert!(l.0.join(".myvcs/merge-state.json").exists());
        l.run(&["merge", "--abort"]);
        assert_eq!(l.read("f"), b"ours\n");
        assert_eq!(l.read("note"), b"unrelated staged");
        assert!(!l.0.join("g").exists());
        assert_eq!(l.read(".myvcs/index.json"), index);
    }
}

#[test]
fn materialized_journal_keeps_conflicts_authoritative() {
    let l = Lab::new();
    let (ours, _) = l.diverge(b"base\n", b"ours\n", b"theirs\n");
    l.run(&["merge", "topic"]);
    let state: Value = serde_json::from_slice(&l.read(".myvcs/merge-state.json")).unwrap();
    assert_eq!(state["phase"], "materialized");
    fs::remove_file(l.0.join(".myvcs/merge-conflicts.txt")).unwrap();
    l.write("note", b"local");
    l.run(&["add", "note"]);
    l.fail(&["commit", "-m", "unresolved"], "unresolved merge conflict");
    assert_eq!(l.head(), ours);
    l.run(&["merge", "--abort"]);
    assert_eq!(l.read("note"), b"local");
}
