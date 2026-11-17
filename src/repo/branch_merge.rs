use super::commit_graph::CommitGraph;
use super::*;

#[derive(serde::Serialize, serde::Deserialize)]
pub(super) struct MergeConflict {
    base: Option<String>,
    ours: Option<String>,
    theirs: Option<String>,
    kind: String,
}

#[derive(serde::Serialize, serde::Deserialize)]
pub(super) struct MergeState {
    #[serde(default)]
    phase: MergePhase,
    pub(super) original_head: String,
    head_raw: String,
    pub(super) target: String,
    index: Option<Vec<u8>>,
    touched: BTreeMap<String, Option<String>>,
    conflicts: BTreeMap<String, MergeConflict>,
}

#[derive(Default, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
enum MergePhase {
    #[default]
    Prepared,
    Materialized,
}

impl Repo {
    // 分支创建、删除与列表。
    pub fn branch(&self, name: Option<&str>, delete: Option<&str>) -> Result<()> {
        if let Some(name) = delete {
            let full = format!("refs/heads/{name}");
            validate_ref(&full)?;
            let p = if self.format_version()? == 1 {
                self.ref_path(&full)?
            } else {
                self.meta.join(&full)
            };
            if self.current_ref()?.as_deref() == Some(&full) {
                bail!("cannot delete current branch");
            }
            let old = self.read_ref(&full)?;
            if old.is_none() {
                bail!("branch does not exist: {name}");
            }
            if self.format_version()? == 2 {
                self.transact_refs_locked(
                    &[RefChange {
                        name: full.clone(),
                        expected: old,
                        new: None,
                    }],
                    "delete branch",
                )?;
            } else {
                fs::remove_file(&p)?;
                sync_directory(p.parent().unwrap())?;
            }
            *self.commit_graph.lock().unwrap() = None;
            println!("deleted branch {name}");
            return Ok(());
        }
        if let Some(name) = name {
            let full = format!("refs/heads/{name}");
            validate_ref(&full)?;
            if self.format_version()? == 1 {
                self.ref_path(&full)?;
            }
            let head = self
                .head_commit_id()?
                .context("cannot create branch without commit")?;
            self.update_ref_locked(&full, None, &head)?;
            println!("created branch {name}");
            return Ok(());
        }
        let snapshot = self.refs_snapshot()?;
        let cur = snapshot
            .current_ref()
            .and_then(|n| n.strip_prefix("refs/heads/"));
        let mut names: Vec<_> = snapshot
            .refs
            .keys()
            .filter_map(|r| r.strip_prefix("refs/heads/").map(str::to_owned))
            .collect();
        names.sort();
        let upstreams = self.upstreams()?;
        for n in names {
            let suffix = upstreams
                .get(&n)
                .map(|u| format!(" [upstream: {}/{}]", u.remote, u.branch))
                .unwrap_or_default();
            if cur.as_deref() == Some(n.as_str()) {
                println!("* {}{}", n, suffix);
            } else {
                println!("  {}{}", n, suffix);
            }
        }
        Ok(())
    }

    // 切换到分支或分离头提交。
    pub fn checkout(&self, target: &str) -> Result<()> {
        self.require_worktree()?;
        validate_ref(&format!("refs/heads/{target}"))?;

        let current = self.head_tree()?;

        if self.read_ref(&format!("refs/heads/{target}"))?.is_some() {
            let commit_id = self
                .read_ref(&format!("refs/heads/{target}"))?
                .context("branch has no commits")?;
            self.validate_closure(&commit_id)?;
            let commit = self.read_commit(&commit_id)?;
            let raw = format!("ref: refs/heads/{target}");
            let prepared = self.prepare_head_locked(&raw, "checkout branch")?;
            self.restore_tree(&current, &commit.tree)?;
            self.publish_head_locked(&raw, prepared)?;
            self.write_index(&Index::default())?;
            self.clear_merge_state()?;
            println!("switched to branch {}", target);
            return Ok(());
        }

        let commit_id = self.resolve_commit(target)?;
        self.validate_closure(&commit_id)?;
        let commit = self.read_commit(&commit_id)?;
        let prepared = self.prepare_head_locked(&commit_id, "checkout detached")?;
        self.restore_tree(&current, &commit.tree)?;
        self.publish_head_locked(&commit_id, prepared)?;
        self.write_index(&Index::default())?;
        self.clear_merge_state()?;
        println!("HEAD detached at {}", short_id(&commit_id));
        Ok(())
    }

