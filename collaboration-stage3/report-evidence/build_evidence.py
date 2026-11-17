"""Render verbatim historical log excerpts; does not run or invent test output."""
from pathlib import Path
import html
import hashlib
import json

out = Path(__file__).resolve().parent
base = out.parent
log = base / 'server-ssh-acceptance.log'
lines = log.read_text(encoding='utf-8').splitlines()
cases = [
 ('01-sync', 'A 提交后 push，B clone 获取同一提交', [(14,24)], 'A push 与 B fetch 输出相同完整 OID，证明本次克隆指向同一提交。'),
 ('02-merge', '过期 push 被拒绝，pull 生成双父合并', [(54,64),(76,78)], 'B 的过期推送被拒绝；pull 后的新提交同时记录 A、B 两个父 OID。'),
 ('03-policy', 'SSH 身份隔离：伪造作者不能获得写权限', [(30,34),(40,43),(115,117)], '未知身份无法读取；reader 修改作者为 alice 后仍不能写入；Bob 的越权原子推送被拒绝。'),
 ('04-lease-race', '显式 lease 与两个引用的并发推送', [(130,134),(156,163)], '过期 lease 拒绝、正确 lease 成功；两个独立 SSH 推送一成一败。脚本另断言两个远端引用均等于胜方提交。'),
 ('05-recovery', 'GC 后保留 reflog 历史并恢复分支', [(164,172),(220,223)], '删除旧历史的最后一个标签后执行 GC，通过 recover 创建 rescued 分支；完整性验证通过。'),
]
manifest = {'source': str(log), 'sha256': hashlib.sha256(log.read_bytes()).hexdigest(), 'figures': []}
css = '''body{margin:0;background:#eef2f6;color:#14253a;font-family:"Microsoft YaHei",Arial,sans-serif}main{max-width:1200px;margin:auto;padding:26px}h1{font-size:26px;margin:8px 0 14px}.badge{color:#255f88;font-size:13px;letter-spacing:1px}p{font-size:16px;line-height:1.65}pre{background:#122238;color:#e6edf6;padding:20px;border-radius:10px;font:13px/1.6 Consolas,monospace;white-space:pre-wrap;overflow-wrap:anywhere}footer{font-size:12px;color:#576779;line-height:1.7}nav{margin-top:20px}a{color:#1e63a0}'''
links=[]
for slug,title,ranges,caption in cases:
    blocks=[]
    for start,end in ranges:
        blocks.append('\n'.join(lines[start-1:end]))
    excerpt='\n\n[中间日志省略]\n\n'.join(blocks)
    (out/(slug+'.txt')).write_text(excerpt+'\n',encoding='utf-8')
    source='server-ssh-acceptance.log · 行 '+', '.join(f'{a}–{b}' for a,b in ranges)
    page=f'<!doctype html><html lang="zh-CN"><meta charset="utf-8"><title>{title}</title><style>{css}</style><main><div class="badge">MYVCS / 多人协作验证 / 真实日志摘录</div><h1>{html.escape(title)}</h1><p>{html.escape(caption)}</p><pre>{html.escape(excerpt)}</pre><footer>来源：{source}<br>本图为已有真实验收日志的浏览器截图，不是本轮实时终端执行截图。OID 与临时端口随重新运行变化。<br>环境：openEuler；独立 SSH 密钥；回环高端口。完整日志 SHA-256：{manifest["sha256"]}</footer><nav><a href="index.html">全部证据</a></nav></main></html>'
    (out/(slug+'.html')).write_text(page,encoding='utf-8')
    links.append(f'<li><a href="{slug}.html">{title}</a> · <a href="{slug}.png">PNG 截图</a></li>')
    manifest['figures'].append({'file':slug,'source_lines':ranges,'caption':caption})
(out/'index.html').write_text('<!doctype html><meta charset="utf-8"><title>MyVCS 报告证据</title><style>'+css+'</style><main><div class="badge">REPORT EVIDENCE</div><h1>MyVCS 多人协作验证材料</h1><p>以下页面逐字摘录已有验收日志，省略处显式标注。配套说明见《多人协作验证与截图指南.md》。</p><ol>'+''.join(links)+'</ol><p>这些是日志证据页截图，不是新运行的终端截图。需要实时终端截图时，请按指南重新执行。</p></main>',encoding='utf-8')
(out/'manifest.json').write_text(json.dumps(manifest,ensure_ascii=False,indent=2),encoding='utf-8')
print('Created',len(cases),'verbatim log evidence pages')
