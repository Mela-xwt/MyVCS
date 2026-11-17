"""Bind raw P1 measurements and validation logs to the exact source snapshot."""
import hashlib
import json
from pathlib import Path
import re
import statistics
import subprocess
import tarfile

root = Path(__file__).resolve().parent.parent
out = root / 'collaboration-p1'
def git(*args):
    return subprocess.check_output(['git', *args], cwd=root, text=True).strip()
code_commit = git('log', '-1', '--format=%H', '--', 'src', 'tests', 'Cargo.toml', 'Cargo.lock')
baseline = git('rev-list', '--max-parents=0', 'HEAD')
files = [root/'Cargo.toml', root/'Cargo.lock', *sorted((root/'src').rglob('*.rs')), *sorted((root/'tests').rglob('*.rs'))]
with tarfile.open(out/'candidate-source.tar.gz') as t:
    for p in files:
        assert t.extractfile(p.relative_to(root).as_posix()).read() == p.read_bytes(), p
manifest = {'baseline_commit': baseline, 'implementation_commit': code_commit,
            'branch': 'feature/p1-extensions', 'host_date': '2026-10-28',
            'source_baseline': 'Current stage3 worktree; user confirms external 3.zip is reference only',
            'code_matches_tested_archive': True,
            'files_sha256': {p.relative_to(root).as_posix(): hashlib.sha256(p.read_bytes()).hexdigest() for p in files},
            'archives_sha256': {n: hashlib.sha256((out/n).read_bytes()).hexdigest() for n in ['candidate-source.tar.gz','baseline-source.tar.gz']}}
(out/'source-manifest.json').write_text(json.dumps(manifest, indent=2)+'\n', encoding='utf-8')
counts = {}
for filename in ['all-tests.log', 'server-build-tests.log']:
    s = (out/filename).read_text(encoding='utf-8')
    assert 'test result: FAILED' not in s
    counts[filename] = sum(map(int, re.findall(r'test result: ok\. (\d+) passed;', s)))
    assert counts[filename] == 148, counts
records = json.loads((out/'measurements/results.json').read_text())
environment = json.loads((out/'measurements/manifest.json').read_text())
assert len(records) == 18
rows = []
summary = []
for scenario in ['text', 'binary', 'unrelated-random']:
    expected = [x['sha256'] for x in json.loads((out/f'measurements/{scenario}-versions.json').read_text())]
    for mode in ['old', 'new']:
        rs = [r for r in records if r['scenario']==scenario and r['mode']==mode]
        assert len(rs)==3
        assert all(r['restored_hashes']==expected for r in rs)
        def median(f): return statistics.median(f(r) for r in rs)
        v = {'scenario':scenario, 'mode':mode,
             'depth':median(lambda r:r['stats']['max_reachable_chain_nodes']),
             'bytes':median(lambda r:r['stats']['object_storage_bytes']),
             'gc_ms':median(lambda r:r['gc']['elapsed_ms']),
             'verify_ms':median(lambda r:r['verify']['elapsed_ms']),
             'history_ms':median(lambda r:r['all_history_checkout_ms']),
             'gc_rss_kib':median(lambda r:r['gc']['max_rss_kib']),
             'verify_rss_kib':median(lambda r:r['verify']['max_rss_kib']),
             'checkout_rss_kib':median(lambda r:r['checkout_peak_rss_kib'])}
        summary.append(v)
        rows.append(f"| {scenario} | {mode} | {v['depth']} | {v['bytes']:,} | {v['gc_ms']:.2f} | {v['verify_ms']:.2f} | {v['history_ms']:.2f} | {v['gc_rss_kib']} / {v['verify_rss_kib']} / {v['checkout_rss_kib']} |")
