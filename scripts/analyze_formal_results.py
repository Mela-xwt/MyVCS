"""Offline audit and Chinese Markdown report; never reruns or modifies experiments."""
import csv
import hashlib
import json
import math
from pathlib import Path
import re
import statistics as st
import tarfile

ROOT = Path(__file__).resolve().parent.parent
DATA = ROOT / 'formal-main01'
OUT = ROOT / 'formal-analysis'
CASES = {'random20': (20, 5), 'ab20': (20, 5), 'random128': (128, 2), 'ab128': (128, 2)}
NAMES = {'random20': '20 MiB 随机', 'ab20': '20 MiB A/B', 'random128': '128 MiB 随机', 'ab128': '128 MiB A/B'}
TOOLS = ('myvcs', 'git')


def read(path):
    return json.loads(path.read_text(encoding='utf-8'))


def close(a, b):
    assert math.isclose(float(a), float(b), rel_tol=1e-10, abs_tol=1e-7), (a, b)


def sha(content):
    return hashlib.sha256(content).hexdigest()


def table(headers, rows):
    return '\n'.join(['| ' + ' | '.join(headers) + ' |',
                      '| ' + ' | '.join('---' for _ in headers) + ' |'] +
                     ['| ' + ' | '.join(map(str, r)) + ' |' for r in rows]) + '\n'


