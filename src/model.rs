use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

// 暂存区：记录待提交的新增/修改，以及待删除路径。
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Index {
    pub entries: BTreeMap<String, String>,
    #[serde(default)]
    pub removed: BTreeSet<String>,
}

// `status` 使用的缓存，避免对未变化文件重复哈希。
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct StatusCache {
    pub entries: BTreeMap<String, StatusCacheEntry>,
}

// 工作区文件指纹缓存项。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StatusCacheEntry {
    pub mtime_secs: u64,
    pub size: u64,
    pub hash: String,
}

// 持久化在对象库中的提交对象。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Commit {
    pub tree: BTreeMap<String, String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub parents: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent: Option<String>,
    pub message: String,
    pub author: String,
    pub timestamp: i64,
}

pub const DELTA_MAGIC: &[u8] = b"MYVCS_DELTA_V1\\n";

// 用于紧凑存储 blob 的增量指令序列。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "op")]
pub enum DeltaOp {
    #[serde(rename = "copy")]
    Copy { offset: usize, len: usize },
    #[serde(rename = "insert")]
    Insert { data: Vec<u8> },
}

// 引用基础对象的序列化增量 blob。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeltaBlob {
    pub base: String,
    pub ops: Vec<DeltaOp>,
    pub size: usize,
}
