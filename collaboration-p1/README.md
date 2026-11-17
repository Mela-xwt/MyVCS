# MyVCS 五项 P1 扩展：实现与验收报告

## 交付结论

本轮已在现有阶段 3 MyVCS 上实现 F01～F05。原有 13 条基础命令及 SSH 团队能力保留；本地 WSL 和 openEuler 均通过 **148 项自动化测试（原有 131 + 新增 17）**，真实 SSH、阶段 3 双向兼容与旧 GC 保护 stash 验证通过。F04 证明了新链深度约束，**没有证明普遍加速或降低峰值内存**。

用户已说明设计文档中的 `3.zip` 为其他参考资料，本轮只需实现相应功能。因此实际基线为本工作区已完成的阶段 3，不声称已审阅或移植 1～6.zip。

分支：`feature/p1-extensions`。基线提交 `85c8e1e`；最终功能源码及测试提交 `b9237a7fdbfa9986a856b4190c33587b9f9a44a2`。`source-manifest.json` 给出完整基线提交、源码与测试 SHA256；服务器逐文件核对结果见 `server-demo.log`。文档、证据与打包可有后续提交，但不改变这份已测代码。没有创建 GitHub 仓库或编造链接。

## 五项功能与实际范围

| 项目 | 完成内容 | 明确边界 |
| --- | --- | --- |
| F01 忽略规则 | 根目录 .myvcsignore、规则来源查询；status/add/stash 统一过滤；已跟踪文件保留 | 简化 glob，无嵌套规则/字符类/转义；不宣称扫描加速 |
| F02 单文件恢复 | HEAD/指定提交、仅撤销暂存、真正的暂存差异、覆盖检查及持久化恢复 | 默认来源 HEAD；一次一个文件；显式 restore 丢弃该跟踪文件未暂存改动 |
| F03 Stash | push/list/show/apply/pop/drop；独立保存索引/工作区；可选未跟踪文件；GC 根与中断恢复 | 本地快照；数字位置会变，稳定 ID 不变；文件级冲突拒绝，无自动文本冲突合并 |
| F04 Delta 深度 | 新 GC 链最多 8 节点；旧读预算默认 64；既有基础保留、收益与哈希校验、完整对象回退 | 旧深链不强制缩短；部分数据占用增加、GC/verify 更慢 |
| F05 Doctor/Stats | 真实环境和配置、明确 unknown、JSON v1、引用/对象/空间/链统计 | 编译能力不等于运行有效；stats 包含 stash/reflog 辅助提交，非纯分支提交数 |

详细命令和语义见 [ignore](../docs/features/ignore.md)、[restore/diff](../docs/features/restore-diff.md)、[stash](../docs/features/stash.md)、[delta-depth](../docs/features/delta-depth.md)、[doctor/stats](../docs/features/doctor-stats.md)。

## 验收证据

| 验证 | 结果与证据 |
| --- | --- |
| 冻结阶段 3 基线 | 131 测试通过；baseline-tests.log、baseline-source.tar.gz |
| 最终 WSL 回归 | 148 通过、0 失败；all-tests.log |
| 最终 openEuler 回归 + Release | 148 通过、Release 成功；server-build-tests.log、server-build-exit-code.txt |
| 格式检查 | cargo fmt --check 成功，format.log 空输出 |
| 静态检查 | 常规 Clippy 成功；库 16 + 旧测试 1 条既有警告；clippy-warnings.log 与 baseline-clippy.log。严格 -D warnings 未通过，不宣称零警告 |
| 真 SSH | 独立密钥 alice/bob/reader/outsider，克隆、并发/过期 push、权限、合并、引用事务、标签、prune、lease、reflog/GC、审计；server-ssh.log |
| 新旧兼容 | 两个真实二进制双向 client/service；full/internal/thin；格式 1/2 旧 GC 保留新版 stash；server-p1-compatibility.log |
| P1 真实 CLI 演示 | 完整命令、输出、断言与二进制 SHA256；server-demo.log |
| Delta 对照 | 3 数据组 × 3 轮 × old/new = 18 组，324 次历史 SHA256 相同；measurements/、server-benchmark.log、[性能报告](../docs/benchmark.md) |

旧 `server-compatibility.log` 记录了误用“阶段 2 → 阶段 3”脚本时的版本假设失败：阶段 3 基线本来支持格式 2，不能要求它拒绝。该记录不是最终兼容结论；最终使用本轮 `compatibility.py`，不通过删除格式保护来迎合错误断言。`p1-tests.log` 为早期 13 项测试记录，最终以全量日志为准。

## 自动化覆盖

新增测试位于 `tests/p1_features.rs`。一个 Rust test 可包含多个故障点/分支，因此 17 是 test 函数数，不能与所有断言次数混算。

