# F02：单文件恢复与暂存差异

```bash
myvcs restore file.txt
myvcs restore --source COMMIT_OR_REF file.txt
myvcs restore --staged file.txt
myvcs restore --staged --source COMMIT_OR_REF file.txt
myvcs diff --staged
myvcs diff --staged file.txt
```

| 操作 | 工作区 | 索引 | HEAD |
| --- | --- | --- | --- |
| restore | 指定文件变为来源内容；来源无该路径时删除 | 保持原样 | 不变 |
| restore --staged | 不变 | 指定文件变为来源状态相对当前 HEAD 的差量 | 不变 |
| diff --staged | 只读 | 比较 HEAD 与索引表示的下一次提交 | 不变 |

默认来源是 **HEAD**，不是 Git restore 默认的索引。单路径接口不递归恢复目录；其他路径不改变。明确执行普通 restore 会丢弃该已跟踪文件的未暂存修改。未跟踪、rm 后重新创建的路径覆盖、符号链接、越界、文件/目录冲突和未完成 merge 会拒绝。`--staged` 只改索引，不覆盖工作文件；索引超过元数据预算时写入前失败。

`diff --staged` 正确区分暂存内容和随后又编辑的工作区内容，支持新增、修改、删除与既有二进制提示格式。

工作文件变更共用持久化 journal。失败后可能存在可识别的中间状态；按 [故障恢复](../troubleshooting.md) 完成或回滚。逐文件原子替换并不等于工作区所有文件全局原子替换。只改索引的 restore 使用既有原子索引写入。

验证：`cargo test --test p1_features restore`，以及完整 P1 测试中的 staged diff 用例。涵盖 HEAD/其他索引不变、旧版本字节、安全拒绝、进程中断、继续/回滚和资源超限。