    // 合并策略：能快进则快进，否则执行三方合并。
    pub fn merge_fast_forward_only(&self, branch: &str) -> Result<()> {
        self.require_worktree()?;
        crate::debug_log!("merge start branch={}", branch);
        if self.read_merge_state()?.is_some()
            || self.read_merge_head()?.is_some()
            || !self.read_merge_conflicts()?.is_empty()
        {
            bail!("merge already in progress; resolve and commit it, or use merge --abort");
        }
        let current = self
            .head_commit_id()?
            .ok_or_else(|| anyhow!("no current commit to merge into"))?;
        let target = self.resolve_commit(branch)?;
        self.validate_closure(&current)?;
        self.validate_closure(&target)?;
        let current_tree = self.head_tree()?;
        self.merge_preflight(&current_tree, &current_tree)?;

        if current == target {
            self.clear_merge_state()?;
            println!("already up to date");
            return Ok(());
        }

        if self.is_ancestor(&current, &target)? {
            let target_commit = self.read_commit(&target)?;
            self.merge_preflight(&current_tree, &target_commit.tree)?;
            let prepared = self.prepare_head_commit_locked(&target, "merge fast-forward")?;
            self.save_merge_state(
                &current,
                &target,
                &current_tree,
                &target_commit.tree,
                BTreeMap::new(),
            )?;
            self.restore_tree(&current_tree, &target_commit.tree)?;
            if let Some(p) = prepared {
                self.publish_refs_locked(p)?;
            } else {
                self.update_head_to_commit(&target)?;
            }
            self.clear_merge_state()?;
            crate::debug_log!("merge fast-forward target={}", target);
            println!("fast-forward merge to {}", short_id(&target));
            return Ok(());
        }

        if self.is_ancestor(&target, &current)? {
            self.clear_merge_state()?;
            println!("already up to date");
            return Ok(());
        }

        let base = self
            .find_lca(&current, &target)?
            .ok_or_else(|| anyhow!("failed to find common ancestor"))?;
        let base_commit = self.read_commit(&base)?;
        let ours_commit = self.read_commit(&current)?;
        let theirs_commit = self.read_commit(&target)?;

        // Check candidate paths before creating merged blob objects too. Rejecting
        // unsupported structural transitions here is deliberately conservative.
        self.merge_preflight(&ours_commit.tree, &theirs_commit.tree)?;

        let (merged_tree, details) =
            self.merge_trees_3way(&base_commit.tree, &ours_commit.tree, &theirs_commit.tree)?;
        let conflicts: Vec<_> = details.keys().cloned().collect();
        crate::debug_log!(
            "merge 3-way base={} ours={} theirs={} conflicts_precheck={}",
            base,
            current,
            target,
            conflicts.len()
        );

        self.merge_preflight(&ours_commit.tree, &merged_tree)?;
        // Reserve bounded reference metadata before touching the worktree for a
        // clean merge. Conflicted merges publish no reference until user commit.
        let publication = if conflicts.is_empty() {
            let merge_commit = Commit {
                tree: merged_tree.clone(),
                parents: vec![current.clone(), target.clone()],
                parent: None,
                message: format!("Merge branch {}", branch),
                author: self.author_identity()?,
                timestamp: now_unix() as i64,
            };
            let id = self.write_object_with_temp(
                &serde_json::to_vec(&merge_commit)?,
                crate::util::BlobTemperature::Hot,
            )?;
            let prepared = self.prepare_head_commit_locked(&id, "merge commit")?;
            Some((id, prepared))
        } else {
            None
        };
        self.save_merge_state(&current, &target, &ours_commit.tree, &merged_tree, details)?;
        // Persist the parent and conflict list before materialization so interruptions
        // can always be recovered by aborting. The journal is removed last.
        self.atomic_write(
            &self.meta.join("MERGE_HEAD"),
            format!("{target}\n").as_bytes(),
        )?;
        self.write_merge_conflicts(&conflicts)?;
        self.restore_tree(&ours_commit.tree, &merged_tree)?;

        if !conflicts.is_empty() {
            // Preserve clean parts of a conflicted merge for the eventual manual
            // commit. Conflict paths must still be explicitly resolved/staged.
            let index = Index {
                entries: merged_tree
                    .iter()
                    .filter(|(p, _)| !conflicts.contains(p))
                    .map(|(p, id)| (p.clone(), id.clone()))
                    .collect(),
                removed: ours_commit
                    .tree
                    .keys()
                    .filter(|p| !merged_tree.contains_key(*p))
                    .cloned()
                    .collect(),
            };
            self.write_index(&index)?;
            self.atomic_write(
                &self.meta.join("MERGE_HEAD"),
                format!("{}\n", target).as_bytes(),
            )?;
            self.write_merge_conflicts(&conflicts)?;
            let mut state = self
                .read_merge_state()?
                .context("missing merge recovery state")?;
            // Readiness is durable only after every file, merge conflict path,
            // and clean merged index entry has been materialized successfully.
            state.phase = MergePhase::Materialized;
            self.atomic_write_json(&self.meta.join("merge-state.json"), &state)?;
            println!("merge has conflicts; resolve then commit manually:");
            for p in conflicts {
                println!("  {}", p);
            }
            return Ok(());
        }

        let (new_id, prepared) = publication.context("missing clean merge publication")?;
        if let Some(p) = prepared {
            self.publish_refs_locked(p)?;
        } else {
            self.update_head_to_commit(&new_id)?;
        }
        self.write_index(&Index::default())?;
        self.clear_merge_state()?;
        crate::debug_log!("merge commit id={}", new_id);
        println!(
            "merged {} into current branch as {}",
            branch,
            short_id(&new_id)
        );
        Ok(())
    }