(out/'measurements/summary.json').write_text(json.dumps(summary,indent=2)+'\n')
table = '\n'.join(rows)
report = f'''# P1 Delta 深度优化：openEuler 对照实验

## 版本与环境

- 客户端记录日期：2026-10-28；虚拟机时钟有偏差，原始时间戳保留，耗时使用单调时钟。
- 基线：`{baseline}`；最终功能源码/测试提交：`{code_commit}`，分支 `feature/p1-extensions`。
- old/new 为上述源码分别在同一服务器构建的 Release 程序。逐文件源码 SHA256 与服务器核对记录见 `collaboration-p1/source-manifest.json`、`server-demo.log`。后续文档提交不改变测量源码。
- old 二进制 SHA256：`{environment['old_binary_sha256']}`。
- new 二进制 SHA256：`{environment['new_binary_sha256']}`。
- openEuler 22.03 LTS，内核 5.10.0-60.18.0.50.oe2203.x86_64，ext4，Rust/Cargo 1.99.0。
- 虚拟机暴露 4 个逻辑 CPU，AMD Ryzen 9 7845HX；MemTotal 3,461,636 KiB。完整 `/proc/cpuinfo`、`meminfo` 与 OS 信息见原始 manifest。

## 输入与实验口径

固定随机种子 28102026，每组 18 个版本。文本由 12000 次 `alpha beta gamma delta\\n` 组成，初始 276000 字节，每版局部改 8 字节；二进制为 1 MiB 固定随机内容，每版局部改 8 字节；无关随机数据每版独立生成 256 KiB。

使用旧程序创建同一组输入仓库，每轮分别复制给 old/new。3 正式轮，无预热轮；轮间交替 old/new 顺序。未清理 OS page cache，数据创建与复制会预热缓存，不是冷缓存实验。两侧统一 `MYVCS_FORCE_DEEP_DELTA=1`，其他 MYVCS 环境变量移除；未固定 CPU 频率或隔离其他系统负载。

每次记录 GC、verify 的端到端 wall time 和 GNU time 进程峰值 RSS；依次 checkout 所有 18 个历史版本并校验文件 SHA256。历史恢复耗时列为该轮 18 次 checkout 总和，包含启动、对象读取、工作文件写入与 GNU time 开销，不是纯 delta 解码微基准。RSS 的 checkout 项先取每轮 18 次进程中的最大值，再对 3 轮取中位数。

## 实测结果

以下均为 3 轮中位数；B 为对象文件长度合计，深度含完整基础节点。原始每轮数值完整保留，未剔除离群点。

| 数据组 | 版本 | 最大链节点 | 对象存储 B | GC ms | verify ms | 18 次恢复合计 ms | 峰值 RSS KiB：GC / verify / checkout |
| --- | --- | ---: | ---: | ---: | ---: | ---: | --- |
{table}

本次 18 次 GC/verify 流程均成功，324 次历史文件 SHA256 比较全部一致。文本/局部二进制的新链最大值由 18 降为 8；随机无关版本保持完整对象，深度为 1。额外自动化测试覆盖已有 16 节点链读取、重复 GC、基础缺失和损坏。

## 可支持的结论与限制

新建链深度约束得到验证，完整对象回退及历史一致性得到验证。更短链需要更多完整基础：文本和局部二进制存储均增加（精确字节数见上表），随机数据占用相同。文本样本的历史恢复中位数有所降低，但不能外推到二进制或所有仓库。

不能把“深度降低”写成“所有操作更快/更省内存”。本实验中 GC、verify 和局部二进制历史恢复没有稳定的普遍优势；进程 RSS 也没有证明降低。只有 3 轮、包含进程和文件 I/O 开销，适合说明该固定样本的权衡，不足以证明一般性能提升。本轮 F04 的主要收益是限制未来读取链增长；若课程要求明确吞吐加速，应继续研究候选基础选择/缓存并另做实验，不能把本次结果包装成加速成果。

## 复现

在有 Git 历史的工程中，将基线提交检出到另一个目录并分别构建，或解压交付的 `collaboration-p1/baseline-source.tar.gz` 后构建。基准脚本要求两个已有二进制：

```bash
cargo build --release --locked
python3 collaboration-p1/benchmark.py \\
  --old /path/to/baseline/target/release/myvcs \\
  --new /path/to/p1/target/release/myvcs \\
  --out /tmp/myvcs-p1-measurements --rounds 3
```

原始资料：`collaboration-p1/measurements/results.json`、`manifest.json`、三组 `*-versions.json`、`summary.json`，以及 `server-benchmark.log`。`make_report.py` 从原始结果生成本表并核对测试数量、历史哈希和源码快照。
'''
(root/'docs/benchmark.md').write_text(report,encoding='utf-8',newline='\n')
print('PASS evidence summary:', counts, 'historical hash comparisons:',18*18, 'code:',code_commit)
