# MyVCS：版本控制与团队协作

Rust 实现的文件版本控制系统。本次 P1 扩展在当前阶段 3 源码上开发；用户已确认 `3.zip` 为外部参考资料，本轮只需实现对应功能，并非移植或复刻该压缩包。

## 构建与验证

推荐 Linux/openEuler 22.03 LTS，Rust 2024 edition 工具链。实测工具链与结果见 [P1 验收报告](collaboration-p1/README.md)。首次构建需要 Cargo 依赖下载；离线运行须先有缓存。

```bash
cargo build --release --locked
cargo test --locked
cargo fmt --check
cargo clippy --all-targets --locked
./target/release/myvcs --help
python3 collaboration-p1/demo.py --bin ./target/release/myvcs
```

使用独立工作目录体验 MyVCS，避免把开发工程的构建目录加入示例仓库：

```bash
mkdir /tmp/myvcs-learning
cd /tmp/myvcs-learning
/path/to/myvcs init --empty
printf 'target/\n*.log\n' > .myvcsignore
/path/to/myvcs add .
/path/to/myvcs commit -m 'initial rules'
```

## 功能入口

原有 `init/add/rm/status/diff/commit/log/reset/branch/checkout/merge/gc/verify` 共 13 条基础命令继续保留。SSH 远程协作、pack 增量、权限、引用事务、合并、reflog 与恢复详见 [阶段 3 说明](collaboration-stage3/README.md)。本轮没有新增 HTTPS。

| P1 功能 | 使用说明 | 代码入口 |
| --- | --- | --- |
| 忽略规则、原因查询 | [ignore](docs/features/ignore.md) | `src/repo/ignore.rs` |
| 单文件恢复、暂存差异 | [restore/diff](docs/features/restore-diff.md) | `src/repo/restore.rs` |
| Stash、故障恢复、GC 保护 | [stash](docs/features/stash.md) | `src/repo/stash.rs` |
| 新 GC delta 深度上限 | [delta-depth](docs/features/delta-depth.md) | `src/repo/gc_verify.rs` |
| 环境诊断、仓库统计 | [doctor/stats](docs/features/doctor-stats.md) | `src/repo/diagnostics.rs` |

[P1 验收与原始证据](collaboration-p1/README.md) · [性能实验](docs/benchmark.md) · [故障处理](docs/troubleshooting.md) · [架构](docs/architecture.md) · [环境及平台限制](docs/environment.md)