def main():
    rows = list(csv.DictReader((DATA / 'results.csv').open(encoding='utf-8', newline='')))
    manifest, completed, supplied = [read(DATA / f) for f in ('manifest.json', 'completed.json', 'aggregate.json')]
    assert len(rows) == 160 and completed['rounds'] == 20
    assert completed['case_trials'] == 80 and completed['tool_trials'] == 160
    identities = {(r['case'], int(r['round']), r['tool']) for r in rows}
    assert identities == {(c, n, t) for c in CASES for n in range(1, 21) for t in TOOLS}
    assert len(identities) == len(rows)

    # Rebuild expected versions independently from the archived raw input bytes.
    inputs = {}
    with tarfile.open(ROOT / 'pilot-repro-results.tar.gz', 'r:gz') as archive:
        for member in archive:
            if member.isfile() and member.name.startswith('benchmarks/') and '/input/' in member.name:
                inputs[member.name] = archive.extractfile(member).read()
    expected = {}
    for case, (size, updates) in CASES.items():
        prefix = 'benchmarks/' + case + '-pilot01/input/'
        for name, digest in manifest['inputs'][case].items():
            assert sha(inputs[prefix + name]) == digest
        data = bytearray(inputs[prefix + 'data.bin'])
        assert len(data) == size * 1024**2
        expected[case] = [sha(data)]
        for n in range(1, updates + 1):
            patch = inputs[prefix + ('patch-%02d.bin' % n)]
            offset = n * 1024**2
            data[offset:offset + len(patch)] = patch
            expected[case].append(sha(data))
    del inputs, data

    measurements = []
    snapshots = []
    restores = 0
    gc_optimized = {}
    for row in rows:
        case, tool, number = row['case'], row['tool'], int(row['round'])
        assert row['first_tool'] == ('myvcs' if number % 2 else 'git')
        size, updates = CASES[case]
        trial = DATA / ('trial-%02d-%s' % (number, case))
        folder = trial / tool
        phases = ['initial'] + ['update%02d' % n for n in range(1, updates + 1)]
        records = {}
        for key in [p + '-' + op for p in phases for op in ('add', 'commit')] + ['gc', 'verify']:
            record = read(folder / (key + '.json'))
            assert record['status'] == 'ok' and record['exit_code'] == 0
            assert record['elapsed_ms'] > 0 and record['max_rss_KiB'] >= 0
            assert (folder / (key + '.log')).is_file()
            records[key] = record
            measurements.append({'case': case, 'round': number, 'tool': tool, 'phase': key,
                                 'elapsed_ms': record['elapsed_ms'], 'max_rss_KiB': record['max_rss_KiB'],
                                 'path': str((folder / (key + '.json')).relative_to(ROOT))})
            for side in ('before', 'after'):
                snapshots.append(read(folder / (key + '-system-' + side + '.json')))
        for metric, key in [('initial_add_ms', 'initial-add'), ('initial_commit_ms', 'initial-commit'),
                            ('gc_ms', 'gc'), ('verify_ms', 'verify')]:
            close(row[metric], records[key]['elapsed_ms'])
        close(row['initial_total_ms'], records['initial-add']['elapsed_ms'] + records['initial-commit']['elapsed_ms'])
        add = [records[p + '-add']['elapsed_ms'] for p in phases[1:]]
        commit = [records[p + '-commit']['elapsed_ms'] for p in phases[1:]]
        close(row['update_add_mean_ms'], st.mean(add))
        close(row['update_commit_mean_ms'], st.mean(commit))
        close(row['update_total_mean_ms'], st.mean(a + b for a, b in zip(add, commit)))
        close(row['submission_peak_rss_KiB'], max(v['max_rss_KiB'] for k, v in records.items()
                                                 if k.endswith('-add') or k.endswith('-commit')))
        for phase in ('gc', 'verify'):
            close(row[phase + '_peak_rss_KiB'], records[phase]['max_rss_KiB'])
        for side in ('before', 'after'):
            close(row['object_bytes_' + side + '_gc'], read(trial / ('storage-' + side + '-gc.json'))[tool])
        close(row['saving_vs_full_versions_pct'], 100 * (1 - float(row['object_bytes_after_gc']) / ((updates + 1) * size * 1024**2)))
        summary = next(r for r in read(trial / 'summary.json') if r['tool'] == tool)
        for key, value in row.items():
            if key in ('case', 'tool', 'first_tool'):
                assert summary[key] == value
            else:
                close(summary[key], value)
        for n, phase in enumerate(phases):
            pair = read(trial / (phase + '-input-hashes.json'))
            assert pair['myvcs'] == pair['git'] == expected[case][n]
        restored = read(folder / 'restored-versions.json')
        assert len(restored) == int(row['restored_versions_checked']) == updates + 1
        assert [r['sha256'] for r in restored] == expected[case]
        restores += len(restored)
        for n in range(len(restored)):
            assert (folder / ('restore-%02d.log' % n)).is_file()
        status = (folder / 'final-status.log').read_text()
        if tool == 'myvcs':
            assert 'verify ok:' in (folder / 'verify.log').read_text()
            assert status.strip() == 'On branch main\n\nNo staged changes\n\nNo unstaged changes\n\nNo untracked files'
            log = (folder / 'gc.log').read_text()
            assert 'deep-delta optimization skipped' not in log
            match = re.search(r'optimized (\d+) blob\(s\), removed (\d+) unreachable', log)
            assert match
            gc_optimized.setdefault(case, set()).add((int(match[1]), int(match[2])))
        else:
            assert 'On branch main' in status and 'nothing to commit, working tree clean' in status

    metrics = [k for k in rows[0] if k not in ('case', 'round', 'tool', 'first_tool', 'restored_versions_checked')]
    stats = {}
    statistics_rows = []
    for case in CASES:
        stats[case] = {}
        for tool in TOOLS:
            selected = [r for r in rows if r['case'] == case and r['tool'] == tool]
            assert len(selected) == 20
            stats[case][tool] = {}
            for metric in metrics:
                values = [float(r[metric]) for r in selected]
                result = {'mean': st.mean(values), 'median': st.median(values), 'stdev': st.stdev(values),
                          'min': min(values), 'max': max(values)}
                stats[case][tool][metric] = result
                for key, val in result.items():
                    close(val, supplied[case][tool]['metrics'][metric][key])
                statistics_rows.append({'case': case, 'tool': tool, 'metric': metric, 'n': 20, **result})

    def proc(text):
        return {line.split()[0].rstrip(':'): int(line.split()[1]) for line in text.splitlines()}
    start = proc(manifest['initial_system']['vmstat'])
    end = proc(completed['final_system']['vmstat'])
    swap_used = [proc(s['meminfo'])['SwapTotal'] - proc(s['meminfo'])['SwapFree'] for s in snapshots]
    long_tails = []
    for case, tool, round_no, phase in [('random20', 'myvcs', 5, 'initial-commit'), ('ab128', 'git', 9, 'gc')]:
        folder = DATA / ('trial-%02d-%s' % (round_no, case)) / tool
        b, a = [read(folder / (phase + '-system-' + side + '.json')) for side in ('before', 'after')]
        before, after = proc(b['vmstat']), proc(a['vmstat'])
        long_tails.append({'case': case, 'tool': tool, 'round': round_no, 'phase': phase,
                           'elapsed_ms': read(folder / (phase + '.json'))['elapsed_ms'],
                           'load_before': b['loadavg'].strip(), 'load_after': a['loadavg'].strip(),
                           'pswpin_delta': after['pswpin'] - before['pswpin'],
                           'pswpout_delta': after['pswpout'] - before['pswpout'],
                           'pgmajfault_delta': after['pgmajfault'] - before['pgmajfault']})
    # Descriptive outlier flags (not rejection criteria) for every group/metric.
    outliers = []
    for case in CASES:
        for tool in TOOLS:
            for metric in ('initial_total_ms', 'update_total_mean_ms', 'gc_ms'):
                selected = [r for r in rows if r['case'] == case and r['tool'] == tool]
                values = [float(r[metric]) for r in selected]
                q1, _, q3 = st.quantiles(values, n=4, method='inclusive')
                lo, hi = q1 - 1.5 * (q3 - q1), q3 + 1.5 * (q3 - q1)
                outliers += [{'case': case, 'tool': tool, 'round': int(r['round']), 'metric': metric,
                              'value': float(r[metric]), 'low_fence': lo, 'high_fence': hi}
                             for r in selected if not lo <= float(r[metric]) <= hi]
    audit = {'status': 'pass', 'rows': len(rows), 'case_trials': 80, 'rounds_per_case_tool': 20,
             'timed_commands_checked': len(measurements), 'system_snapshots_checked': len(snapshots),
             'restored_version_hashes_checked': restores, 'aggregate_matches_raw_records': True,
             'inputs_reconstructed_from_pilot_archive': True,
             'gc_optimized_removed_counts': {k: sorted(v) for k, v in gc_optimized.items()},
             'swap_pages_in_delta_entire_run': end['pswpin'] - start['pswpin'],
             'swap_pages_out_delta_entire_run': end['pswpout'] - start['pswpout'],
             'swap_used_KiB_snapshot_min_max': [min(swap_used), max(swap_used)],
             'long_tail_records': long_tails, 'iqr_flags_no_exclusion': outliers,
             'results_csv_sha256': sha((DATA / 'results.csv').read_bytes())}
    OUT.mkdir(exist_ok=True)
    (OUT / 'audit.json').write_text(json.dumps(audit, ensure_ascii=False, indent=2), encoding='utf-8')
    for name, values in [('statistics.csv', statistics_rows), ('all_measurements.csv', measurements)]:
        with (OUT / name).open('w', encoding='utf-8-sig', newline='') as f:
            w = csv.DictWriter(f, fieldnames=list(values[0])); w.writeheader(); w.writerows(values)

    def val(c, t, m, k='mean'):
        return stats[c][t][m][k]

    def ms(c, t, m):
        return '%.2f ± %.2f' % (val(c,t,m), val(c,t,m,'stdev'))

    lines = ['# MyVCS 正式性能复现报告', '',
             '> 实验批次：formal-main01；虚拟机记录开始时间：' + manifest['started_local'] + '。',
             '> 本报告根据用户实际运行后下载的记录进行离线核对，未重跑实验、未修改原始结果、未删除慢样本。', '',
             '## 1. 完成情况与证据核对', '',
             '四个场景各完成 20 轮，每轮新建两个仓库，共 80 个场景轮次、160 个工具轮次。复现了论文中的工作负载类别与重复次数；硬件、计时、缓存与部分未说明参数并非完全相同，因此不称为逐项完全一致的复现。', '',
             f'- 核对 {len(measurements)} 条计时 JSON：全部 status=ok、exit_code=0，耗时和 RSS 与 CSV 一致。',
             f'- 核对 {len(snapshots)} 份系统快照，以及所有 trial summary、对象大小和 aggregate.json：统计一致。',
             '- 两个工具各完成 80 次完整性检查；全部回到 main，最终工作区干净。',
             f'- 从已备份的原始文件与补丁重新构造版本，核对 {restores} 条 GC 后历史恢复哈希：全部匹配。该结论基于保存的恢复日志和哈希，临时仓库已由脚本清理。',
             '- 未发现 MyVCS 因负载跳过增量 GC。20 MiB 组每轮优化 5 个 blob，128 MiB 组每轮优化 2 个，均移除 0 个不可达对象。',
             '- 每个场景中，MyVCS 先执行 10 轮、Git 先执行 10 轮。保留全部 20 轮，包括两条明显长尾记录。', '',
             '原始数据：[results.csv](../formal-main01/results.csv)；离线核对：[audit.json](audit.json)；完整统计：[statistics.csv](statistics.csv)；全部逐命令数据：[all_measurements.csv](all_measurements.csv)。', '',
             '## 2. 环境和测量方法', '',
             table(['项目','本次实验'], [
                 ['系统 / 内核','openEuler 22.03 LTS / 5.10.0-60.18.0.50.oe2203.x86_64'],
                 ['执行环境','VMware 虚拟机；客户机记录 CPU 为 AMD Ryzen 9 7845HX，4 个可用 vCPU'],
                 ['客户机内存','/proc/meminfo: MemTotal 3,461,636 KiB（约 3.30 GiB）；配置内存为 4 GB'],
                 ['Git / Rust / Python',(DATA/'git-version.log').read_text().strip()+' / '+(DATA/'rustc-version.log').read_text().strip()+' / Python 3.9.9'],
                 ['MyVCS','release 二进制；与预实验基线 SHA-256 相同，未修改原项目'],
                 ['数据','固定数据与补丁，20 轮复用；随机/A/B 同规模组使用相同补丁'],
                 ['缓存','未清空页缓存；复制及哈希检查预热输入，不是冷缓存实验'],
                 ['计时','perf_counter_ns 包含 GNU time + 目标命令启动及运行；不含 Python 启动、输入生成和哈希检查'],
                 ['内存','GNU time %M，Linux 峰值 RSS，单位 KiB；提交、GC、验证分别记录'],
                 ['输出','目标程序 stdout/stderr 写入日志；保留默认 git commit 输出行为'],
                 ['Git 配置','禁用全局/系统配置和初始化模板，关闭提交签名与自动 GC；普通 git gc'],
                 ['MyVCS 配置','清除继承的 MYVCS_* 覆盖项，保留源码默认策略'],
             ]),
             '二进制 SHA-256：`' + manifest['binary_sha256'] + '`。完整记录见 [manifest.json](../formal-main01/manifest.json)。', '',
             table(['场景','数据构造','增量次数','每次覆盖','偏移'],[
                 ['20 MiB 随机','保存的 /dev/urandom 数据','5','4 KiB','1、2、3、4、5 MiB'],
                 ['20 MiB A/B','10 MiB A + 10 MiB B','5','4 KiB 随机补丁','同上'],
                 ['128 MiB 随机','保存的 /dev/urandom 数据','2','64 KiB','1、2 MiB'],
                 ['128 MiB A/B','64 MiB A + 64 MiB B','2','64 KiB 随机补丁','同上'],
             ]),
             '每个工具每场景以 20 轮为统计单位。增量指标先在每轮内对 5 次或 2 次修改求均值，再统计这 20 个轮均值。下表“±”为轮间样本标准差，不是置信区间。结果表统一使用二进制 MiB/KiB；与论文比较时显式区分十进制 MB/KB。', '',
             '## 3. 提交耗时', '',
             table(['场景','MyVCS 首次总耗时 ms','Git 首次总耗时 ms','MyVCS 增量总耗时 ms','Git 增量总耗时 ms'],
                   [[NAMES[c],ms(c,'myvcs','initial_total_ms'),ms(c,'git','initial_total_ms'),
                     ms(c,'myvcs','update_total_mean_ms'),ms(c,'git','update_total_mean_ms')] for c in CASES]),
             '首次和增量的“总耗时”均为分别测得的 add 与 commit 之和，不是一个连续组合命令的单次计时。', '',
             table(['场景','MyVCS 首次中位数 ms','Git 首次中位数 ms','MyVCS 增量中位数 ms','Git 增量中位数 ms','MyVCS 增量平均耗时降低'],
                   [[NAMES[c],f"{val(c,'myvcs','initial_total_ms','median'):.2f}",f"{val(c,'git','initial_total_ms','median'):.2f}",
                     f"{val(c,'myvcs','update_total_mean_ms','median'):.2f}",f"{val(c,'git','update_total_mean_ms','median'):.2f}",
                     f"{100*(1-val(c,'myvcs','update_total_mean_ms')/val(c,'git','update_total_mean_ms')):.2f}%"] for c in CASES]),
             '观察：四组增量提交流程的平均耗时都低于 Git，A/B 组的差距更大；随机数据首次提交则是 Git 更快。此结论描述固定工作负载和当前虚拟机，不是一般性优越性证明。', '',
             '分阶段均值（ms）：', '',
             table(['场景','工具','首次 add','首次 commit','增量 add','增量 commit'],
                   [[NAMES[c],t]+[f'{val(c,t,m):.3f}' for m in ('initial_add_ms','initial_commit_ms','update_add_mean_ms','update_commit_mean_ms')]
                    for c in CASES for t in TOOLS]), '',
             '## 4. 存储收益', '',
             '对象目录内文件字节数包含对象和 Git pack 索引等辅助文件，不包含工作区文件、目录项或实际分配块。分母为完整数据版本数 × 文件字节数，20 MiB 组为 120 MiB，128 MiB 组为 384 MiB。', '',
             table(['场景','工具','GC 前均值 MiB','GC 后均值 MiB','GC 后均值 字节','总节省率'],
                   [[NAMES[c],t,f"{val(c,t,'object_bytes_before_gc')/1024**2:.4f}",
                     f"{val(c,t,'object_bytes_after_gc')/1024**2:.4f}",f"{val(c,t,'object_bytes_after_gc'):.2f}",
                     f"{val(c,t,'saving_vs_full_versions_pct'):.4f}%"] for c in CASES for t in TOOLS]),
             '观察：四组总节省率均超过 60%。随机组两工具几乎相同；A/B 组 Git 的最终对象大小略小。MyVCS 在 A/B 数据 GC 前较小，并不意味着 GC 后仍较小。不要将压缩高度重复输入产生的全部收益都归因于增量存储。', '',
             '## 5. GC、内存与验证', '',
             table(['场景','工具','GC 均值 ± SD ms','GC 中位数 ms','GC 最小–最大 ms'],
                   [[NAMES[c],t,ms(c,t,'gc_ms'),f"{val(c,t,'gc_ms','median'):.2f}",
                     f"{val(c,t,'gc_ms','min'):.2f}–{val(c,t,'gc_ms','max'):.2f}"] for c in CASES for t in TOOLS]),
             '四组 GC 中位数均为 MyVCS 更慢。但保留全部样本时，128 MiB A/B 的 Git GC 均值被一条 33.65 秒记录拉高，超过 MyVCS；不能写成“四组正式实验的 GC 平均耗时都是 MyVCS 更慢”。', '',
             table(['场景','工具','提交流程峰值均值 MiB','GC 峰值均值 MiB','验证峰值均值 MiB'],
                   [[NAMES[c],t]+[f'{val(c,t,m)/1024:.2f}' for m in
                     ('submission_peak_rss_KiB','gc_peak_rss_KiB','verify_peak_rss_KiB')]
                    for c in CASES for t in TOOLS]),
             '内存表是“每轮峰值的均值”，不是随时间平均的内存。提交流程峰值取该轮所有 add/commit 命令峰值的最大值。MyVCS 提交阶段约 3.4 MiB，但 128 MiB 随机数据 GC 约 847.5 MiB，因此不能说整个流程始终只需 3 MiB。', '',
             'MyVCS verify 与 Git fsck 的检查范围实现不同；这里只报告各自耗时和内存、确认成功，不据此证明二者验证性能等价。RSS 测量和数据恢复成功不足以证明不存在内存泄漏。', '',
             '## 6. 长尾、系统状态与顺序效应', '',
             '### 6.1 两条明显长尾记录（全部保留）', '',
             '- 第 5 轮 random20，MyVCS initial-commit：954.366 ms；同组首次 commit 中位数约 3.670 ms，次大值约 5.238 ms。该组首次 add+commit 均值 532.65 ms、中位数约 ' + f"{val('random20','myvcs','initial_total_ms','median'):.2f}" + ' ms。证据：[计时 JSON](../formal-main01/trial-05-random20/myvcs/initial-commit.json)。',
             '- 第 9 轮 ab128，Git gc：33,649.472 ms；同组中位数 1,128.535 ms。证据：[计时 JSON](../formal-main01/trial-09-ab128/git/gc.json)。',
             '- 两条记录 exit=0、后续正确性检查通过。前后快照的 pswpin、pswpout 和 pgmajfault 均未增加，不能把这两条直接归因于客户机换页；也不能因此排除宿主机调度、暂停或 I/O 等原因。现有日志无法确定根因。',
             '- 仅作敏感性说明：若排除各自这一个最大值，MyVCS 首次 commit 的其余 19 轮均值约 3.705 ms，Git A/B 128 GC 的其余 19 轮均值约 1,130.306 ms。这不是正式主结果，原始样本没有被删除。', '',
             '### 6.2 正式运行期间确实出现系统级换页活动', '',
             f"整段运行的 /proc/vmstat 中 pswpin 增加 {audit['swap_pages_in_delta_entire_run']} 页，pswpout 增加 {audit['swap_pages_out_delta_entire_run']} 页。测量前后快照里的 Swap 已用范围为 {min(swap_used)}–{max(swap_used)} KiB。该计数属于整个客户机，不能直接归因于某一个工具或命令。", '',
             '因此不能沿用预实验结束时 Swap=0 的描述，声称正式实验全程未换页。保留这一环境限制，不按是否换页选择性删除轮次。', '',
             '### 6.3 执行顺序分组', '',
             '下表每格为 10 轮的中位数（ms）。顺序交替减少固定先后偏差，但由于场景顺序也按轮次轮换，不能把组间差异单独解释成执行顺序的因果效应。', '',
             table(['场景','工具','先执行工具','首次总耗时中位数','增量总耗时中位数'],
                   [[NAMES[c],t,first]+[f"{st.median(float(r[m]) for r in rows if r['case']==c and r['tool']==t and r['first_tool']==first):.2f}"
                     for m in ('initial_total_ms','update_total_mean_ms')]
                    for c in CASES for t in TOOLS for first in TOOLS]),
             '其他按组 IQR 规则标记的较大/较小值保存在 audit.json 的 iqr_flags_no_exclusion 字段，仅用于定位记录，不作为剔除依据。', '',
             '## 7. 与论文的对照', '',
             '对照来源为本地论文第 6 章、正文第 39–41 页（PDF 第 51–53 页），表 6-1 至表 6-4。论文 CPU 为 Intel i5-12500H；本次为 AMD 宿主机上的 VMware 客户机。系统、内核和 Git 版本相符，但工具链、硬件、计时细节、页缓存条件和补丁内容不保证相同。论文中 MB/KB 未始终明确区分二进制与十进制，本次固定为 MiB/KiB。', '',
             table(['场景 / 指标','论文 MyVCS / Git','本次 MyVCS / Git（ms）'],[
                 ['20 随机 首次总耗时','474 / 460',f"{val('random20','myvcs','initial_total_ms'):.2f} / {val('random20','git','initial_total_ms'):.2f}"],
                 ['20 随机 增量总耗时','475 / 537',f"{val('random20','myvcs','update_total_mean_ms'):.2f} / {val('random20','git','update_total_mean_ms'):.2f}"],
                 ['20 A/B 首次总耗时','46.5 / 161.3',f"{val('ab20','myvcs','initial_total_ms'):.2f} / {val('ab20','git','initial_total_ms'):.2f}"],
                 ['20 A/B 增量总耗时','44.8 / 289.3',f"{val('ab20','myvcs','update_total_mean_ms'):.2f} / {val('ab20','git','update_total_mean_ms'):.2f}"],
                 ['128 随机 首次 add','3020 / 2742',f"{val('random128','myvcs','initial_add_ms'):.2f} / {val('random128','git','initial_add_ms'):.2f}"],
                 ['128 随机 增量 add','3052 / 2714',f"{val('random128','myvcs','update_add_mean_ms'):.2f} / {val('random128','git','update_add_mean_ms'):.2f}"],
                 ['128 随机 增量 commit','27 / 470',f"{val('random128','myvcs','update_commit_mean_ms'):.2f} / {val('random128','git','update_commit_mean_ms'):.2f}"],
                 ['128 A/B 首次 add','279.5 / 624.0',f"{val('ab128','myvcs','initial_add_ms'):.2f} / {val('ab128','git','initial_add_ms'):.2f}"],
                 ['128 A/B 增量 add','275.5 / 610.5',f"{val('ab128','myvcs','update_add_mean_ms'):.2f} / {val('ab128','git','update_add_mean_ms'):.2f}"],
                 ['128 A/B 增量 commit','26.3 / 896.3',f"{val('ab128','myvcs','update_commit_mean_ms'):.2f} / {val('ab128','git','update_commit_mean_ms'):.2f}"],
             ]),
             table(['场景','论文 GC 后大小 MyVCS / Git','本次 GC 后均值 MyVCS / Git（同列十进制单位）'],[
                 ['20 随机','约 21.01 / 21.02 MB',f"{val('random20','myvcs','object_bytes_after_gc')/1e6:.5f} / {val('random20','git','object_bytes_after_gc')/1e6:.5f} MB"],
                 ['20 A/B','约 25.0 / 26.9 KB',f"{val('ab20','myvcs','object_bytes_after_gc')/1000:.3f} / {val('ab20','git','object_bytes_after_gc')/1000:.3f} KB"],
                 ['128 随机','约 134.4 / 134.4 MB',f"{val('random128','myvcs','object_bytes_after_gc')/1e6:.5f} / {val('random128','git','object_bytes_after_gc')/1e6:.5f} MB"],
                 ['128 A/B','约 304 / 275 KB',f"{val('ab128','myvcs','object_bytes_after_gc')/1000:.3f} / {val('ab128','git','object_bytes_after_gc')/1000:.3f} KB"],
             ]),
             '随机数据的空间结果、128 MiB A/B 的空间量级、A/B 提交速度优势和提交阶段低内存的方向得到支持。20 MiB A/B 的对象大小没有复现到论文约 25/27 KB，本次约 52/50 KB；论文没有提供补丁具体内容等细节，本次明确采用随机覆盖，不能认定差异完全由某一因素造成。', '',
             '128 MiB 随机组的提交阶段峰值均值为 MyVCS ' + f"{val('random128','myvcs','submission_peak_rss_KiB')/1024:.2f}" + ' MiB、Git ' + f"{val('random128','git','submission_peak_rss_KiB')/1024:.2f}" + ' MiB；A/B 组为 ' + f"{val('ab128','myvcs','submission_peak_rss_KiB')/1024:.2f}" + ' / ' + f"{val('ab128','git','submission_peak_rss_KiB')/1024:.2f}" + ' MiB。与论文所报约 3.38/261 和 3.43/389 MB 的量级相近，但论文内存采样方法不完整，不能视为完全同口径。', '',
             '## 8. 可用于汇报的结论与后续工作', '',
             '1. 已完成 openEuler 环境下四种负载各 20 轮的性能复现，并用完整性检查及全部数据版本恢复核对保障结果正确性。',
             '2. MyVCS 在四组增量提交流程中的平均耗时均低于 Git，A/B 数据优势更明显；随机数据首次提交则不占优。',
             '3. MyVCS 提交阶段峰值内存约 3.4 MiB；GC 内存明显更高，应按阶段报告。四组空间节省率均超过 60%，GC 后存储大小与 Git 接近。',
             '4. 保留两条长尾并同时报告均值、中位数、标准差；系统级换页记录和虚拟化影响属于实验限制。当前结果不支持“全面优于 Git”“全程约 3 MiB”或“已证明无内存泄漏”。',
             '5. 若“响应时间 <2 s”仅指 commit，本批 MyVCS commit 样本均低于 2 s；若包含 add+commit，128 MiB 随机数据约 3 s，不满足该口径。不能混用定义。',
             '6. 下一阶段可优先修复功能复现发现的人工冲突提交丢失第二父节点、rm 后 status 重复显示删除的问题；性能优化则可针对 GC 的时间和峰值内存。修改后用同数据、同脚本重新建立对照，不覆盖基线。',
             '可选诊断：若需解释长尾，单独开展记录 CPU 时间、宿主机活动和 I/O 状态的诊断实验；不必为了消除不理想样本而重跑或替换本批正式结果。', '',
             '## 附录：160 条原始轮次结果摘录', '',
             '下表按原始 CSV 顺序展示；为阅读仅舍入显示值，完整精度与其他字段保留在 results.csv。增量列是每轮内部均值。', '',
             table(['场景','轮次','工具','首次总 ms','增量总 ms','GC ms','GC 后 字节'],
                   [[r['case'],r['round'],r['tool'],f"{float(r['initial_total_ms']):.3f}",
                     f"{float(r['update_total_mean_ms']):.3f}",f"{float(r['gc_ms']):.3f}",r['object_bytes_after_gc']] for r in rows])]
    report = OUT / 'MyVCS_正式性能复现报告.md'
    report.write_text('\n'.join(lines), encoding='utf-8')
    print(json.dumps({'report': str(report), 'audit': audit}, ensure_ascii=False, indent=2))


if __name__ == '__main__':
    main()