    fn merge_trees_3way(
        &self,
        base: &BTreeMap<String, String>,
        ours: &BTreeMap<String, String>,
        theirs: &BTreeMap<String, String>,
    ) -> Result<(BTreeMap<String, String>, BTreeMap<String, MergeConflict>)> {
        let mut all_paths = BTreeSet::new();
        all_paths.extend(base.keys().cloned());
        all_paths.extend(ours.keys().cloned());
        all_paths.extend(theirs.keys().cloned());

        let mut out = BTreeMap::new();
        let mut conflicts = BTreeMap::new();

        for p in all_paths {
            let b = base.get(&p);
            let o = ours.get(&p);
            let t = theirs.get(&p);

            // 经典三方合并规则；无法自动归并时写入冲突内容。
            if o == t {
                if let Some(oid) = o {
                    out.insert(p, oid.clone());
                }
                continue;
            }

            if b == o {
                if let Some(oid) = t {
                    out.insert(p, oid.clone());
                }
                continue;
            }

            if b == t {
                if let Some(oid) = o {
                    out.insert(p, oid.clone());
                }
                continue;
            }

            let bb = b
                .map(|id| self.read_blob_resolved(id))
                .transpose()?
                .unwrap_or_default();
            let ob = o
                .map(|id| self.read_blob_resolved(id))
                .transpose()?
                .unwrap_or_default();
            let tb = t
                .map(|id| self.read_blob_resolved(id))
                .transpose()?
                .unwrap_or_default();
            let text = [&bb, &ob, &tb]
                .iter()
                .all(|bytes| line_merge::is_text(bytes));
            let mut kind = if text { "text" } else { "binary" };
            if text {
                if let Some((mut merged, mut conflict)) = line_merge::merge(&bb, &ob, &tb) {
                    // Deletion and a surviving empty file are different tree
                    // states even when their line edits happen to be identical.
                    if !conflict && (o.is_none() || t.is_none()) {
                        conflict = true;
                        merged = b"<<<<<<< OURS\n".to_vec();
                        merged.extend_from_slice(&ob);
                        if !ob.is_empty() && !ob.ends_with(b"\n") {
                            merged.push(b'\n');
                        }
                        merged.extend_from_slice(b"=======\n");
                        merged.extend_from_slice(&tb);
                        if !tb.is_empty() && !tb.ends_with(b"\n") {
                            merged.push(b'\n');
                        }
                        merged.extend_from_slice(b">>>>>>> THEIRS\n");
                    }
                    let oid =
                        self.write_object_with_temp(&merged, crate::util::BlobTemperature::Warm)?;
                    out.insert(p.clone(), oid);
                    if !conflict {
                        continue;
                    }
                } else {
                    kind = "text-limit";
                    if let Some(oid) = o {
                        out.insert(p.clone(), oid.clone());
                    }
                }
            } else if let Some(oid) = o {
                out.insert(p.clone(), oid.clone());
            }
            conflicts.insert(
                p,
                MergeConflict {
                    base: b.cloned(),
                    ours: o.cloned(),
                    theirs: t.cloned(),
                    kind: kind.into(),
                },
            );
        }

        Ok((out, conflicts))
    }