1. 忽略规则一致性、否定、已跟踪文件、来源行号与不安全路径。
2. 根锚定、目录重新纳入、Unicode、显式 target 规则。
3. 暂存与未暂存内容区分，单文件恢复，HEAD 与其他条目不变。
4. 未跟踪覆盖、越界及危险符号链接拒绝。
5. restore 的 prepared/file/index 三个进程中断点，继续/回滚及事后新编辑保护。
6. stash 工作区、索引、未跟踪文件精确还原，忽略文件保留，GC 后恢复。
7. pop 失败保留快照、多个记录、drop、稳定 ID。
8. 不同 HEAD 的独立路径三方组合及同路径冲突。
9. stash push 三个中断点、恢复、GC 后 pop。
10. 未出生分支与未跟踪路径碰撞。
11. doctor JSON/unknown、空仓库、多命名空间、GC 前后统计及只读内容检查。
12. GC 新链 ≤8、重复 GC、全部 20 个历史版本恢复。
13. 合法旧 16 节点链可读、缺失基础及损坏 delta 明确失败。
14. 已暂存新增但工作文件已删除时，不删除后来重新创建的未跟踪文件。
15. 损坏 stash 对象、pop 中断/回滚保留快照。
16. 格式 2 drop 后受 reflog 保护，按策略过期后才回收。
17. restore --staged 超出索引元数据预算时，原索引和工作区不变。

## 构建、演示与截图建议

从工程根运行（首次联网下载依赖后才可用 --offline）：

```bash
cargo build --release --locked
cargo test --locked
cargo fmt --check
cargo clippy --locked --all-targets
python3 collaboration-p1/demo.py --bin ./target/release/myvcs
```

演示脚本仅创建新的临时仓库，逐条打印实际命令和结果，保留现场供检查；不会修改现有团队仓库。截取连续终端内容时按 F01、F02、F03、F04/F05 段落分别截图，保持命令、退出码和 PASS 一起可见。故障注入建议用 `cargo test --test p1_features restore_crash -- --nocapture` 等测试日志证明，不能将预期输出做成“实测截图”。本轮交付原始文本日志，没有伪造或生成终端截图。

F04 的 18/8 深度及前后数字以性能报告和原始 JSON 为证据；小型 CLI 演示仓库可能只有完整对象（深度 1），不能用它冒充深链基准。旧 SSH 部署与原有功能验收见 [阶段 3 操作说明](../collaboration-stage3/OPERATIONS.md)。

## 故障恢复、平台和遗留问题

- 工作区采用 journal + 两侧对象 pin + 逐文件原子替换；进程中断通过 `recover-worktree` 或 `recover-worktree --abort`。不是全工作区原子提交，也不能阻止外部编辑器并发写入。
- 有 journal 时只使用新版恢复，阶段 3 旧程序不知道该标记。普通无 journal 场景已测试旧程序 GC 兼容；不代表允许任意时刻混用客户端。
- journal 已删而 pin 尚未删时崩溃，可能留下只增加空间的孤立 pin；未提供自动回收工具。
- stash 文件级冲突保守拒绝；没有 rename 识别或 Git stash 的所有选项。稳定 ID 可跨 push/drop 使用，数字序号仅为当前位置。
- GC 限制链深会增加完整基础对象；本轮不以更短链推断全面性能优势。stats 遍历历史，规模很大时可能较慢。
- 本轮目标 openEuler/Linux 已验证，WSL 另做回归；原生 Windows/macOS 未验收，Windows 目录 fsync 和文件替换/大小写存在平台限制。HTTPS 仍未实现。
- 现有 Clippy 警告未清零；新增功能未替代整个仓库的生产级持久化/外部并发模型。

详见 [故障处理](../docs/troubleshooting.md)、[环境](../docs/environment.md) 和 [架构](../docs/architecture.md)。压缩包含完整源码、测试、文档、实验脚本、原始证据、基线快照与已验收 openEuler 二进制；后者不保证能在不同 libc/架构运行，应优先本机编译。SHA256SUMS 与 package-manifest.json 用于校验归档内容。

## 压缩包与历史

`MyVCS-P1-source-and-evidence.zip` 解压后进入 MyVCS-P1 目录即可按上述命令构建。`collaboration-p1/bin/myvcs-openEuler-x86_64` 为已测服务器程序，解压后在匹配平台执行前可 `chmod +x`；其 SHA256 应与 measurements/manifest.json 的 new_binary_sha256 一致。

压缩包不包含 `.git` 工作目录，但附带 `MyVCS-P1-history.bundle` 保存基线与功能/文档提交。需要检出指定提交时：

```bash
git clone collaboration-p1/MyVCS-P1-history.bundle /tmp/myvcs-p1-history
# 在该克隆中可检出本报告指定基线/功能提交。
```

`package-manifest.json` 记录逐文件摘要，压缩包外 `SHA256SUMS.txt` 记录 ZIP 摘要。压缩包不会包含私人 SSH 密钥、密码、构建缓存或无关课程资料。
