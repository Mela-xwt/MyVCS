use std::{
    fs,
    path::PathBuf,
    process::{Command, Output},
    sync::atomic::{AtomicU64, Ordering},
};

static NEXT: AtomicU64 = AtomicU64::new(0);
struct Sandbox(PathBuf);
impl Sandbox {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "myvcs-local-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&path).unwrap();
        Self(path)
    }
    fn output(&self, args: &[&str]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_myvcs"))
            .args(args)
            .current_dir(&self.0)
            .env("MYVCS_FORCE_DEEP_DELTA", "1")
            .output()
            .unwrap()
    }
    fn run(&self, args: &[&str]) -> String {
        let out = self.output(args);
        assert!(
            out.status.success(),
            "{args:?}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8(out.stdout).unwrap()
    }
    fn head(&self) -> String {
        fs::read_to_string(self.0.join(".myvcs/refs/heads/main"))
            .unwrap()
            .trim()
            .to_owned()
    }
}
impl Drop for Sandbox {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[test]
fn empty_and_explicit_repositories_are_usable() {
    let dir = Sandbox::new();
    dir.run(&["init", "--empty", "one"]);
    dir.run(&["init", "--empty", "two"]);
    assert!(!dir.0.join("one/.myvcs/refs/heads/main").exists());
    assert!(dir.run(&["--repo", "one", "log"]).contains("no commits"));
    fs::write(dir.0.join("one/file.txt"), "one").unwrap();
    dir.run(&["--repo", "one", "add", "file.txt"]);
    dir.run(&["--repo", "one", "commit", "-m", "first"]);
    dir.run(&["--repo", "one", "verify"]);
    assert!(dir.run(&["--repo", "two", "log"]).contains("no commits"));
    dir.run(&["--repo", "two", "gc"]);
}

#[test]
fn bare_repository_rejects_worktree_commands() {
    let dir = Sandbox::new();
    dir.run(&["init", "--bare", "server"]);
    assert!(dir.0.join("server/HEAD").exists());
    assert!(!dir.0.join("server/index.json").exists());
    dir.run(&["--repo", "server", "verify"]);
    dir.run(&["--repo", "server", "gc"]);
    assert!(
        !dir.output(&["--repo", "server", "add", "anything"])
            .status
            .success()
    );
    assert!(!dir.output(&["--repo", "server", "status"]).status.success());
}

#[test]
fn gc_preserves_staged_objects() {
    let dir = Sandbox::new();
    dir.run(&["init"]);
    fs::write(dir.0.join("file.txt"), "staged only").unwrap();
    dir.run(&["add", "file.txt"]);
    dir.run(&["gc"]);
    dir.run(&["commit", "-m", "after gc"]);
    dir.run(&["verify"]);
}

#[test]
fn gc_preserves_remote_and_nested_refs() {
    let dir = Sandbox::new();
    dir.run(&["init"]);
    let initial = dir.head();
    fs::write(dir.0.join("file.txt"), "remote history").unwrap();
    dir.run(&["add", "file.txt"]);
    dir.run(&["commit", "-m", "remote"]);
    let remote = dir.head();
    fs::create_dir_all(dir.0.join(".myvcs/refs/remotes/origin")).unwrap();
    fs::write(dir.0.join(".myvcs/refs/remotes/origin/main"), &remote).unwrap();
    dir.run(&["reset", "--hard", &initial]);
    dir.run(&["gc"]);
    dir.run(&["checkout", &remote]);
    dir.run(&["branch", "feature/nested"]);
    fs::remove_file(dir.0.join(".myvcs/refs/remotes/origin/main")).unwrap();
    dir.run(&["checkout", "main"]);
    dir.run(&["gc"]);
    dir.run(&["checkout", "feature/nested"]);
    assert_eq!(
        fs::read_to_string(dir.0.join("file.txt")).unwrap(),
        "remote history"
    );
    dir.run(&["verify"]);
}

#[test]
fn raw_delta_magic_file_roundtrips() {
    let dir = Sandbox::new();
    dir.run(&["init"]);
    let initial = dir.head();
    let data = b"MYVCS_DELTA_V1\\nnot a delta object";
    fs::write(dir.0.join("data.bin"), data).unwrap();
    dir.run(&["add", "data.bin"]);
    dir.run(&["commit", "-m", "literal magic"]);
    let tip = dir.head();
    dir.run(&["verify"]);
    dir.run(&["reset", "--hard", &initial]);
    dir.run(&["checkout", &tip]);
    assert_eq!(fs::read(dir.0.join("data.bin")).unwrap(), data);
}

#[test]
fn invalid_reference_cannot_escape_metadata() {
    let dir = Sandbox::new();
    dir.run(&["init"]);
    for name in [
        "../../escape",
        "a/../escape",
        "a//b",
        ".hidden",
        "bad.lock",
        "a\\b",
    ] {
        assert!(
            !dir.output(&["branch", name]).status.success(),
            "accepted {name}"
        );
    }
    assert!(!dir.0.join(".myvcs/escape").exists());
    assert!(!dir.output(&["checkout", "中a"]).status.success());
}

#[cfg(unix)]
#[test]
fn worktree_symlinks_are_not_followed() {
    let dir = Sandbox::new();
    dir.run(&["init"]);
    let outside = Sandbox::new();
    fs::write(outside.0.join("secret"), "outside").unwrap();
    std::os::unix::fs::symlink(outside.0.join("secret"), dir.0.join("link")).unwrap();
    assert!(!dir.output(&["add", "link"]).status.success());
    assert_eq!(
        fs::read_to_string(outside.0.join("secret")).unwrap(),
        "outside"
    );
}

#[cfg(unix)]
#[test]
fn recursive_add_rejects_nonportable_names_without_aliasing_them() {
    let dir = Sandbox::new();
    dir.run(&["init"]);
    fs::write(dir.0.join("wrong\\name"), "data").unwrap();
    assert!(!dir.output(&["add", "."]).status.success());
    assert!(dir.run(&["status"]).contains("No staged changes"));
}

#[test]
fn unsafe_paths_and_oids_fail_without_partial_deletion() {
    let dir = Sandbox::new();
    dir.run(&["init"]);
    fs::write(dir.0.join("safe"), "keep").unwrap();
    dir.run(&["add", "safe"]);
    dir.run(&["commit", "-m", "safe"]);
    for path in [
        "../escape",
        ".myvcs/HEAD",
        "nested/.myvcs/HEAD",
        "/tmp/escape",
        "C:/escape",
        "a/../safe",
    ] {
        assert!(!dir.output(&["rm", "safe", path]).status.success());
        assert_eq!(fs::read_to_string(dir.0.join("safe")).unwrap(), "keep");
    }
    for oid in [
        "中a".to_string(),
        "a".repeat(63),
        "g".repeat(64),
        "A".repeat(64),
    ] {
        let result = dir.output(&["reset", "--hard", &oid]);
        assert!(!result.status.success());
        assert!(!String::from_utf8_lossy(&result.stderr).contains("panicked"));
    }
}
