# F03：Stash 保存与恢复工作现场

```bash
myvcs stash push -m 'work in progress'
myvcs stash push --include-untracked -m 'include notes'
myvcs stash list
myvcs stash show 0
myvcs stash apply 0
myvcs stash pop 0
myvcs stash drop 0
```

默认保存已跟踪文件的工作区和索引，包括新增/修改/删除；`--include-untracked` 加入非忽略的未跟踪文件。成功发布持久化快照后，相关工作文件回到 HEAD，索引清空。未包含的未跟踪文件和忽略文件保留。未出生分支可用。

list 的列为位置、稳定 ID、短 OID、JSON 编码消息。数字 0 表示当前最新条目，push/drop 后数字位置会变化。需要跨操作稳定定位时，保存第二列 ID 并传给 show/apply/pop/drop。ID 含时间与唯一后缀；时间排序依赖本机时钟，不保证时钟倒退时的绝对创建顺序。

apply 保留快照，pop 完成恢复后才移除引用，drop 只移除引用。恢复会保留同一基础上的原始索引，使“已暂存 A、工作区 B”分别还原。

## 冲突和恢复边界

apply/pop 要求索引干净且当前跟踪文件与 HEAD 一致。允许无关未跟踪文件存在，但任何目标路径碰撞会拒绝。不同 HEAD 使用基础/当前/保存版本的保守**文件级三方组合**：独立文件修改可组合，同一路径不同内容拒绝，不生成文本冲突标记，不自动重命名。冲突拒绝发生在写工作文件前，快照保留；可先保存当前工作、切回基础或另建工作目录后处理。

未完成 merge 时不允许 push/apply/pop。快照对象损坏、缺失、路径不安全、超限均失败，不清理原现场。push 在发布快照后若后续准备失败，可能留下一个安全可用的 stash；以 list 确认，不应反复盲目 push。

## GC 与格式兼容

快照以 `refs/pins/stash/<ID>` 作为根。顶层提交记录工作区 tree 与结构化消息，父提交包含基础、暂存内容和未跟踪内容的辅助快照，因此既有 GC 与 verify 能遍历所有对象。普通 SSH push/fetch 只广告分支/标签，stash 不自动同步到远端；不能依赖远端仓库备份未推送的 stash。

格式 1 可用；格式 2 下 pin 变更保留 reflog，因此 drop 后 GC 未必马上释放对象。只有全部保护引用及保留日志都不再引用时才可回收。`reflog expire --keep 0` 会删除整个仓库的恢复记录，不是 stash 专用清理命令，应根据团队保留策略使用。

工作区变更前创建独立 `refs/pins/work-*` 保护两侧的文件与索引对象，再写 `work-journal.json`。进程中断后通过 `recover-worktree` 或 `recover-worktree --abort` 处理。恢复遇到事后新编辑会拒绝覆盖。pop 中断后可能已经恢复文件但仍保留 stash，这是保留恢复机会的预期行为。

已实测阶段 3 旧程序 GC 在格式 1、2 下都能保护 P1 stash。旧程序不知道 journal，因此未完成恢复时只能使用新版恢复工具；禁止混用旧客户端修改同一工作区。

验证：`cargo test --test p1_features stash`。含 GC 后还原、跨 HEAD、未出生分支、冲突不覆盖、稳定 ID、drop/reflog、损坏对象、三个中断点及索引新增但工作文件删除的边界。
