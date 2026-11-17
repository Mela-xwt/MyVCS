use clap::{Parser, Subcommand};

// 顶层命令行解析器。
#[derive(Parser)]
#[command(name = "myvcs")]
#[command(about = "A lightweight VCS with SSH team collaboration")]
pub struct Cli {
    #[arg(long, global = true)]
    pub repo: Option<std::path::PathBuf>,
    #[command(subcommand)]
    pub cmd: Commands,
}

// 面向用户的子命令集合。
#[derive(Subcommand, Debug)]
pub enum Commands {
    CheckIgnore {
        #[arg(required = true)]
        paths: Vec<String>,
    },
    Restore {
        path: String,
        #[arg(long)]
        source: Option<String>,
        #[arg(long)]
        staged: bool,
    },
    RecoverWorktree {
        #[arg(long)]
        abort: bool,
    },
    Stash {
        #[command(subcommand)]
        action: StashAction,
    },
    Doctor {
        #[arg(long)]
        json: bool,
    },
    Stats {
        #[arg(long)]
        json: bool,
    },
    /// Configure named remotes (SSH or local subprocess transport).
    Remote {
        #[command(subcommand)]
        action: Option<RemoteAction>,
    },
    Push {
        remote: Option<String>,
        refspecs: Vec<String>,
        #[arg(short = 'u', long)]
        set_upstream: bool,
        #[arg(long)]
        atomic: bool,
        #[arg(long)]
        tags: bool,
        #[arg(long="force-with-lease", action=clap::ArgAction::Append)]
        leases: Vec<String>,
    },
    Fetch {
        remote: Option<String>,
        branch: Option<String>,
        #[arg(long)]
        prune: bool,
        #[arg(long)]
        tags: bool,
    },
    Pull {
        remote: Option<String>,
        branch: Option<String>,
        #[arg(long)]
        ff_only: bool,
    },
    Config {
        key: String,
        value: Option<String>,
    },
    Tag {
        name: Option<String>,
        target: Option<String>,
        #[arg(short = 'd', long)]
        delete: bool,
    },
    Audit {
        #[arg(long, default_value = "100")]
        limit: usize,
        #[command(subcommand)]
        action: Option<ReflogAction>,
    },
    Clone {
        url: String,
        destination: std::path::PathBuf,
        #[arg(short, long)]
        branch: Option<String>,
    },
    /// Protocol-only stdio service. Root must be configured by the operator.
    Serve {
        #[arg(long)]
        root: Option<std::path::PathBuf>,
        #[arg(long)]
        repository: String,
        #[arg(long)]
        read_only: bool,
    },
    Upgrade {
        /// Confirm ALL old-version myvcs processes are stopped.
        #[arg(long)]
        quiescent: bool,
    },
    Reflog {
        #[arg(long)]
        reference: Option<String>,
        #[command(subcommand)]
        action: Option<ReflogAction>,
    },
    Recover {
        oid: String,
        branch: String,
    },
    Init {
        path: Option<std::path::PathBuf>,
        #[arg(long)]
        bare: bool,
        #[arg(long)]
        empty: bool,
        #[arg(long)]
        team: bool,
    },
    Add {
        paths: Vec<String>,
    },
    Rm {
        paths: Vec<String>,
    },
    Status,
    Diff {
        #[arg(long)]
        staged: bool,
        path: Option<String>,
    },
    Commit {
        #[arg(short, long)]
        message: String,
    },
    Log {
        #[arg(long)]
        graph: bool,
        #[arg(long)]
        all: bool,
    },
    Reset {
        commit: String,
        #[arg(long)]
        hard: bool,
    },
    Branch {
        name: Option<String>,
        #[arg(short = 'd', long)]
        delete: Option<String>,
        #[arg(long, conflicts_with = "unset_upstream")]
        set_upstream_to: Option<String>,
        #[arg(long)]
        unset_upstream: bool,
    },
    Checkout {
        target: String,
    },
    Merge {
        #[arg(required_unless_present = "abort", conflicts_with = "abort")]
        branch: Option<String>,
        /// Restore the original files and index of an interrupted/conflicted merge.
        #[arg(long)]
        abort: bool,
    },
    Gc,
    Verify,
}

#[derive(Subcommand, Debug)]
pub enum RemoteAction {
    Add { name: String, url: String },
    SetUrl { name: String, url: String },
    Remove { name: String },
}

#[derive(Subcommand, Debug)]
pub enum ReflogAction {
    Expire {
        #[arg(long)]
        keep: usize,
    },
}

#[derive(Subcommand, Debug)]
pub enum StashAction {
    Push {
        #[arg(short, long, default_value = "work in progress")]
        message: String,
        #[arg(long)]
        include_untracked: bool,
    },
    List,
    Show {
        #[arg(default_value = "0")]
        entry: String,
    },
    Apply {
        #[arg(default_value = "0")]
        entry: String,
    },
    Pop {
        #[arg(default_value = "0")]
        entry: String,
    },
    Drop {
        #[arg(default_value = "0")]
        entry: String,
    },
}
