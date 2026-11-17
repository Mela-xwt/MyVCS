# MyVCS 阶段 3：日常团队协作

本轮以 SSH 为传输通道，不实现 HTTPS。源码入口仍是工作区根目录的 `src/` 和 `tests/`；支持团队身份映射、受控 lease 和多引用事务。源码快照 `candidate-source.tar.gz` 包含 Rust 源码、测试及受限 SSH 入口。

## 行级合并与 abort

```bash
myvcs merge feature
# 冲突时编辑文件，然后逐项 add 或 rm，最后 commit
myvcs add file.txt
myvcs commit -m 'resolve merge'

# 放弃本次合并，恢复开始前的文件与暂存区
myvcs merge --abort
```

按基础文件行坐标组合双方不相交修改，相同修改只保留一次；重叠修改保留冲突标记，手动解决后的提交包含两个父提交。没有冲突的内容保留原始换行字节，包括 CRLF 和末行无换行。二进制或超过文本计算限额的冲突保留我方内容，并记录结构化冲突；我方删除时可用 `rm` 明确确认删除。

文本自动合并限每侧 1 MiB、20,000 行，单次 LCS 表不超过 4,000,000 单元；超过后转为明确的文件冲突。该限制约束计算规模，并不承诺任意大文本都能自动合并。不支持自动识别重命名；文件/目录或大小写转换保守拒绝。有多个最佳共同祖先的交叉合并明确拒绝，不任意选择一个基础继续执行。

合并前检查暂存内容、跟踪文件修改和未跟踪覆盖。开始修改工作区前持久化恢复状态；完整写入文件、索引与冲突状态后才标记可提交。准备或 abort 中断后，commit 会拒绝不完整状态，应再次 `merge --abort`。abort 只恢复记录的合并路径，保留无关未跟踪文件；开始后的 HEAD 已变化时拒绝回退。工作区文件恢复不是跨所有文件的一次原子操作，重试 abort 是该场景的恢复入口。

本项完成了独立复核及两轮边界修复，见 `task-1-report.md`、`task-1-review.md` 和 `task-1-review-team-final.log`。

## 日常命令

```bash
myvcs config user.name 'Alice'
myvcs config user.email 'alice@example.test'
myvcs config user.name

# 本地 topic 对应远端 review/topic；成功推送后记录 upstream
myvcs push -u origin topic:review/topic
myvcs branch --set-upstream-to origin/review/topic topic
myvcs branch --unset-upstream topic

# 设置 upstream 后可省略 remote 和分支；显式参数优先
myvcs push
myvcs fetch
myvcs pull

myvcs tag v1 HEAD
myvcs tag
myvcs branch release
myvcs push --atomic origin main release refs/tags/v1
myvcs fetch --tags
myvcs fetch --prune
myvcs tag -d v1
myvcs log --graph --all

# 明确提供之前确认过的完整 OID；服务端还必须授予 force 权限
myvcs push --force-with-lease refs/heads/topic/review:FULL_EXPECTED_OID origin topic/review
# 删除远端引用需要独立 delete 权限；默认 HEAD 分支拒绝删除
myvcs push --atomic origin :topic/obsolete
```

身份配置存于仓库，提交作者使用 `Name <email>`；没有配置时兼容原有系统用户名行为。作者字段不参与权限判定。clone 会记录 origin upstream；`push -u` 只在成功后更新配置。upstream 的 remote 和远端分支分别保存，允许本地与远端分支不同名。

标签只指向提交，是不可覆盖的轻量标签；不支持附注标签或签名。DAG 日志按子提交先于父提交的顺序输出，并显示父 OID 和引用标记；普通日志也遍历两个父分支，`--all` 加入所有分支、远端跟踪引用、标签和 detached HEAD，不包含只存在于 reflog 的历史。

## 升级与部署约束

采用显式的仓库元数据版本升级，原始对象与 OID 保持不变。升级必须安排维护窗口，停止访问该仓库的所有旧版进程；格式标记只能阻止旧版重新打开，无法使已打开仓库的旧进程自动采用新规则。

