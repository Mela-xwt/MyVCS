# P1 故障处理

## unfinished worktree operation

先停止其他进程对该工作区的操作，保留整个仓库及工作区副本。若进程中断后没有另外编辑文件，可选一个方向：

```bash
myvcs recover-worktree          # 完成中断操作
# 或
myvcs recover-worktree --abort  # 恢复操作前文件与索引
```

这不是 `merge --abort`，也不是引用恢复 `recover`。一旦启动回滚，journal 记录回滚方向，后续无参数恢复也继续回滚。不要直接删除 journal 或 pin 绕过检查。debug 测试的 `.myvcs/test-work-crash` 标记应先移除；release 没有该注入入口。

若报 `recovery refuses new edits`，先把报错路径的事后编辑复制到仓库外；不要为通过校验随意覆盖其他路径。HEAD 或索引不匹配时从备份核对操作前状态。自动工具不能替用户决定丢弃哪一份新编辑。

中断后的 pop 可能已恢复工作区但仍保留 stash，先检查文件、索引和 list，再决定是否 drop。journal 清理后、pin 删除前的极小窗口可能留下 `refs/pins/work-*`；它只多占空间。当前没有自动孤立 pin 清理命令，保留或在有完整备份、确认无活动 journal 的维护窗口处理，不要直接猜测删除对象。

## stash 冲突

有本地修改或索引条目时，apply/pop 拒绝。先另存/提交当前修改，或使用新的工作目录恢复；不同分支同路径冲突采取文件级拒绝，需要人工决定组合方式。快照不会因失败 pop 被删除。

## drop 后空间未下降

格式 2 的 reflog 仍可保护快照；此外共享 Blob、基础链和其他引用也可能保留对象。`stats` 比较总对象与可达对象，按团队日志保留策略过期 reflog 后再 GC。不要仅为了让计数下降清空恢复历史。

## 旧工具、平台与数据损坏

在工作区有未完成 journal 时，禁止使用阶段 3 旧二进制修改/清理该工作区。正常无 journal 状态下，已实测新旧读写格式 1/2 以及旧 GC 保护新 stash。

缺失基础、损坏 delta、哈希不符或资源超限会明确失败，不能通过删基础对象来缩链。先备份，从完整副本恢复缺失对象或恢复整个仓库，再 verify。Windows 文件替换、目录持久化和大小写行为与 Linux 不完全一致；本轮未进行原生 Windows/macOS 验收。
