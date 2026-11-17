# F05：Doctor 与 Stats

```bash
myvcs doctor
myvcs doctor --json
myvcs --repo /path/to/repository doctor --json
myvcs stats
myvcs stats --json
```

JSON 使用 `schema_version: 1`；文本模式按字段输出。doctor 可在仓库外运行，显式 `--repo` 必须有效。它报告 OS/内核/架构、文件系统、NUMA 节点数、PSI memory 文件、编译时 I/O 能力、现有缓冲策略与限制。

`mmap_compiled`、`posix_fadvise_compiled`、`madvise_compiled` 仅表示构建包含该能力，不证明调用成功或带来收益。ETMem 和运行时 I/O hints 的有效性无法可靠证明，输出 null 与说明。Linux 以外缺少 `/proc`/`/sys` 的字段为 null。缓冲区策略沿用现有 util：自动文件系统探测使用进程当前目录，不一定是 `--repo` 指定目录，JSON 同时输出 `buffer_probe_path`。

## Stats 口径

| 字段 | 含义 |
| --- | --- |
| commits_reachable | GC 保护根能到达的提交，包含 stash/恢复辅助提交及保留 reflog |
| blob_references | 提交树与索引中的引用次数，不是去重 Blob 数 |
| local_branches / remote_branches / tags | 对应引用命名空间中的条目数 |
| stashes | 当前有效 stash 引用数，不含已 drop 的 reflog 记录 |
| objects_total | 对象目录中合法 OID 文件数，包含尚未 GC 的不可达对象 |
| objects_reachable | 按 GC 相同保护规则得到的去重对象数，包含物理 delta 基础 |
| object_storage_bytes | 所有对象文件长度之和，不是文件系统块占用或整个 .myvcs 大小 |
| index_entries / index_removed | 暂存新增/修改与删除条目数；bare 仓库为 0 |
| reachable_delta_objects | 可达对象中 delta 编码对象数 |
| max_reachable_chain_nodes | 可达链最大节点数，完整基础计 1；无对象时 0 |

统计会读取并验证可达历史和 delta 依赖，不是常数时间的轻量命令；损坏对象会导致非零退出。不会修改文件、索引、引用或对象内容；stats 沿用仓库读写串行锁，缺失锁文件时可能创建 `repo.lock`。未完成 work journal 时要求先恢复。doctor 不尝试修复或安装优化组件。

实测 openEuler：ext4、NUMA 节点 1、PSI memory 文件不存在、ETMem null、缓冲区 131072 字节。证据见 `collaboration-p1/server-benchmark.log` 与 `server-demo.log`；这些是本次机器观察，不是所有 openEuler 系统的默认值。