```bash
# 新建支持引用事务与恢复日志的仓库
myvcs init --bare --team /srv/myvcs/team

# 已有仓库：先停止所有访问该仓库的旧进程并备份，再确认维护窗口
myvcs --repo /srv/myvcs/team upgrade --quiescent

myvcs reflog
myvcs reflog --reference refs/heads/main
myvcs recover FULL_COMMIT_OID recovered-branch
myvcs checkout recovered-branch

# 显式保留最近 N 条日志；先导出需要长期保留的记录
myvcs reflog expire --keep 1000
```

普通 `init` 保留格式 1 兼容模式；`init --team` 仅用于新仓库。格式 2 使用唯一权威文件 `refs-state.json`，将所有引用、HEAD 和本次 reflog 一起发布；旧版松散引用备份不再参与读取。旧程序直接打开格式 2 会拒绝；旧客户端可以通过新版服务访问，受新版服务权限约束。升级先写完整状态，再提交格式标记；提交标记前中断可重试，重试会读取当时的旧格式引用。

每次事务接受 1～256 项引用变更，全部使用期望旧 OID 检查；一项不符即不发布。reflog 保存旧、新端点，GC 将仍保留的端点及其历史视为根，因此 reset 或删除分支后可以恢复。`recover` 只接受仍在日志中的完整 OID，并创建新分支，不改变工作区。没有自动日志过期；显式过期后，失去其他根保护的对象可能在下次 GC 中被删除。升级前已经丢失的历史无法补回。

引用状态默认上限 16 MiB；到达上限时变更明确失败，需显式导出、过期日志。日志序号不会因过期而复用。原子发布覆盖引用与成功更新日志，不覆盖整个工作区。发布后连接断开或目录同步出错时，客户端应重新读取远端引用确认结果，不能将错误返回等同于绝对未提交。

## 远程接口与版本边界

控制协议版本和 pack 编码版本是两个独立概念。扩展引用操作使用控制协议 2；完整对象、包内 delta 和已确认接收端基础的 pack 模式仍沿用阶段 2，不改逻辑对象或 OID。

| 操作 | 版本及范围 |
| --- | --- |
| 普通单分支 clone / push / fetch / pull | 保留控制协议 1 与格式 1 兼容路径；新版服务的权限检查同样适用 |
| 多引用 push、显式 lease、远端标签与删除 | 扩展控制协议；远端须为格式 2；本地仅发送时无需为此升级 |
| `fetch --tags` / `fetch --prune` | 本地须为格式 2，以一次事务发布本次跟踪引用、标签和清理结果 |
| reflog / recover | 格式 2；只恢复仍保留日志中的提交 |
| HTTPS / HTTP、Git wire / pack 互通 | 不支持 |

prune 只清理指定 remote 的已消失分支跟踪引用，依据一次完整且成功的远端广告；不删除本地分支、其他 remote 或标签。即使选定 upstream 已删除，也可清理它的旧跟踪引用。`--tags` 显式接收标签及其历史；已有本地同名标签指向不同提交时整批拒绝，不悄悄覆盖。标签不会随 prune 自动删除。

新功能遇到旧服务会明确报版本不支持，不静默退化为多次单引用写入。可设置 `MYVCS_PACK_MODE=full` 使用完整对象编码，但这不会将扩展引用事务变成旧控制协议。阶段 2 的资源限制、基础固定和逻辑对象校验继续适用，详见 [阶段 2 协议](../collaboration-stage2/PROTOCOL.md)。