    pub(super) fn read_merge_state(&self) -> Result<Option<MergeState>> {
        let path = self.meta.join("merge-state.json");
        self.check_internal_path(&path)?;
        if !path.exists() {
            return Ok(None);
        }
        let state: MergeState =
            serde_json::from_str(&self.read_metadata(&path)?).context("invalid merge state")?;
        validate_oid(&state.original_head)?;
        validate_oid(&state.target)?;
        for (p, id) in &state.touched {
            validate_tree_path(p)?;
            if let Some(id) = id {
                validate_oid(id)?;
            }
        }
        for (p, c) in &state.conflicts {
            validate_tree_path(p)?;
            for id in [&c.base, &c.ours, &c.theirs].into_iter().flatten() {
                validate_oid(id)?;
            }
        }
        Ok(Some(state))
    }

    pub(super) fn check_merge_commit_state(
        &self,
        index: &Index,
        merge_head: Option<&str>,
    ) -> Result<()> {
        let Some(state) = self.read_merge_state()? else {
            return Ok(());
        };
        if state.phase != MergePhase::Materialized {
            bail!("incomplete merge materialization; run merge --abort before committing");
        }
        if self.head_commit_id()?.as_deref() != Some(&state.original_head)
            || self.head_raw()? != state.head_raw
        {
            bail!(
                "HEAD changed since merge began; inspect and clear stale merge state before committing"
            );
        }
        if merge_head != Some(&state.target) {
            bail!("incomplete merge parent metadata; run merge --abort before committing");
        }
        // The durable journal is authoritative even if the compatibility path
        // list was removed by an interrupted cleanup or edited independently.
        for path in state.conflicts.keys() {
            if !index.entries.contains_key(path) && !index.removed.contains(path) {
                bail!(
                    "unresolved merge conflict: {}; resolve and stage it before committing",
                    path
                );
            }
        }
        Ok(())
    }

    fn save_merge_state(
        &self,
        original_head: &str,
        target: &str,
        ours: &BTreeMap<String, String>,
        merged: &BTreeMap<String, String>,
        conflicts: BTreeMap<String, MergeConflict>,
    ) -> Result<()> {
        let touched = ours
            .keys()
            .chain(merged.keys())
            .chain(conflicts.keys())
            .filter(|p| ours.get(*p) != merged.get(*p) || conflicts.contains_key(*p))
            .map(|p| (p.clone(), ours.get(p).cloned()))
            .collect();
        let index_path = self.index_path();
        self.check_internal_path(&index_path)?;
        let index = if index_path.exists() {
            Some(fs::read(index_path)?)
        } else {
            None
        };
        let state = MergeState {
            phase: MergePhase::Prepared,
            original_head: original_head.into(),
            head_raw: self.head_raw()?,
            target: target.into(),
            index,
            touched,
            conflicts,
        };
        // Keep the snapshot readable under the same metadata bound after a crash.
        // Reserve the extra bytes required when "prepared" becomes
        // "materialized", so publishing readiness cannot exceed this bound.
        if serde_json::to_vec_pretty(&state)?.len().saturating_add(4) > self.limits.metadata_bytes {
            bail!("merge recovery state size limit exceeded");
        }
        self.atomic_write_json(&self.meta.join("merge-state.json"), &state)
    }

