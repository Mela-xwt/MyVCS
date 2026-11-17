# P1 Delta 深度优化：openEuler 对照实验

## 版本与环境

- 客户端记录日期：2026-10-28；虚拟机时钟有偏差，原始时间戳保留，耗时使用单调时钟。
- 基线：`85c8e1e78c662b0b702ff6c776c34e96e64a20e6`；最终功能源码/测试提交：`b9237a7fdbfa9986a856b4190c33587b9f9a44a2`，分支 `feature/p1-extensions`。
- old/new 为上述源码分别在同一服务器构建的 Release 程序。逐文件源码 SHA256 与服务器核对记录见 `collaboration-p1/source-manifest.json`、`server-demo.log`。后续文档提交不改变测量源码。
- old 二进制 SHA256：`5e9e26e4f312fc3b23c74e3ab338c82f7aba4d4b982d6c7a090cad715b5e9e4b`。
- new 二进制 SHA256：`2eb421d1a9e40dedcb5289b506ad8bf7b006bf4c9910cbbc419a2d1105783620`。
- openEuler 22.03 LTS，内核 5.10.0-60.18.0.50.oe2203.x86_64，ext4，Rust/Cargo 1.99.0。
- 虚拟机暴露 4 个逻辑 CPU，AMD Ryzen 9 7845HX；MemTotal 3,461,636 KiB。完整 `/proc/cpuinfo`、`meminfo` 与 OS 信息见原始 manifest。

## 输入与实验口径

固定随机种子 28102026，每组 18 个版本。文本由 12000 次 `alpha beta gamma delta\n` 组成，初始 276000 字节，每版局部改 8 字节；二进制为 1 MiB 固定随机内容，每版局部改 8 字节；无关随机数据每版独立生成 256 KiB。

使用旧程序创建同一组输入仓库，每轮分别复制给 old/new。3 正式轮，无预热轮；轮间交替 old/new 顺序。未清理 OS page cache，数据创建与复制会预热缓存，不是冷缓存实验。两侧统一 `MYVCS_FORCE_DEEP_DELTA=1`，其他 MYVCS 环境变量移除；未固定 CPU 频率或隔离其他系统负载。

每次记录 GC、verify 的端到端 wall time 和 GNU time 进程峰值 RSS；依次 checkout 所有 18 个历史版本并校验文件 SHA256。历史恢复耗时列为该轮 18 次 checkout 总和，包含启动、对象读取、工作文件写入与 GNU time 开销，不是纯 delta 解码微基准。RSS 的 checkout 项先取每轮 18 次进程中的最大值，再对 3 轮取中位数。

## 实测结果

以下均为 3 轮中位数；B 为对象文件长度合计，深度含完整基础节点。原始每轮数值完整保留，未剔除离群点。

| 数据组 | 版本 | 最大链节点 | 对象存储 B | GC ms | verify ms | 18 次恢复合计 ms | 峰值 RSS KiB：GC / verify / checkout |
| --- | --- | ---: | ---: | ---: | ---: | ---: | --- |
| text | old | 18 | 8,187 | 50.50 | 7.36 | 193.47 | 5204 / 4868 / 5200 |
| text | new | 8 | 11,966 | 61.02 | 7.95 | 168.53 | 5396 / 5264 / 5328 |
| binary | old | 18 | 1,056,319 | 360.57 | 21.97 | 817.32 | 8228 / 6192 / 7428 |
| binary | new | 8 | 3,154,986 | 432.70 | 35.88 | 834.45 | 8564 / 7524 / 7628 |
| unrelated-random | old | 1 | 4,725,836 | 4408.83 | 32.47 | 621.26 | 7292 / 4480 / 4748 |
| unrelated-random | new | 1 | 4,725,836 | 4424.63 | 31.69 | 617.32 | 7536 / 4748 / 4876 |

本次 18 次 GC/verify 流程均成功，324 次历史文件 SHA256 比较全部一致。文本/局部二进制的新链最大值由 18 降为 8；随机无关版本保持完整对象，深度为 1。额外自动化测试覆盖已有 16 节点链读取、重复 GC、基础缺失和损坏。

## 可支持的结论与限制

新建链深度约束得到验证，完整对象回退及历史一致性得到验证。更短链需要更多完整基础：文本和局部二进制存储均增加（精确字节数见上表），随机数据占用相同。文本样本的历史恢复中位数有所降低，但不能外推到二进制或所有仓库。

不能把“深度降低”写成“所有操作更快/更省内存”。本实验中 GC、verify 和局部二进制历史恢复没有稳定的普遍优势；进程 RSS 也没有证明降低。只有 3 轮、包含进程和文件 I/O 开销，适合说明该固定样本的权衡，不足以证明一般性能提升。本轮 F04 的主要收益是限制未来读取链增长；若课程要求明确吞吐加速，应继续研究候选基础选择/缓存并另做实验，不能把本次结果包装成加速成果。

## 复现

在有 Git 历史的工程中，将基线提交检出到另一个目录并分别构建，或解压交付的 `collaboration-p1/baseline-source.tar.gz` 后构建。基准脚本要求两个已有二进制：

```bash
cargo build --release --locked
python3 collaboration-p1/benchmark.py \
  --old /path/to/baseline/target/release/myvcs \
  --new /path/to/p1/target/release/myvcs \
  --out /tmp/myvcs-p1-measurements --rounds 3
```

原始资料：`collaboration-p1/measurements/results.json`、`manifest.json`、三组 `*-versions.json`、`summary.json`，以及 `server-benchmark.log`。`make_report.py` 从原始结果生成本表并核对测试数量、历史哈希和源码快照。
