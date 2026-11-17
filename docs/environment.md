# 构建环境与支持范围

本轮主验收环境为 openEuler 22.03 LTS x86_64，内核 `5.10.0-60.18.0.50.oe2203.x86_64`，ext4；Rust/Cargo 1.99.0。另在 Windows 主机的 WSL Ubuntu 上执行完整回归、格式与 Clippy 检查。工程使用 Rust 2024 edition，应使用支持该 edition 且满足 Cargo.lock 依赖要求的工具链。

```bash
cargo build --release --locked
cargo test --locked
cargo fmt --check
cargo clippy --locked --all-targets
python3 collaboration-p1/demo.py --bin target/release/myvcs
```

首次构建有网络时下载 Cargo 依赖，之后可追加 `--offline`。Python 演示/基准只用标准库（基准要求 Python 3.9+、Linux `/proc`、GNU `/usr/bin/time`）；真实 SSH 验收还需要 OpenSSH ssh/ssh-keygen/sshd。客户端远程采集脚本 `remote_validate.py`、`final_evidence.py` 使用 Paramiko、交互式密码及已确认的 known_hosts；不要把密码或私钥放进脚本。脚本默认使用本项目已授权实验服务器，其他同学应修改主机/路径与信任配置。

## 平台限制

- Linux/openEuler 是已验收目标；WSL 验收不能代替原生 Windows 验收。
- `/proc`、`/sys`、PSI 和 fadvise/madvise 属于 Linux 相关路径，其他平台按可用性降级/返回 unknown。
- Windows 没有本项目可移植目录 fsync 保证；文件刷新、原子重命名、大小写、保留文件名和符号链接策略须在目标平台单独验证。macOS 未测。
- 文件执行位、符号链接、重命名识别仍按既有 MyVCS 支持范围处理；本轮不扩展 Git 对象/协议互通。
- SSH 继续为团队协作主通道，HTTPS 未实现；stash 是本地状态，不自动远程传输。
- 当前阶段 3 和 P1 同仓库格式，无法靠格式标记阻止旧工具绕过 P1 journal 检查。维护中断恢复时必须统一使用新版。

基线与新版均有 17 条不同的既有 Clippy 警告（库 16、旧测试 1）；常规 Clippy 成功，`-D warnings` 严格门禁未通过。原始日志保留了这一限制，未声明零警告。

客户端记录日期为 2026-10-28；实验虚拟机时钟落后，因此测试日志中的 Unix 时间/提交时间不应与客户端日期混作同一时钟。未调整服务器时间；耗时使用单调时钟测量。