    fn merge_preflight(
        &self,
        current: &BTreeMap<String, String>,
        target: &BTreeMap<String, String>,
    ) -> Result<()> {
        self.validate_tree(target)?;
        let index = self.read_index()?;
        if !index.entries.is_empty() || !index.removed.is_empty() {
            bail!("staged changes; commit or unstage before merge");
        }
        for (name, oid) in current {
            let path = self.work_path(name)?;
            if !path.is_file() || hex_sha256_file(&path)? != *oid {
                bail!("local changes would be overwritten: {name}");
            }
        }
        for name in target.keys() {
            self.read_blob_resolved(&target[name])?;
            let path = self
                .work_path(name)
                .with_context(|| format!("unsafe path or file/directory conflict: {name}"))?;
            if !current.contains_key(name) && path.exists() {
                bail!("untracked path would be overwritten: {name}");
            }
            if path.is_dir() {
                bail!("file/directory conflict: {name}");
            }
            let mut parent = path.parent();
            while let Some(p) = parent {
                if p == self.root {
                    break;
                }
                if p.exists() && !p.is_dir() {
                    bail!("file/directory conflict: {name}");
                }
                parent = p.parent();
            }
            for old in current.keys() {
                if old != name && old.eq_ignore_ascii_case(name) {
                    bail!("case-only path transition requires manual checkout");
                }
            }
        }
        Ok(())
    }

    pub fn merge_abort(&self) -> Result<()> {
        self.require_worktree()?;
        let mut state = self
            .read_merge_state()?
            .context("no merge recovery state; no merge to abort")?;
        if self.head_commit_id()?.as_deref() != Some(&state.original_head)
            || self.head_raw()? != state.head_raw
        {
            bail!("HEAD changed since merge began; refusing abort");
        }
        // Preflight the complete snapshot before touching anything; never recursively
        // remove directories because they may contain unrelated untracked files.
        for (name, id) in &state.touched {
            let path = self.work_path(name)?;
            if path.exists() && !path.is_file() {
                bail!("file/directory conflict during abort: {name}");
            }
            if let Some(id) = id {
                self.read_blob_resolved(id)?;
            }
            let mut parent = path.parent();
            while let Some(p) = parent {
                if p == self.root {
                    break;
                }
                if p.exists() && !p.is_dir() {
                    bail!("file/directory conflict during abort: {name}");
                }
                parent = p.parent();
            }
        }
        // If restoration/cleanup is interrupted, an otherwise complete merge
        // must no longer be committable using a partially restored worktree/index.
        state.phase = MergePhase::Prepared;
        self.atomic_write_json(&self.meta.join("merge-state.json"), &state)?;
        for (name, id) in &state.touched {
            let path = self.work_path(name)?;
            if let Some(id) = id {
                self.atomic_write(&path, &self.read_blob_resolved(id)?)?;
            } else {
                match fs::remove_file(&path) {
                    Ok(()) => sync_directory(path.parent().unwrap())?,
                    Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                    Err(e) => return Err(e.into()),
                }
            }
        }
        let index_path = self.index_path();
        self.check_internal_path(&index_path)?;
        if let Some(bytes) = &state.index {
            self.atomic_write(&index_path, bytes)?;
        } else {
            match fs::remove_file(&index_path) {
                Ok(()) => sync_directory(&self.meta)?,
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => return Err(e.into()),
            }
        }
        self.clear_merge_state()?;
        println!("merge aborted");
        Ok(())
    }

    // 将目标 tree 物化到工作区。
    pub(super) fn restore_tree(
        &self,
        current_tree: &BTreeMap<String, String>,
        target_tree: &BTreeMap<String, String>,
    ) -> Result<()> {
        self.require_worktree()?;
        self.validate_tree(current_tree)?;
        self.validate_tree(target_tree)?;
        // Reject every unsafe path and unreadable object before touching the worktree.
        for p in current_tree.keys().chain(target_tree.keys()) {
            self.work_path(p)?;
        }
        for oid in target_tree.values() {
            self.read_blob_resolved(oid)?;
        }
        for p in current_tree.keys() {
            if !target_tree.contains_key(p) {
                let fp = self.work_path(p)?;
                if fp.exists() {
                    fs::remove_file(&fp)?;
                    sync_directory(fp.parent().unwrap())?;
                }
            }
        }

        for (p, oid) in target_tree {
            let data = self
                .read_blob_resolved(oid)
                .with_context(|| format!("failed to read blob {} for path {}", oid, p))?;
            let fp = self.work_path(p)?;
            if let Some(parent) = fp.parent() {
                fs::create_dir_all(parent)?;
            }
            self.atomic_write(&fp, &data)
                .with_context(|| format!("failed to restore file {}", fp.display()))?;
        }

        Ok(())
    }

