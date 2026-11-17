use super::*;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Upstream {
    pub remote: String,
    pub branch: String,
}

impl Repo {
    fn daily_config(&self) -> Result<BTreeMap<String, String>> {
        let path = self.meta.join("identity.json");
        self.check_internal_path(&path)?;
        let values: BTreeMap<String, String> = if path.exists() {
            serde_json::from_str(&self.read_metadata(&path)?)?
        } else {
            BTreeMap::new()
        };
        for (k, v) in &values {
            Self::identity_value(k, v)?;
        }
        Ok(values)
    }
    fn identity_value(key: &str, value: &str) -> Result<()> {
        if !matches!(key, "user.name" | "user.email")
            || value.trim().is_empty()
            || value.len() > 512
            || value.chars().any(char::is_control)
            || value.contains(['<', '>'])
        {
            bail!("invalid identity key/value (user.name or user.email, 1..512 safe characters)");
        }
        if key == "user.email"
            && (value.split('@').count() != 2
                || value.split('@').any(str::is_empty)
                || value.chars().any(char::is_whitespace))
        {
            bail!("invalid user.email");
        }
        Ok(())
    }
    pub fn config(&self, key: &str, value: Option<&str>) -> Result<Option<String>> {
        if !matches!(key, "user.name" | "user.email") {
            bail!("unsupported identity key");
        }
        if let Some(value) = value {
            Self::identity_value(key, value)?;
            self.with_repo_lock(|r| {
                let mut v = r.daily_config()?;
                v.insert(key.into(), value.into());
                r.atomic_write(&r.meta.join("identity.json"), &serde_json::to_vec(&v)?)
            })?;
        }
        Ok(self.daily_config()?.remove(key))
    }
    pub(super) fn author_identity(&self) -> Result<String> {
        let c = self.daily_config()?;
        Ok(match (c.get("user.name"), c.get("user.email")) {
            (Some(n), Some(e)) => format!("{n} <{e}>"),
            _ => std::env::var("USER").unwrap_or_else(|_| "unknown".into()),
        })
    }
    pub fn upstreams(&self) -> Result<BTreeMap<String, Upstream>> {
        let p = self.meta.join("upstreams.json");
        self.check_internal_path(&p)?;
        let values: BTreeMap<String, Upstream> = if p.exists() {
            serde_json::from_str(&self.read_metadata(&p)?)?
        } else {
            BTreeMap::new()
        };
        for (b, u) in &values {
            validate_ref(&format!("refs/heads/{b}"))?;
            validate_ref(&format!("refs/remotes/{}/{}", u.remote, u.branch))?;
            if u.remote.contains('/') {
                bail!("invalid upstream remote");
            }
        }
        Ok(values)
    }
    pub fn set_upstream(&self, local: Option<&str>, target: Option<&str>) -> Result<()> {
        let local = local
            .map(str::to_owned)
            .or_else(|| self.current_branch_name())
            .context("upstream requires attached branch")?;
        validate_ref(&format!("refs/heads/{local}"))?;
        let upstream = target
            .map(|s| -> Result<Upstream> {
                let (remote, branch) = s
                    .split_once('/')
                    .context("upstream must be remote/branch")?;
                validate_ref(&format!("refs/heads/{branch}"))?;
                if !self.remote_list()?.contains_key(remote) {
                    bail!("upstream remote is not configured");
                }
                Ok(Upstream {
                    remote: remote.into(),
                    branch: branch.into(),
                })
            })
            .transpose()?;
        self.with_repo_lock(|r| {
            if r.read_ref(&format!("refs/heads/{local}"))?.is_none()
                && r.current_branch_name().as_deref() != Some(&local)
            {
                bail!("local branch does not exist");
            }
            let mut v = r.upstreams()?;
            if let Some(u) = upstream {
                v.insert(local, u);
            } else {
                v.remove(&local);
            }
            let raw = serde_json::to_vec(&v)?;
            if raw.len() > r.limits.metadata_bytes {
                bail!("upstream configuration too large");
            }
            r.atomic_write(&r.meta.join("upstreams.json"), &raw)
        })
    }
    pub(super) fn remote_selection(
        &self,
        remote: Option<&str>,
        branch: Option<&str>,
    ) -> Result<(String, Option<String>)> {
        let current = self.current_branch_name();
        let mut upstreams = self.upstreams()?;
        let upstream = current.as_ref().and_then(|b| upstreams.remove(b));
        let selected_remote = remote
            .map(str::to_owned)
            .or_else(|| upstream.as_ref().map(|u| u.remote.clone()))
            .unwrap_or_else(|| "origin".into());
        let selected_branch = branch
            .map(str::to_owned)
            .or_else(|| {
                upstream
                    .filter(|u| u.remote == selected_remote)
                    .map(|u| u.branch)
            })
            .or(current);
        Ok((selected_remote, selected_branch))
    }
    pub fn tag(&self, name: Option<&str>, target: Option<&str>, delete: bool) -> Result<()> {
        if delete && (name.is_none() || target.is_some()) {
            bail!("tag -d requires exactly one tag name");
        }
        if name.is_none() && target.is_some() {
            bail!("tag target requires a name");
        }
        let Some(name) = name else {
            for (n, id) in self.list_refs()? {
                if let Some(n) = n.strip_prefix("refs/tags/") {
                    println!("{n}\t{id}");
                }
            }
            return Ok(());
        };
        let full = format!("refs/tags/{name}");
        validate_ref(&full)?;
        self.with_repo_lock(|r| {
            let old = r.read_ref(&full)?;
            let new = if delete {
                if old.is_none() {
                    bail!("tag does not exist");
                }
                None
            } else {
                let id = r.resolve_commit(target.unwrap_or("HEAD"))?;
                if let Some(old) = &old {
                    if old != &id {
                        bail!("tag already exists and is immutable");
                    }
                    return Ok(());
                }
                Some(id)
            };
            r.apply_local_changes_locked(
                &[RefChange {
                    name: full,
                    expected: old,
                    new,
                }],
                "tag",
            )
        })
    }
    pub(super) fn apply_local_changes_locked(
        &self,
        changes: &[RefChange],
        reason: &str,
    ) -> Result<()> {
        if changes.is_empty() {
            return Ok(());
        }
        if self.format_version()? == 2 {
            return self.transact_refs_locked(changes, reason);
        }
        // Format 1 keeps its historical single-ref durability contract.
        for c in changes {
            if self.read_ref(&c.name)? != c.expected {
                bail!("reference changed concurrently");
            }
            if let Some(id) = &c.new {
                self.update_ref_locked(&c.name, c.expected.as_deref(), id)?;
            } else {
                let p = self.ref_path(&c.name)?;
                fs::remove_file(&p)?;
                sync_directory(p.parent().unwrap())?;
            }
        }
        Ok(())
    }
    pub fn log_with_options(&self, graph: bool, all: bool) -> Result<()> {
        let state = self.refs_snapshot()?;
        let mut pending: Vec<String> = state.head_commit_id().into_iter().collect();
        let mut decorations: BTreeMap<String, Vec<String>> = BTreeMap::new();
        if let Some(id) = state.head_commit_id() {
            decorations.entry(id).or_default().push("HEAD".into());
        }
        for (n, id) in &state.refs {
            if ["refs/heads/", "refs/remotes/", "refs/tags/"]
                .iter()
                .any(|p| n.starts_with(p))
            {
                decorations.entry(id.clone()).or_default().push(n.clone());
                if all {
                    pending.push(id.clone());
                }
            }
        }
        let mut commits = BTreeMap::new();
        while let Some(id) = pending.pop() {
            if commits.contains_key(&id) {
                continue;
            }
            let c = self.read_commit(&id)?;
            pending.extend(Self::commit_parents(&c));
            commits.insert(id, c);
            if commits.len() > self.limits.graph_objects {
                bail!("log graph limit exceeded");
            }
        }
        if commits.is_empty() {
            println!("no commits yet");
            return Ok(());
        }
        let mut children: BTreeMap<String, usize> =
            commits.keys().map(|id| (id.clone(), 0)).collect();
        for c in commits.values() {
            for p in Self::commit_parents(c) {
                *children.get_mut(&p).context("missing graph parent")? += 1;
            }
        }
        let mut ready: BTreeSet<String> = children
            .iter()
            .filter(|(_, n)| **n == 0)
            .map(|(id, _)| id.clone())
            .collect();
        let mut count = 0;
        while let Some(id) = ready.pop_first() {
            let c = &commits[&id];
            let parents = Self::commit_parents(c);
            println!(
                "{}commit {}{}",
                if graph { "* " } else { "" },
                id,
                decorations
                    .get(&id)
                    .map(|d| format!(" ({})", d.join(", ")))
                    .unwrap_or_default()
            );
            println!("Parents: {}", parents.join(" "));
            if graph {
                for parent in &parents {
                    println!("| -> {parent}");
                }
            }
            println!(
                "Author: {}\nDate:   {}\n\n    {}\n",
                c.author, c.timestamp, c.message
            );
            count += 1;
            for p in parents {
                let n = children.get_mut(&p).unwrap();
                *n -= 1;
                if *n == 0 {
                    ready.insert(p);
                }
            }
        }
        if count != commits.len() {
            bail!("cycle in commit graph");
        }
        Ok(())
    }
}
