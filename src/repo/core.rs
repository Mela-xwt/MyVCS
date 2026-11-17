use super::*;
use sha2::{Digest, Sha256};
use std::sync::{Arc, Mutex};

use crate::util::{self, advise_file_dontneed, advise_file_sequential, detect_fs_type};

impl Repo {
    const OBJECT_IO_HINT_THRESHOLD: u64 = 1024 * 1024;

    pub fn discover() -> Result<Self> {
        let cwd = std::env::current_dir()?;
        for cur in cwd.ancestors() {
            if cur.join(META_DIR).exists() || cur.join("format.json").is_file() {
                return Self::open(cur);
            }
        }
        bail!("not a myvcs repository (run `myvcs init` first)")
    }

    /// Opens exactly the supplied worktree, .myvcs directory, or bare repository.
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        let path = fs::canonicalize(path.as_ref()).context("repository path does not exist")?;
        let (root, meta) = if path.file_name().is_some_and(|s| s == META_DIR) {
            (
                path.parent()
                    .context("invalid metadata path")?
                    .to_path_buf(),
                path.clone(),
            )
        } else if path.join(META_DIR).exists() {
            (path.clone(), path.join(META_DIR))
        } else {
            (path.clone(), path)
        };
        no_symlinks(&root, &meta)?;
        let mut repo = Self {
            root,
            meta,
            bare: false,
            limits: ReadLimits::default(),
            overlay: None,
            commit_graph: Arc::new(Mutex::new(None)),
        };
        let format = repo.meta.join("format.json");
        repo.check_internal_path(&format)?;
        if format.exists() {
            let value: serde_json::Value = serde_json::from_str(&repo.read_metadata(&format)?)?;
            if value["version"] != 1 && value["version"] != 2 {
                bail!("unsupported repository format");
            }
            repo.bare = value["bare"].as_bool().context("invalid bare flag")?;
        } else if repo.meta == repo.root {
            bail!("not a myvcs repository");
        }
        if repo.bare != (repo.meta == repo.root) {
            bail!("repository layout does not match format");
        }
        let mut required_dirs = vec![repo.objects_dir()];
        if repo.format_version()? == 1 {
            required_dirs.push(repo.meta.join("refs"));
        }
        for dir in required_dirs {
            repo.check_internal_path(&dir)?;
            if !dir.is_dir() {
                bail!("incomplete repository: {}", dir.display());
            }
        }
        repo.head_commit_id()?;
        Ok(repo)
    }

    pub fn with_repo_lock<T, F>(&self, f: F) -> Result<T>
    where
        F: FnOnce(&Self) -> Result<T>,
    {
        self.with_repo_lock_unchecked(|r| {
            r.no_pending_work()?;
            f(r)
        })
    }

    pub(super) fn with_repo_lock_unchecked<T, F>(&self, f: F) -> Result<T>
    where
        F: FnOnce(&Self) -> Result<T>,
    {
        // 仓库级单锁保证关键元数据写入串行化。
        let lock_path = self.meta.join("repo.lock");
        self.check_internal_path(&lock_path)?;
        crate::debug_log!("acquire lock {}", lock_path.display());
        let lock = OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(false)
            .open(&lock_path)
            .with_context(|| format!("failed to open lock file {}", lock_path.display()))?;
        Self::lock_repo_file(&lock)?;
        let result = self.format_version().and_then(|_| f(self));
        let unlock_result = Self::unlock_repo_file(&lock);
        crate::debug_log!("release lock {}", lock_path.display());
        if let Err(e) = unlock_result {
            eprintln!("warning: failed to release lock: {}", e);
        }
        result
    }

    fn lock_repo_file(lock: &File) -> Result<()> {
        #[cfg(unix)]
        {
            Self::lock_repo_file_fcntl(lock)?;
        }
        lock.lock_exclusive()
            .context("failed to acquire repository lock")?;
        Ok(())
    }

    fn unlock_repo_file(lock: &File) -> Result<()> {
        #[cfg(unix)]
        {
            Self::unlock_repo_file_fcntl(lock)?;
        }
        lock.unlock().context("failed to release repository lock")?;
        Ok(())
    }

    #[cfg(unix)]
    fn lock_repo_file_fcntl(lock: &File) -> Result<()> {
        use std::os::fd::AsRawFd;

        let mut fl = libc::flock {
            l_type: libc::F_WRLCK as libc::c_short,
            l_whence: libc::SEEK_SET as libc::c_short,
            l_start: 0,
            l_len: 0,
            l_pid: 0,
        };

        let rc = unsafe { libc::fcntl(lock.as_raw_fd(), libc::F_SETLKW, &mut fl) };
        if rc == -1 {
            return Err(anyhow!(
                "failed to acquire repository lock (fcntl): {}",
                std::io::Error::last_os_error()
            ));
        }
        Ok(())
    }

    #[cfg(unix)]
    fn unlock_repo_file_fcntl(lock: &File) -> Result<()> {
        use std::os::fd::AsRawFd;

        let mut fl = libc::flock {
            l_type: libc::F_UNLCK as libc::c_short,
            l_whence: libc::SEEK_SET as libc::c_short,
            l_start: 0,
            l_len: 0,
            l_pid: 0,
        };

        let rc = unsafe { libc::fcntl(lock.as_raw_fd(), libc::F_SETLK, &mut fl) };
        if rc == -1 {
            return Err(anyhow!(
                "failed to release repository lock (fcntl): {}",
                std::io::Error::last_os_error()
            ));
        }
        Ok(())
    }

    pub fn init_here() -> Result<()> {
        Self::init_at(".", false, false).map(|_| ())
    }

    /// `empty` suppresses the legacy synthetic initial commit; bare implies empty.
    pub fn init_at(path: impl AsRef<Path>, bare: bool, empty: bool) -> Result<Self> {
        fs::create_dir_all(path.as_ref())?;
        let root = fs::canonicalize(path.as_ref())?;
        let meta = if bare {
            root.clone()
        } else {
            root.join(META_DIR)
        };
        no_symlinks(&root, &meta)?;
        fs::create_dir_all(&meta)?;
        let repo = Self {
            root,
            meta,
            bare,
            limits: ReadLimits::default(),
            overlay: None,
            commit_graph: Arc::new(Mutex::new(None)),
        };
        // Persist newly created repository directories before publishing HEAD.
        for ancestor in repo.meta.ancestors() {
            sync_directory(ancestor)?;
        }
        repo.with_repo_lock(|r| {
            r.check_internal_path(&r.meta.join("HEAD"))?;
            if r.meta.join("HEAD").exists() {
                let existing = Self::open(&r.meta)?;
                if existing.bare != bare {
                    bail!("repository kind differs");
                }
                println!("repository already initialized: {}", r.meta.display());
                return Ok(());
            }
            // Refuse to reinterpret arbitrary data as an unfinished repository.
            if fs::read_dir(&r.meta)?
                .any(|e| e.map(|e| e.file_name() != "repo.lock").unwrap_or(true))
            {
                bail!("metadata directory is not empty; refusing partial reinitialization");
            }
            r.create_dirs_durable(&r.objects_dir())?;
            r.create_dirs_durable(&r.meta.join("refs/heads"))?;
            r.atomic_write(
                &r.meta.join("format.json"),
                format!("{{\"version\":1,\"bare\":{bare}}}\n").as_bytes(),
            )?;
            if !bare {
                r.write_index(&Index::default())?;
            }
            if !bare && !empty {
                let commit = Commit {
                    tree: BTreeMap::new(),
                    parents: vec![],
                    parent: None,
                    message: "Initial commit".into(),
                    author: std::env::var("USER").unwrap_or_else(|_| "unknown".into()),
                    timestamp: now_unix() as i64,
                };
                let id = r.write_object_with_temp(
                    &serde_json::to_vec(&commit)?,
                    util::BlobTemperature::Hot,
                )?;
                r.update_ref_locked("refs/heads/main", None, &id)?;
            }
            // HEAD is the initialization commit point.
            r.atomic_write(&r.meta.join("HEAD"), b"ref: refs/heads/main\n")?;
            println!("initialized empty myvcs repository in {}", r.meta.display());
            Ok(())
        })?;
        Ok(repo)
    }

    pub(super) fn objects_dir(&self) -> PathBuf {
        self.meta.join("objects")
    }

    pub(super) fn index_path(&self) -> PathBuf {
        self.meta.join("index.json")
    }

    pub(super) fn status_cache_path(&self) -> PathBuf {
        self.meta.join("status-cache.json")
    }

    pub(super) fn merge_conflicts_path(&self) -> PathBuf {
        self.meta.join("merge-conflicts.txt")
    }

    pub(super) fn read_index(&self) -> Result<Index> {
        let p = self.index_path();
        if !p.exists() {
            return Ok(Index::default());
        }
        let data = self
            .read_metadata(&p)
            .with_context(|| format!("failed to read index {}", p.display()))?;
        let index: Index = serde_json::from_str(&data).context("invalid index format")?;
        self.validate_tree(&index.entries)?;
        for p in &index.removed {
            validate_tree_path(p)?;
        }
        Ok(index)
    }

    pub(super) fn write_index(&self, index: &Index) -> Result<()> {
        self.validate_tree(&index.entries)?;
        for p in &index.removed {
            validate_tree_path(p)?;
        }
        self.atomic_write_json(&self.index_path(), index)
    }

    pub(super) fn read_status_cache(&self) -> Result<StatusCache> {
        let p = self.status_cache_path();
        if !p.exists() {
            return Ok(StatusCache::default());
        }
        let data = self
            .read_metadata(&p)
            .with_context(|| format!("failed to read status cache {}", p.display()))?;
        Ok(serde_json::from_str(&data).context("invalid status cache format")?)
    }

    pub(super) fn write_status_cache(&self, cache: &StatusCache) -> Result<()> {
        self.atomic_write_json(&self.status_cache_path(), cache)
    }

    pub(super) fn read_merge_conflicts(&self) -> Result<Vec<String>> {
        let p = self.merge_conflicts_path();
        if !p.exists() {
            return Ok(Vec::new());
        }
        let raw = self
            .read_metadata(&p)
            .with_context(|| format!("failed to read merge conflict file {}", p.display()))?;
        let mut out = Vec::new();
        for line in raw.lines() {
            let v = line.trim();
            if !v.is_empty() {
                validate_tree_path(v)?;
                out.push(v.to_string());
            }
        }
        Ok(out)
    }

    pub(super) fn write_merge_conflicts(&self, conflicts: &[String]) -> Result<()> {
        let p = self.merge_conflicts_path();
        if conflicts.is_empty() {
            if p.exists() {
                fs::remove_file(&p).with_context(|| {
                    format!("failed to remove merge conflict file {}", p.display())
                })?;
            }
            return Ok(());
        }

        let mut body = String::new();
        for c in conflicts {
            body.push_str(c);
            body.push('\n');
        }
        self.atomic_write(&p, body.as_bytes())
            .with_context(|| format!("failed to write merge conflict file {}", p.display()))
    }

    // 冲突文件列表不能代替合并父节点；跨命令保存合并目标的提交 ID。
    pub(super) fn read_merge_head(&self) -> Result<Option<String>> {
        let path = self.meta.join("MERGE_HEAD");
        self.check_internal_path(&path)?;
        if !path.exists() {
            return Ok(None);
        }
        let raw = self.read_metadata(&path)?;
        let id = raw.trim();
        validate_oid(id)?;
        Ok(Some(id.to_string()))
    }

    pub(super) fn clear_merge_state(&self) -> Result<()> {
        self.write_merge_conflicts(&[])?;
        // Recovery journal is the final file removed. If cleanup is interrupted,
        // another abort can repeat all restoration steps from the same snapshot.
        for name in ["MERGE_HEAD", "merge-state.json"] {
            let path = self.meta.join(name);
            self.check_internal_path(&path)?;
            match fs::remove_file(path) {
                Ok(()) => sync_directory(&self.meta)?,
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => return Err(e).with_context(|| format!("failed to remove {name}")),
            }
        }
        Ok(())
    }

    pub(super) fn head_raw(&self) -> Result<String> {
        if self.format_version()? == 2 {
            return Ok(self.refs_snapshot()?.head);
        }
        self.head_raw_legacy()
    }

    pub(super) fn head_raw_legacy(&self) -> Result<String> {
        let head_path = self.meta.join("HEAD");
        let s = self
            .read_metadata(&head_path)
            .with_context(|| format!("failed to read HEAD {}", head_path.display()))?;
        Ok(s.trim().to_string())
    }

    pub(super) fn current_ref(&self) -> Result<Option<String>> {
        let raw = self.head_raw()?;
        if let Some(rest) = raw.strip_prefix("ref:") {
            let name = rest.trim();
            validate_ref(name)?;
            if !name.starts_with("refs/heads/") {
                bail!("HEAD must point to a local branch");
            }
            return Ok(Some(name.to_string()));
        }
        Ok(None)
    }

    pub fn head_commit_id(&self) -> Result<Option<String>> {
        let snapshot = self.refs_snapshot()?;
        if let Some(name) = snapshot.current_ref() {
            validate_ref(name)?;
            if !name.starts_with("refs/heads/") {
                bail!("HEAD must point to a local branch");
            }
        } else {
            validate_oid(&snapshot.head)?;
        }
        Ok(snapshot.head_commit_id())
    }

    pub(super) fn update_head_to_commit(&self, id: &str) -> Result<()> {
        if self.format_version()? == 2 {
            let p = self.prepare_head_commit_locked(id, "update HEAD")?.unwrap();
            return self.publish_refs_locked(p);
        }
        if let Some(r) = self.current_ref()? {
            self.update_ref_locked(&r, self.read_ref(&r)?.as_deref(), id)?;
        } else {
            self.validate_closure(id)?;
            self.atomic_write(&self.meta.join("HEAD"), format!("{id}\n").as_bytes())?;
            *self.commit_graph.lock().unwrap() = None;
        }
        Ok(())
    }

    pub fn resolve_commit(&self, name: &str) -> Result<String> {
        if name == "HEAD" {
            return self.head_commit_id()?.context("HEAD has no commits");
        }
        if validate_oid(name).is_ok() && self.has_object(name)? {
            self.read_commit(name)?;
            return Ok(name.to_owned());
        }
        let refs = if name.starts_with("refs/") {
            vec![name.to_owned()]
        } else {
            vec![
                format!("refs/heads/{name}"),
                format!("refs/remotes/{name}"),
                format!("refs/tags/{name}"),
            ]
        };
        let snapshot = self.refs_snapshot()?;
        let mut found = Vec::new();
        for r in refs {
            validate_ref(&r)?;
            if let Some(id) = snapshot.refs.get(&r) {
                found.push(id.clone());
            }
        }
        if found.len() > 1 {
            bail!("ambiguous reference: {name}; use a full ref name");
        }
        let id = found
            .pop()
            .with_context(|| format!("unknown commit or branch: {name}"))?;
        self.read_commit(&id)?;
        Ok(id)
    }

    pub(super) fn has_object(&self, hash: &str) -> Result<bool> {
        let p = self.object_path(hash)?;
        Ok(p.exists())
    }

    pub(super) fn object_path(&self, hash: &str) -> Result<PathBuf> {
        validate_oid(hash)?;
        let (dir, file) = hash.split_at(2);
        let path = self.objects_dir().join(dir).join(file);
        self.check_internal_path(&path)?;
        Ok(path)
    }

    /// 写入对象（默认压缩级别）。
    #[allow(dead_code)]
    pub(super) fn write_object(&self, bytes: &[u8]) -> Result<String> {
        self.write_object_with_temp(bytes, crate::util::BlobTemperature::Warm)
    }

    /// 写入对象，按温度自适应选择 zlib 压缩级别。
    /// Hot→fast, Warm→default, Cold→best。
    pub(super) fn write_object_with_temp(
        &self,
        bytes: &[u8],
        temp: crate::util::BlobTemperature,
    ) -> Result<String> {
        if bytes.len() > self.limits.object_bytes {
            bail!("object size limit exceeded");
        }
        let hash = hex_sha256(bytes);
        let p = self.object_path(&hash)?;
        if p.exists() {
            self.read_blob_resolved(&hash)?;
            return Ok(hash);
        }

        if let Some(parent) = p.parent() {
            self.create_dirs_durable(parent)?;
        }

        let level = crate::util::adaptive_compression(temp);
        // 预分配压缩缓冲区：zlib 典型压缩率 30%-70%，取 2/3 容量
        let estimated = bytes.len() * 2 / 3 + 64;
        let mut encoder = ZlibEncoder::new(Vec::with_capacity(estimated), level);
        encoder.write_all(bytes)?;
        let compressed = encoder.finish()?;
        self.atomic_write_object(&p, &compressed)?;

        Ok(hash)
    }

    pub(super) fn write_object_from_path(&self, path: &Path) -> Result<String> {
        self.check_internal_path(path)?;
        let meta = fs::metadata(path)
            .with_context(|| format!("failed to stat file {}", path.display()))?;
        if meta.len() > self.limits.object_bytes as u64 {
            bail!("object size limit exceeded");
        }
        if meta.len() < crate::util::LARGE_FILE_THRESHOLD {
            let bytes = read_file_for_blob(path)
                .with_context(|| format!("failed to read file {}", path.display()))?;
            // 新暂存文件为 Hot，使用快速压缩
            return self.write_object_with_temp(&bytes, crate::util::BlobTemperature::Hot);
        }

        let mut f =
            File::open(path).with_context(|| format!("failed to open file {}", path.display()))?;
        let compression = crate::util::large_file_compression(&mut f, meta.len())?;
        advise_file_sequential(&f);

        // openEuler 优化：流式写入使用对象库所在 FS 对齐缓冲区
        let chunk = if crate::util::should_enable_openeuler_io_hints() {
            let objects_dir = self.objects_dir();
            detect_fs_type(&objects_dir).optimal_write_buffer()
        } else {
            crate::util::io_chunk_size_bytes()
        };

        let tmp_dir = self.objects_dir().join("tmp");
        self.create_dirs_durable(&tmp_dir)?;
        let tmp_path = tmp_dir.join(format!("stream-{}", unique_suffix()));
        let tmp_file = OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&tmp_path)?;
        let estimated = meta
            .len()
            .saturating_mul(2)
            .saturating_div(3)
            .saturating_add(64);
        if let Some(prealloc) = crate::util::eulerfs_prealloc_len(&self.objects_dir(), estimated) {
            crate::util::preallocate_file(&tmp_file, prealloc);
        }
        let mut encoder = ZlibEncoder::new(tmp_file, compression);

        let mut total = 0usize;
        let mut hasher = Sha256::new();
        let mut buf = vec![0u8; chunk];
        loop {
            let n = f.read(&mut buf[..])?;
            if n == 0 {
                break;
            }
            total = total.checked_add(n).context("object size overflow")?;
            if total > self.limits.object_bytes {
                bail!("object size limit exceeded");
            }
            hasher.update(&buf[..n]);
            encoder.write_all(&buf[..n])?;
        }

        let mut tmp_file = encoder.finish()?;
        tmp_file.flush()?;
        use std::io::Seek;
        let compressed_len = tmp_file.stream_position()?;
        tmp_file.set_len(compressed_len)?;
        tmp_file.sync_all()?;
        drop(tmp_file);
        advise_file_dontneed(&f);

        let hash = crate::util::digest_to_hex(&hasher.finalize());
        let p = self.object_path(&hash)?;
        if p.exists() {
            let _ = fs::remove_file(&tmp_path);
            self.read_blob_resolved(&hash)?;
            return Ok(hash);
        }

        if let Some(parent) = p.parent() {
            self.create_dirs_durable(parent)?;
        }

        if let Err(e) = fs::rename(&tmp_path, &p) {
            if p.exists() {
                let _ = fs::remove_file(&tmp_path);
            } else {
                return Err(e.into());
            }
        }

        sync_directory(p.parent().unwrap())?;
        sync_directory(&tmp_dir)?;
        Ok(hash)
    }

    pub(super) fn read_object(&self, hash: &str) -> Result<Vec<u8>> {
        let mut p = self.object_path(hash)?;
        if let Some(overlay) = &self.overlay {
            let staged = overlay.join(hash);
            self.check_internal_path(&staged)?;
            if staged.is_file() {
                p = staged;
            }
        }
        let mut f = File::open(&p).with_context(|| format!("object not found: {}", hash))?;
        let len = f.metadata().map(|m| m.len()).unwrap_or(0);
        if len >= Self::OBJECT_IO_HINT_THRESHOLD {
            advise_file_sequential(&f);
            // openEuler 优化：对大对象预读（WILLNEED）以减少缺页抖动
            crate::util::advise_hot_blob(&f);
        }

        let stored_limit = self
            .limits
            .object_bytes
            .saturating_mul(2)
            .saturating_add(65536);
        if len > stored_limit as u64 {
            bail!("stored object size limit exceeded");
        }
        use std::io::BufRead;
        let mut reader = std::io::BufReader::new(&mut f);
        let mut decoder = flate2::Decompress::new(true);
        let mut buffer = [0u8; 65536];
        let mut out = Vec::new();
        loop {
            let input = reader.fill_buf()?;
            let before_in = decoder.total_in();
            let before_out = decoder.total_out();
            let status = decoder.decompress(input, &mut buffer, flate2::FlushDecompress::None)?;
            let consumed = (decoder.total_in() - before_in) as usize;
            let produced = (decoder.total_out() - before_out) as usize;
            reader.consume(consumed);
            if out.len().saturating_add(produced) > self.limits.object_bytes {
                bail!("decompressed object size limit exceeded");
            }
            out.extend_from_slice(&buffer[..produced]);
            if status == flate2::Status::StreamEnd {
                break;
            }
            if consumed == 0 && produced == 0 {
                bail!("truncated zlib stream");
            }
        }
        // Old EulerFS posix_fallocate could leave trailing zeros. Accept only
        // zero padding after a fully validated stream, never a second payload.
        loop {
            let n = reader.read(&mut buffer)?;
            if n == 0 {
                break;
            }
            if buffer[..n].iter().any(|b| *b != 0) {
                bail!("nonzero trailing object data");
            }
        }
        drop(reader);

        if len >= Self::OBJECT_IO_HINT_THRESHOLD {
            advise_file_dontneed(&f);
        }
        Ok(out)
    }

    pub fn read_blob_resolved(&self, hash: &str) -> Result<Vec<u8>> {
        self.resolve_object_inner(hash, &mut BTreeSet::new(), &mut 0)
    }

    pub(super) fn stored_delta(&self, hash: &str, raw: &[u8]) -> Result<Option<DeltaBlob>> {
        // Object IDs identify the logical bytes, not the storage encoding.
        if hex_sha256(raw) == hash {
            return Ok(None);
        }
        let delta = decode_delta_blob(raw)?.context("object hash mismatch")?;
        validate_oid(&delta.base)?;
        if delta.size > self.limits.object_bytes || delta.ops.len() > self.limits.delta_ops {
            bail!("delta size or operation limit exceeded");
        }
        Ok(Some(delta))
    }

    fn resolve_object_inner(
        &self,
        hash: &str,
        seen: &mut BTreeSet<String>,
        budget: &mut usize,
    ) -> Result<Vec<u8>> {
        if seen.len() >= self.limits.delta_depth {
            bail!("delta depth limit exceeded");
        }
        if !seen.insert(hash.to_owned()) {
            bail!("delta chain cycle detected");
        }
        let raw = self.read_object(hash)?;
        *budget = budget
            .checked_add(raw.len())
            .context("decode budget overflow")?;
        if *budget > self.limits.chain_bytes {
            bail!("delta byte budget exceeded");
        }
        if let Some(delta) = self.stored_delta(hash, &raw)? {
            *budget = budget
                .checked_add(delta.size)
                .context("decode budget overflow")?;
            if *budget > self.limits.chain_bytes {
                bail!("delta byte budget exceeded");
            }
            let base = self.resolve_object_inner(&delta.base, seen, budget)?;
            let out = crate::util::apply_delta_limited(&base, &delta.ops, delta.size)?;
            if out.len() != delta.size || hex_sha256(&out) != hash {
                bail!("delta reconstructed hash/size mismatch");
            }
            return Ok(out);
        }
        Ok(raw)
    }

    pub fn read_commit(&self, id: &str) -> Result<Commit> {
        let raw = self.read_blob_resolved(id)?;
        if raw.len() > self.limits.metadata_bytes {
            bail!("commit metadata size limit exceeded");
        }
        let commit: Commit = serde_json::from_slice(&raw).context("invalid commit object")?;
        self.validate_tree(&commit.tree)?;
        for p in Self::commit_parents(&commit) {
            validate_oid(&p)?;
        }
        if let Some(p) = &commit.parent {
            validate_oid(p)?;
        }
        if !commit.parents.is_empty()
            && commit
                .parent
                .as_ref()
                .is_some_and(|p| Some(p) != commit.parents.first())
        {
            bail!("conflicting legacy and modern parent fields");
        }
        Ok(commit)
    }

    pub(super) fn commit_parents(commit: &Commit) -> Vec<String> {
        if !commit.parents.is_empty() {
            return commit.parents.clone();
        }
        match &commit.parent {
            Some(p) => vec![p.clone()],
            None => Vec::new(),
        }
    }

    pub(super) fn head_tree(&self) -> Result<BTreeMap<String, String>> {
        match self.head_commit_id()? {
            Some(id) => Ok(self.read_commit(&id)?.tree),
            None => Ok(BTreeMap::new()),
        }
    }

    pub(super) fn current_branch_name(&self) -> Option<String> {
        let raw = self.head_raw().ok()?;
        let r = raw.strip_prefix("ref:")?.trim();
        Some(r.strip_prefix("refs/heads/")?.to_string())
    }

    pub(super) fn atomic_write_json<T: Serialize>(&self, path: &Path, value: &T) -> Result<()> {
        let bytes = serde_json::to_vec_pretty(value)?;
        self.atomic_write(path, &bytes)
    }

    pub(super) fn atomic_write_object(&self, path: &Path, data: &[u8]) -> Result<()> {
        self.atomic_write(path, data)
    }

    pub(super) fn create_dirs_durable(&self, path: &Path) -> Result<()> {
        self.check_internal_path(path)?;
        if path.is_dir() {
            return Ok(());
        }
        let parent = path.parent().context("directory has no parent")?;
        self.create_dirs_durable(parent)?;
        match fs::create_dir(path) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists && path.is_dir() => {}
            Err(e) => return Err(e.into()),
        }
        sync_directory(parent)?;
        sync_directory(path)
    }

    // Durability order: write -> file fsync -> rename -> directory fsync.
    pub(super) fn atomic_write(&self, path: &Path, data: &[u8]) -> Result<()> {
        self.check_internal_path(path)?;
        let parent = path.parent().context("path has no parent")?;
        self.create_dirs_durable(parent)?;
        let tmp = parent.join(format!(".myvcs-tmp-{}", unique_suffix()));
        let result = (|| -> Result<()> {
            let mut f = OpenOptions::new().create_new(true).write(true).open(&tmp)?;
            f.write_all(data)?;
            f.sync_all()?;
            drop(f);
            #[cfg(test)]
            super::ref_state::test_crash_checkpoint(path, "before");
            fs::rename(&tmp, path)?;
            #[cfg(test)]
            super::ref_state::test_crash_checkpoint(path, "after");
            sync_directory(parent)
        })();
        if result.is_err() {
            let _ = fs::remove_file(tmp);
        }
        result
    }
}
