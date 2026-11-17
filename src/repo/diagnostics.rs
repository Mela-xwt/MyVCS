use super::*;
use serde_json::{Value, json};

impl Repo {
    pub fn doctor(repo: Option<&Repo>) -> Result<Value> {
        let path = repo
            .map(|r| r.root.clone())
            .unwrap_or(std::env::current_dir()?);
        let os = fs::read_to_string("/etc/os-release").ok().and_then(|s| {
            s.lines().find_map(|l| {
                l.strip_prefix("PRETTY_NAME=")
                    .map(|s| s.trim_matches('"').to_owned())
            })
        });
        let kernel = fs::read_to_string("/proc/sys/kernel/osrelease")
            .ok()
            .map(|s| s.trim().to_owned());
        let numa = fs::read_dir("/sys/devices/system/node").ok().map(|es| {
            es.filter_map(Result::ok)
                .filter(|e| {
                    e.file_name()
                        .to_str()
                        .and_then(|s| s.strip_prefix("node"))
                        .and_then(|s| s.parse::<usize>().ok())
                        .is_some()
                })
                .count()
        });
        let psi = if cfg!(target_os = "linux") {
            Some(Path::new("/proc/pressure/memory").exists())
        } else {
            None
        };
        Ok(json!({
            "schema_version":1,
            "system":{"os":os,"platform":std::env::consts::OS,"kernel":kernel,"architecture":std::env::consts::ARCH},
            "filesystem":{"path":path,"type":crate::util::detect_fs_type(&path).to_string()},
            "capabilities":{"numa_nodes":numa,"psi_memory_file":psi,"etmem":null,
                "etmem_note":"No reliable runtime integration probe; presence is not proof of use",
                "mmap_compiled":true,"posix_fadvise_compiled":cfg!(target_os="linux"),"madvise_compiled":cfg!(target_os="linux"),
                "runtime_io_hints_verified":null},
            "io":{"buffer_bytes":crate::util::io_chunk_size_bytes(),"buffer_probe_path":std::env::current_dir()?,
                "note":"Reports the existing process policy; automatic filesystem probe uses process cwd",
                "large_file_compression":std::env::var("MYVCS_LARGE_FILE_COMPRESSION").unwrap_or_else(|_| "auto".into())},
            "limits":{"object_bytes":ReadLimits::default().object_bytes,"read_chain_nodes":ReadLimits::default().delta_depth,"new_gc_chain_nodes":8}
        }))
    }

    pub fn stats(&self) -> Result<Value> {
        let refs = self.list_refs()?;
        let all = self.all_object_ids()?;
        let (reachable, commits, blobs) = self.protected_objects()?;
        let mut bytes = 0u64;
        let mut delta = 0usize;
        let mut depth = 0usize;
        for oid in &all {
            bytes = bytes
                .checked_add(fs::metadata(self.object_path(oid)?)?.len())
                .context("object byte count overflow")?;
            if reachable.contains(oid) {
                let mut seen = BTreeSet::new();
                let mut id = oid.clone();
                loop {
                    if !seen.insert(id.clone()) || seen.len() > self.limits.delta_depth {
                        bail!("invalid delta chain");
                    }
                    match self.stored_delta(&id, &self.read_object(&id)?)? {
                        Some(d) => {
                            if seen.len() == 1 {
                                delta += 1;
                            }
                            id = d.base;
                        }
                        None => break,
                    }
                }
                depth = depth.max(seen.len());
            }
        }
        let idx = if self.bare {
            Index::default()
        } else {
            self.read_index()?
        };
        Ok(
            json!({"schema_version":1,"commits_reachable":commits,"blob_references":blobs,
            "local_branches":refs.keys().filter(|n| n.starts_with("refs/heads/")).count(),
            "remote_branches":refs.keys().filter(|n| n.starts_with("refs/remotes/")).count(),
            "tags":refs.keys().filter(|n| n.starts_with("refs/tags/")).count(),
            "stashes":refs.keys().filter(|n| n.starts_with("refs/pins/stash/")).count(),
            "objects_total":all.len(),"objects_reachable":reachable.len(),"object_storage_bytes":bytes,
            "index_entries":idx.entries.len(),"index_removed":idx.removed.len(),
            "reachable_delta_objects":delta,"max_reachable_chain_nodes":depth,
            "notes":"Counts include pins, stash synthetic commits, retained reflog and physical delta bases. Bytes are object file lengths, not disk allocation. Depth includes full base (full=1)."}),
        )
    }
}
