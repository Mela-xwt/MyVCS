# F01：根目录忽略规则

```gitignore
# .myvcsignore 位于仓库根目录
target/
*.log
cache/
!important.log
!cache/keep.txt
```

```bash
myvcs status
myvcs add .
myvcs check-ignore target/debug/app important.log
```

规则由 `status`、递归 `add`、`stash push --include-untracked` 共用。已跟踪/已暂存文件不受忽略规则过滤；显式 `add ignored.log` 会报错，不提供强制添加选项。

## 匹配语义

- UTF-8；支持 LF/CRLF、空行、以 `#` 开头的整行注释；不自动去除空格。
- `*` 匹配组件内任意长度，`?` 匹配组件内单字符，`**` 可以跨 `/`。这是一套简化 glob：例如 `**/a` 中的 `/` 仍为字面分隔符，不隐含匹配根目录 `a`。
- 无 `/` 的模式匹配任意层路径组件；以 `/` 开头或含 `/` 的模式相对仓库根匹配。末尾 `/` 仅匹配目录及其后代。
- 后匹配的规则覆盖前规则；`!` 重新纳入。允许直接以 `!cache/keep.txt` 纳入已忽略目录的子文件。
- 仅加载根目录规则，不支持嵌套规则、字符类、反斜线转义；含 `[`、`]`、`\` 的规则明确报错。不是完整 Git ignore 兼容实现。
- 上限 1024 条有效规则，每条 512 字节；规则文件沿用元数据读取上限。内部 `.myvcs`/`.git` 与不安全路径不可通过否定规则纳入；符号链接沿用保守拒绝策略。

`check-ignore` 输出 `ignored/included`、`.myvcsignore:行号`、规则、路径，已跟踪路径输出 `tracked`。未命中输出 `included`。有效查询返回 0，无论是否忽略；无效路径或规则返回非零。对不存在路径，目录类型仅能按实际文件系统判定，建议查询具体子文件。

原先硬编码跳过的 `target/` 改由规则显式控制，建议在开发仓库写入该规则。为支持否定规则，扫描仍遍历被忽略目录，不声称扫描性能提升。

验证：`cargo test --test p1_features ignore_`。覆盖规则一致、否定、目录、根锚定、Unicode、已跟踪文件及越界路径。