    // 遍历提交图判断祖先关系（Commit-Graph 加速）。
    pub fn is_ancestor(&self, ancestor: &str, descendant: &str) -> Result<bool> {
        self.read_commit(ancestor)?;
        self.read_commit(descendant)?;
        if ancestor == descendant {
            return Ok(true);
        }
        // 尝试构建/复用 Commit-Graph 做 O(1) 剪枝
        self.get_or_build_commit_graph()?;
        let cg_guard = self.commit_graph();
        if let Some(cg) = cg_guard.as_ref() {
            if let Some(result) = cg.fast_ancestor_check(ancestor, descendant) {
                if !result {
                    return Ok(false);
                }
            }
        }
        drop(cg_guard); // 释放锁，避免递归死锁

        let mut stack = vec![descendant.to_string()];
        let mut seen = BTreeSet::new();
        while let Some(id) = stack.pop() {
            if !seen.insert(id.clone()) {
                continue;
            }
            self.check_graph_size(seen.len())?;
            if id == ancestor {
                return Ok(true);
            }
            let c = self.read_commit(&id)?;
            for p in Self::commit_parents(&c) {
                stack.push(p);
            }
        }
        Ok(false)
    }

    // 通过 BFS 计算祖先深度表，用于 LCA 评分。
    fn ancestor_depths(&self, start: &str) -> Result<BTreeMap<String, usize>> {
        let mut out = BTreeMap::new();
        let mut queue = std::collections::VecDeque::new();
        queue.push_back((start.to_string(), 0usize));
        while let Some((id, d)) = queue.pop_front() {
            if out.contains_key(&id) {
                continue;
            }
            out.insert(id.clone(), d);
            self.check_graph_size(out.len())?;
            let c = self.read_commit(&id)?;
            for p in Self::commit_parents(&c) {
                queue.push_back((p, d + 1));
            }
        }
        Ok(out)
    }

    // A merge base is a common ancestor with no common descendant. Distance
    // scoring chooses an arbitrary answer for criss-cross histories and is unsafe.
    fn find_lca(&self, a: &str, b: &str) -> Result<Option<String>> {
        let da = self.ancestor_depths(a)?;
        let db = self.ancestor_depths(b)?;

        let common: BTreeSet<_> = da
            .keys()
            .filter(|id| db.contains_key(*id))
            .cloned()
            .collect();
        let mut older = BTreeSet::new();
        for id in &common {
            // Every parent of a common ancestor is itself common. Mark direct
            // parents; transitivity is covered when that parent is visited.
            for parent in Self::commit_parents(&self.read_commit(id)?) {
                older.insert(parent);
            }
        }
        let best: Vec<_> = common.difference(&older).cloned().collect();
        if best.len() > 1 {
            bail!("multiple merge bases are unsupported; recursive virtual-base merge required");
        }
        Ok(best.into_iter().next())
    }

    /// 懒加载构建/复用 Commit-Graph（关联到当前 Repo 实例）。
    fn get_or_build_commit_graph(&self) -> Result<()> {
        let mut cg_opt = self.commit_graph.lock().unwrap();
        if cg_opt.is_some() {
            return Ok(());
        }
        crate::debug_log!("commit-graph: building...");
        let cg = CommitGraph::build(self)?;
        crate::debug_log!("commit-graph: ready, {} commits indexed", cg.len());
        *cg_opt = Some(cg);
        Ok(())
    }

    fn commit_graph(&self) -> std::sync::MutexGuard<'_, Option<CommitGraph>> {
        self.commit_graph.lock().unwrap()
    }
}
