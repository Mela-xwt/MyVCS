use std::collections::{BTreeMap, BTreeSet};

use anyhow::Result;

use crate::repo::Repo;

pub struct CommitGraph {
    generations: BTreeMap<String, u64>,
}

impl CommitGraph {
    pub fn build(repo: &Repo) -> Result<Self> {
        let mut generations = BTreeMap::new();
        let mut visiting = BTreeSet::new();
        for root in repo.commit_roots()? {
            let mut stack = vec![(root, false)];
            while let Some((id, finish)) = stack.pop() {
                if generations.contains_key(&id) {
                    continue;
                }
                let c = repo.read_commit(&id)?;
                let parents = Repo::commit_parents(&c);
                if finish {
                    let generation = parents
                        .iter()
                        .filter_map(|p| generations.get(p))
                        .copied()
                        .max()
                        .unwrap_or(0u64)
                        + 1;
                    generations.insert(id.clone(), generation);
                    visiting.remove(&id);
                } else {
                    if !visiting.insert(id.clone()) {
                        anyhow::bail!("commit graph cycle");
                    }
                    repo.check_graph_size(visiting.len() + generations.len())?;
                    stack.push((id, true));
                    for parent in parents {
                        if !generations.contains_key(&parent) {
                            stack.push((parent, false));
                        }
                    }
                }
            }
        }
        Ok(Self { generations })
    }

    pub fn generation(&self, commit_id: &str) -> Option<u64> {
        self.generations.get(commit_id).copied()
    }

    pub fn fast_ancestor_check(&self, ancestor: &str, descendant: &str) -> Option<bool> {
        let gen_a = self.generation(ancestor)?;
        let gen_d = self.generation(descendant)?;

        if gen_a > gen_d {
            return Some(false);
        }

        if gen_a == gen_d && ancestor == descendant {
            return Some(true);
        }

        None
    }

    /// 已索引的提交数。
    pub fn len(&self) -> usize {
        self.generations.len()
    }
}
