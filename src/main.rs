mod cli;

use anyhow::{Result, bail};
use clap::Parser;

use crate::cli::{Cli, Commands, ReflogAction, RemoteAction, StashAction};
use myvcs::repo::Repo;

// 供调试日志使用的人类可读命令标签。
fn command_label(cmd: &Commands) -> &'static str {
    match cmd {
        Commands::CheckIgnore { .. } => "check-ignore",
        Commands::Restore { .. } => "restore",
        Commands::RecoverWorktree { .. } => "recover-worktree",
        Commands::Stash { .. } => "stash",
        Commands::Doctor { .. } => "doctor",
        Commands::Stats { .. } => "stats",
        Commands::Config { .. } => "config",
        Commands::Tag { .. } => "tag",
        Commands::Audit { .. } => "audit",
        Commands::Remote { .. } => "remote",
        Commands::Push { .. } => "push",
        Commands::Fetch { .. } => "fetch",
        Commands::Pull { .. } => "pull",
        Commands::Clone { .. } => "clone",
        Commands::Serve { .. } => "serve",
        Commands::Init { .. } => "init",
        Commands::Upgrade { .. } => "upgrade",
        Commands::Reflog { .. } => "reflog",
        Commands::Recover { .. } => "recover",
        Commands::Add { .. } => "add",
        Commands::Rm { .. } => "rm",
        Commands::Status => "status",
        Commands::Diff { .. } => "diff",
        Commands::Commit { .. } => "commit",
        Commands::Log { .. } => "log",
        Commands::Reset { .. } => "reset",
        Commands::Branch { .. } => "branch",
        Commands::Checkout { .. } => "checkout",
        Commands::Merge { .. } => "merge",
        Commands::Gc => "gc",
        Commands::Verify => "verify",
    }
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    myvcs::debug_log!("command={} args={:?}", command_label(&cli.cmd), &cli.cmd);

    let open = || match &cli.repo {
        Some(path) => Repo::open(path),
        None => Repo::discover(),
    };
    match cli.cmd {
        Commands::CheckIgnore { paths } => open()?.with_repo_lock(|r| r.check_ignore(&paths)),
        Commands::Restore {
            path,
            source,
            staged,
        } => open()?.with_repo_lock(|r| r.restore_file(&path, source.as_deref(), staged)),
        Commands::RecoverWorktree { abort } => open()?.recover_worktree(abort),
        Commands::Stash { action } => open()?.with_repo_lock(|r| match action {
            StashAction::Push {
                message,
                include_untracked,
            } => r.stash_push(&message, include_untracked),
            StashAction::List => r.stash_list(),
            StashAction::Show { entry } => r.stash_show(&entry),
            StashAction::Apply { entry } => r.stash_apply(&entry, false),
            StashAction::Pop { entry } => r.stash_apply(&entry, true),
            StashAction::Drop { entry } => r.stash_drop(&entry),
        }),
        Commands::Doctor { json } => {
            let repo = if cli.repo.is_some() {
                Some(open()?)
            } else {
                open().ok()
            };
            let value = Repo::doctor(repo.as_ref())?;
            if json {
                println!("{}", serde_json::to_string_pretty(&value)?);
            } else {
                for (key, v) in value.as_object().unwrap() {
                    println!("{key}: {v}");
                }
            }
            Ok(())
        }
        Commands::Stats { json } => open()?.with_repo_lock(|r| {
            let value = r.stats()?;
            if json {
                println!("{}", serde_json::to_string_pretty(&value)?);
            } else {
                for (key, v) in value.as_object().unwrap() {
                    println!("{key}: {v}");
                }
            }
            Ok(())
        }),
        Commands::Remote { action } => {
            let repo = open()?;
            match action {
                None => {
                    for (name, url) in repo.remote_list()? {
                        println!("{name}\t{url}");
                    }
                    Ok(())
                }
                Some(RemoteAction::Add { name, url }) => {
                    repo.remote_configure("add", &name, Some(&url))
                }
                Some(RemoteAction::SetUrl { name, url }) => {
                    repo.remote_configure("set-url", &name, Some(&url))
                }
                Some(RemoteAction::Remove { name }) => repo.remote_configure("remove", &name, None),
            }
        }
        Commands::Config { key, value } => {
            if let Some(v) = open()?.config(&key, value.as_deref())? {
                println!("{v}");
            }
            Ok(())
        }
        Commands::Tag {
            name,
            target,
            delete,
        } => open()?.tag(name.as_deref(), target.as_deref(), delete),
        Commands::Audit { limit, action } => {
            let repo = open()?;
            match action {
                Some(ReflogAction::Expire { keep }) => repo.audit_expire(keep),
                None => {
                    for e in repo.audit_list(limit)? {
                        println!("{}", serde_json::to_string(&e)?);
                    }
                    Ok(())
                }
            }
        }
        Commands::Push {
            remote,
            refspecs,
            set_upstream,
            atomic,
            tags,
            leases,
        } => open()?.push_with_options(
            remote.as_deref(),
            &refspecs,
            set_upstream,
            atomic,
            tags,
            &leases,
        ),
        Commands::Fetch {
            remote,
            branch,
            prune,
            tags,
        } => open()?
            .fetch_with_options(remote.as_deref(), branch.as_deref(), prune, tags)
            .map(|_| ()),
        Commands::Pull {
            remote,
            branch,
            ff_only,
        } => open()?.pull_with_options(remote.as_deref(), branch.as_deref(), ff_only),
        Commands::Clone {
            url,
            destination,
            branch,
        } => Repo::clone_remote(&url, &destination, branch.as_deref()),
        Commands::Serve {
            root,
            repository,
            read_only,
        } => {
            let root = root.or_else(|| std::env::var_os("MYVCS_REPO_ROOT").map(Into::into));
            let Some(root) = root else {
                bail!("serve requires --root or operator-configured MYVCS_REPO_ROOT");
            };
            Repo::serve(&root, &repository, read_only)
        }
        Commands::Upgrade { quiescent } => {
            if !quiescent {
                bail!(
                    "upgrade requires stopping ALL old-version myvcs processes; acknowledge with --quiescent"
                );
            }
            open()?.upgrade()
        }
        Commands::Reflog { reference, action } => {
            let repo = open()?;
            match action {
                Some(ReflogAction::Expire { keep }) => repo.reflog_expire(keep),
                None => {
                    for e in repo.reflog(reference.as_deref())? {
                        println!(
                            "{} tx={} {} {} -> {} {} {} {}",
                            e.seq,
                            e.transaction,
                            e.name,
                            e.old.as_deref().unwrap_or("-"),
                            e.new.as_deref().unwrap_or("-"),
                            e.timestamp,
                            e.actor,
                            e.reason
                        );
                    }
                    Ok(())
                }
            }
        }
        Commands::Recover { oid, branch } => open()?.recover(&oid, &branch),
        Commands::Init {
            path,
            bare,
            empty,
            team,
        } => {
            let path = path.or(cli.repo).unwrap_or_else(|| ".".into());
            if team
                && (path.join("HEAD").exists()
                    || path.join(".myvcs/HEAD").exists()
                    || path.join("format.json").exists()
                    || path.join(".myvcs/format.json").exists())
            {
                bail!(
                    "existing repository: use `upgrade --quiescent` after stopping all old processes"
                );
            }
            let repo = Repo::init_at(&path, bare, empty)?;
            if team {
                repo.upgrade()?;
            }
            Ok(())
        }
        Commands::Add { paths } => {
            let repo = open()?;
            // 修改类操作在仓库锁保护下执行。
            repo.with_repo_lock(|r| r.add_paths(&paths))
        }
        Commands::Rm { paths } => {
            let repo = open()?;
            repo.with_repo_lock(|r| r.rm_paths(&paths))
        }
        Commands::Status => {
            let repo = open()?;
            repo.with_repo_lock(|r| r.status())
        }
        Commands::Diff { path, staged } => open()?.with_repo_lock(|r| {
            if staged {
                r.diff_staged(path.as_deref())
            } else {
                r.diff(path.as_deref())
            }
        }),
        Commands::Commit { message } => {
            let repo = open()?;
            repo.with_repo_lock(|r| r.commit(&message))
        }
        Commands::Log { graph, all } => open()?.log_with_options(graph, all),
        Commands::Reset { commit, hard } => {
            if !hard {
                // 当前仅实现 --hard 语义，避免行为歧义。
                bail!("only --hard reset is currently supported")
            }
            let repo = open()?;
            let target = repo.resolve_commit(&commit)?;
            repo.with_repo_lock(|r| r.reset_hard(&target))
        }
        Commands::Branch {
            name,
            delete,
            set_upstream_to,
            unset_upstream,
        } => {
            let repo = open()?;
            if set_upstream_to.is_some() || unset_upstream {
                return repo.set_upstream(name.as_deref(), set_upstream_to.as_deref());
            }
            if name.is_some() || delete.is_some() {
                repo.with_repo_lock(|r| r.branch(name.as_deref(), delete.as_deref()))
            } else {
                repo.branch(name.as_deref(), delete.as_deref())
            }
        }
        Commands::Checkout { target } => {
            let repo = open()?;
            repo.with_repo_lock(|r| r.checkout(&target))
        }
        Commands::Merge { branch, abort } => {
            let repo = open()?;
            repo.with_repo_lock(|r| {
                if abort {
                    r.merge_abort()
                } else {
                    r.merge_fast_forward_only(branch.as_deref().expect("clap requires branch"))
                }
            })
        }
        Commands::Gc => {
            let repo = open()?;
            repo.with_repo_lock(|r| r.gc())
        }
        Commands::Verify => open()?.verify(),
    }
}
