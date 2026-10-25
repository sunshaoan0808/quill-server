# CHECKPOINT — Phase 6.5 OPDS 集成（2026-07-31）

## 状态：✅ 全链路验证通过（浏览/搜索/下载/入库/阅读）

## 项目根
/root/workspaces/museai-web-android-brainstorm/museai-server/web/bookshelf
服务: http://localhost:8899/web/bookshelf/ (serve.py, ThreadingHTTPServer 保活)

## 本次修复（4 处代码改动）
1. **serve.py**: 新增 /opds-proxy 端点（urllib 抓取 + 跨域头）+ 从 HTTPServer 改 ThreadingHTTPServer（并发不卡）
   - 验证: 本地 feed 200、Gutenberg 200、/opds-proxy 路径 URL 已校验拒绝
2. **opds-manager.js pickDownload/findSearchTemplate/groups**: `getLink(rel)` 返回数组，改成 `.find()` 取首个而非当字符串用（原 bug: rel 数组转字符串后 includes 误判）
3. **opds-manager.js 搜索处**: 搜索 URL 相对路径未绝对化 → proxy 拒绝 400。修复: `new URL(tpl, state.url)` 绝对化（带中文参数 URL 编码）
4. **test feed 纯导航化**: test-opds12.xml 根目录只留 navigation 子项

## 测试文件（web/bookshelf/ 下）
- test-opds12.xml: 根目录纯导航 feed
- sub.xml: 2 个出版项（带 download）
- opensearch.xml: 搜索模板（test-opds12-search.xml?q={searchTerms}）
- test-opds12-search.xml: 搜索 feed（1 条结果: 三体/刘慈欣）
- test-opds-book.epub: 最小合法 EPUB（mimetype STORED + 第一位置 ✅, OPF/spine 规范, 内容"OPDS 测试书通过 OPDS 目录下载的测试 EPUB。"）

## 验证证据（Playwright headless）
- 浏览: root feed 27 导航项卡片
- 搜索: title "搜索: 三体", 1 卡片, 作者刘慈欣
- 下载→入库: .fbm-row "test-opds-book.epub EPUB 1KB 阅读 删除", 面板 "📚 本地书籍 (1)", IDB LocalBooks/books store
- 阅读: view.renderer.getContents()[0].doc.body.textContent 含 EPUB 内容 ✅

## 关键排障教训（防再踩）
1. **foliate-view 是 closed shadow root**（attachShadow closed）! view.innerHTML / querySelectorAll 全部看不到内容。
   必须用 `view.renderer`（public 字段）→ `renderer.getContents()` 拿 doc。查 iframes 用 renderer.querySelectorAll。
   - 之前误判"渲染失败"全是探测方法错误。openFoliateBook 流程本身 OK: makeBook → view.open → view.init。
2. **sections 内容是懒加载**: makeBook 后 sections[].len=0 正常; `sec.load(book)` 返回 blob URL。
3. **Playwright 每次 launch 全新 context → IDB 空**（书架书不持久）, 测试前先导入。
4. **pageerror "Cannot read properties of null (reading 'documentElement')"** 偶发（iframe 时序），不影响渲染，可忽略。
5. 探针注意: page.screenshot 不能放 evaluate 内。

## 遗留
- OPDS 配置 UI 目前是硬编码 URL（opds-manager.js 顶部）; 未做 URL 配置持久化（6.5 子项 "OPDS 目录输入（URL 配置）" 实际用固定 URL，可后续加设置）
- 真实网络书库（Gutenberg 等）proxy 已通，未做端到端 UI 测试

## 探针脚本（web/bookshelf/probe-*.js 已留档）
probe-search.js / probe-download2.js / probe-shadow.js（最终验证用）
运行: NODE_PATH=/root/scripts/node_modules xvfb-run -a node probe-xxx.js

## 遗留处理（2026-07-31 追加）
1. **默认源种子**: opds-manager.js getSources 为空时预置 2 源（localStorage 可覆盖/删除）
   - 本地测试书库: {origin}/web/bookshelf/test-opds12.xml
   - Project Gutenberg: https://www.gutenberg.org/ebooks.opds/ （**注意 .opds 不是 /opds/**）
2. **Gutenberg 端到端验证 ✅**（Playwright）:
   - 根: 📁 Popular/Latest/Random 导航
   - Popular: 25 本书渲染为导航项（**Gutenberg 列表 entry 无 acquisition link，仅 subsection → 按 foliate 规范判为 navigation，是上游设计非 bug**）
   - 点书 → 详情 OPDS 页 → 卡片 + "⬇ 下载 epub+zip" ✅（Moby Dick 实测）
   - 搜索: Gutenberg 根 feed **无 OpenSearch 声明** → 搜索框隐藏（标准行为）
3. **修正之前误判**: "URL 硬编码" 实际不存在 —— opds-manager.js 已有完整源管理 UI（添加/打开/删除 + LS_KEY 持久化）
4. **代理 UA 注意**: serve.py /opds-proxy 用 Mozilla UA → Gutenberg 返回 OPDS XML；curl 默认 UA 拿 HTML。调试 Gutenberg 时用 `curl -A "Mozilla/5.0"` 或走 proxy