服务器权限来自管理员绑定到 SSH 密钥的固定身份，提交作者字段仅用于显示。受限密钥使用固定服务入口，解析请求中的仓库标识，不将请求作为 shell 命令执行；生产部署还需限制 shell、转发和客户端环境变量。OpenSSH 的 forced command、`restrict` 与 `SSH_ORIGINAL_COMMAND` 语义见 [sshd 手册](https://man.openbsd.org/sshd.8)，环境变量接收规则见 [sshd_config 手册](https://man.openbsd.org/sshd_config.5)。

本轮权限验收使用独立高端口、回环监听的临时 SSH 服务和新测试密钥，不改系统 SSH 配置或现有 authorized_keys。openEuler 对此测试服务的 `UsePAM no` 发出平台警告；该配置仅用于隔离验收，不作为生产 PAM 配置建议。

## SSH 身份与权限配置

管理员在裸仓库目录放置 `policy.json`；非裸仓库则放在 `.myvcs/policy.json`。存在策略时，缺少可信身份、未列出的身份、无权读取或不匹配写入规则都拒绝。未配置策略时保留原有单账户普通协作模式，但不会因此开放 force 或删除权限。受策略管理的仓库须先升级到格式 2。

```json
{
  "version": 1,
  "users": {
    "alice": {
      "read": true,
      "write": ["refs/heads/*", "refs/tags/*"],
      "force": ["refs/heads/topic/*"],
      "delete": ["refs/heads/topic/*"]
    },
    "reader": {"read": true, "write": [], "force": [], "delete": []}
  }
}
```

规则支持完整引用名或末尾 `/*` 的命名空间前缀，不是通用 glob。force 和 delete 与 write 分开授予；tag 即使有 force 也不允许覆盖。read 是仓库级权限，不能用分支写权限隐藏该仓库其他历史。

将入口脚本和二进制安装到管理员控制的绝对路径，为每把团队密钥绑定固定身份。例如 authorized_keys 中的一行：

```text
restrict,command="/usr/bin/python3 -I /opt/myvcs/ssh-gate.py --root /srv/myvcs --program /opt/myvcs/myvcs --principal alice" ssh-ed25519 PUBLIC_KEY_HERE
```

客户端仍使用 `myvcs clone ssh://ACCOUNT@HOST/team`。入口只接受 `myvcs serve --repository ID`，检查仓库根目录、拒绝路径逃逸和符号链接，并重新构造服务环境；客户端不能选择 principal、服务程序或 shell 命令。不要给受限团队密钥额外的同账户 shell/任意文件写入权限，也不要通过 AcceptEnv 接收控制服务身份或加载器的变量。

`file://` 保留为本机子进程传输，信任服务进程环境，不提供 SSH 密钥认证。它适用于本机测试或已拥有仓库文件权限的调用方，不能代替上述远程身份边界。

格式 2 成功推送的 actor、引用旧/新 OID 和结果与引用事务一起保留在 reflog；读取和拒绝等尝试记录另存 `audit-attempts.json`。`myvcs audit` 查看，`myvcs audit expire --keep N` 只显式过期尝试记录。尝试日志最多 10,000 条，并受元数据大小限制；容量不足时明确失败，不静默漏记。成功记录随显式 reflog 过期而删除；长期审计应在过期前导出。未升级、未配置策略的格式 1 兼容写入没有原子成功审计，需升级后使用该保证。审计不防范拥有仓库文件写权限的 OS 用户或管理员篡改。

`audit` 默认显示最近 100 条，`--limit N` 最大 10,000，这是查看窗口，不是全量归档接口。完整归档应在停止写入的维护窗口备份 `audit-attempts.json`，格式 2 还需备份 `refs-state.json`；保留原始记录后再分别执行过期命令。格式 1 也支持查看和过期其读取/失败记录。

故障恢复与维护步骤见 [OPERATIONS.md](OPERATIONS.md)。

## 验收与跨平台范围

WSL Ubuntu 和服务器 openEuler 的最终完整 Rust 测试均为 **131 passed、0 failed**：40 单元、9 本地接口、13 接收事务、12 原有远程协作、14 原有回归、16 行级合并、12 引用恢复、15 团队协作测试。格式检查、真实旧版二进制兼容矩阵、SSH 入口独立检查均通过。

| 完成标准 | 证据 |
| --- | --- |
| 多人并发 | 独立进程 CAS、协议会话竞争；真实 SSH 两客户端同时更新 main 和 race，只有一方成功，两引用保持一致 |
| 复杂分叉与合并 | 行级不相交修改自动合并、冲突解决保留双父、完整 DAG 与 detached 根；多个最佳基础保守拒绝 |
| 权限 | alice / bob / reader / outsider 使用独立测试密钥；伪造作者无效；v1/v2 接收确认后撤权仍拒绝发布 |
| 日常维护 | upstream、完整广告 prune、标签冲突拒绝、只被标签引用的历史经 fetch/GC 保留 |
| 故障恢复 | 原子状态与升级标记前后进程终止、对象安装边界终止；重开后全部旧或全部新状态；GC 后 reflog 恢复 |
| 真实 SSH 恢复 | 改写 main 并删除旧历史最后一个标签后，服务端 GC、recover 及 verify 成功 |
| 兼容 | 真正阶段 2 二进制与阶段 3 双向普通协作；旧客户端仍受新策略约束；旧程序拒绝直接打开格式 2，旧服务拒绝扩展控制协议 |
| 资源上限 | 合并计算、引用状态、日志、审计、快照和 pack 均有界；超限明确失败，完整编码回退仍保留 |

原生 Windows Rust 运行未验证，Windows 主机的客户端验证使用 WSL。实际 SSH 多身份验收在 openEuler 的独立回环高端口服务上完成；本轮没有新增跨物理主机 SSH 验收。文件持久化使用现有文件 fsync、原子 rename 和 Linux 目录 fsync；Windows 目录同步仍是空操作。故障注入是实际进程终止，不是断电或文件系统损坏实验。不能据此承诺原生 Windows 与 Linux 相同的崩溃持久性。

最终服务器隔离部署目录：`/home/oslab/os-lab/myvcs-stage3.cmjI2y`，二进制为 `target/release/myvcs`。验收结束后测试 SSH 服务退出并清理测试私钥；二进制与测试仓库保留用于复核。没有覆盖阶段 1/2 部署，也没有修改系统 SSH 配置或现有 authorized_keys。

复现命令：

```bash
cargo test --locked --offline
cargo fmt --check
python3 collaboration-stage3/gate_acceptance.py
python3 collaboration-stage3/compatibility.py --old /path/to/stage2/myvcs --new /path/to/stage3/myvcs --quiescent
```

Windows 主机可运行 `python collaboration-stage3/remote_validate.py` 新建服务器隔离构建目录，再用 `--acceptance` 执行真实 SSH 验收、`--inspect` 记录二进制哈希。验证器交互读取密码，不保存凭据。`--offline` 要求依赖已缓存。

## 审阅与复现文件

- `baseline-source.tar.gz`：阶段 2 基线。
- `task-1-source.tar.gz`、`task-1.diff`：合并功能经复核后的快照与差异。
- `task-2-source.tar.gz`、`task-2.diff`、`task-3.diff`：引用基础和团队功能的复核输入。
- `final-fix-tests.log`：WSL 最终 15 项专项及 131 项完整测试；`server-build-tests.log`：服务器最终完整测试。
- `tests-local.log`：最终审查修复前的 130 项完整测试，保留原始记录。
- `server-ssh-acceptance.log`、`server-ssh-exit-code.txt`：真实 SSH 验收及退出码。
- `compatibility-final.log`、`gate-acceptance.log`：旧版二进制和受限入口检查。
- `server-artifacts.log`：服务器源码包与二进制 SHA-256。
- `final-review.md`、`final-fix-review.md`：整体审查及唯一审计兼容性修复的复核结论。
- `candidate-source.tar.gz`、`final.diff`、`source-manifest.json`、`SHA256SUMS.txt`：最终源码、相对阶段 2 的差异及完整性记录。
- `progress.md`：实现与复核进度。
- `artifacts.py`：在没有 Git 元数据的本工作区生成源码快照、差异与哈希。
- `remote_validate.py`：新建服务器隔离目录，构建、测试及执行 SSH 验收。
- `isolated_sshd.py`：测试专用的 SSH 密钥身份隔离环境。
- `compatibility.py`：使用真正的阶段 2 和阶段 3 二进制验证兼容矩阵。
