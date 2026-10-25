# bookshelf 遗留处理 checkpoint (2026-07-31)

工作目录: /root/workspaces/museai-web-android-brainstorm/museai-server/web/bookshelf
书架: http://localhost:8899 (独立书架, museai-server)
阅读器: foliate-reader/ (1.2 模块拆分 + 1.5 CSS 抽取后)

## 已修复
1. **页面空白恢复** ✅ — 根因: 我 patch 吞掉 ui.js popup.innerHTML 开头
2. **footnote popup 永远隐藏** ✅ — 根因: 1.5 CSS 抽取时 style.css 给
   #foliate-footnote-popup 加了 `display: none`, 但旧 js cssText 没有 display
   (默认 block)。修法: style.css 删 display:none; ui.js 清理重复内联 cssText
   (CSS 已有完整规则)
3. **批注选择菜单在 iframe 内不触发** ✅ — 根因: 旧代码只挂
   `document.addEventListener('selectionchange')`, 但 foliate-paginator 渲染在
   closed-shadow iframe 里, window.getSelection() 选不到正文。词典功能(5.4)
   早已用 content doc 监听模式解决。修法: reader.js 新增
   `_attachAnnotationSelectionListener()` (复用词典模式: 拿 renderer.getContents()
   [0].doc 挂 selectionchange, 内容未就绪时轮询 20×400ms); ui.js
   `_onSelectionChange(sel)` 接受 selection 参数, iframe selection 时通过
   `sel.anchorNode.ownerDocument.defaultView.frameElement.getBoundingClientRect()`
   加坐标偏移。

## 验证状态
- probe-legacy.js 全绿: open/saveAnnotation/IDB/footnote(display:block)/TTS/wordbook/progress
- node --check foliate-reader/{ui,reader}.js 通过
- **A. 真实 UI 交互验证 ✅ (2026-07-31 完成)**: iframe 内 TreeWalker 选中正文 → 菜单出现 →
  点击高亮 → overlayer 淡黄带渲染 → IDB 保存 → 重开书恢复, 全链路通过
  (diag-draw14~20 逐步物理验证: rect 属性/位置/层叠/像素全确认)

## 关键诊断结论 (高亮"看不见"真相)
- **高亮 rect 设计为淡黄: rx=3, fill=#ffff00, opacity=0.25** (reader.js _onDrawAnnotation L337-339)
  → 白底合成 ≈ (255,255,191)。**检测阈值必须用 r>235, g>235, b<205, |r-g|<25**;
  之前用 r>200,g>200,b<130 永远检测不到 → 误判"无黄带"
- overlayer svg 挂在 paginator **closed shadowRoot #container** (不是 iframe doc!)
  → 主窗口 document.querySelectorAll('svg') 找不到它 (被 shadow 隔离)
- #container 横向翻页布局 → svg 宽 286254px (正常, 非 bug); svg 从视口 (0,44) 覆盖阅读区
- diag-draw17 实测: 直接 append 红色 rect 到 svg 可见 → 证明 svg/shadow 渲染正常

## 待做 (已全部完成 2026-07-31)
- ~~A. 真实 UI 交互验证~~ ✅ (见上方"验证状态")
- ~~B. 旧 foliate-reader.js 死文件~~ ✅ — 已删, 保留 foliate-reader.js.bak
- ~~C. LS_ANNOTATIONS_PREFIX 死代码~~ ✅ — constants.js 定义行 + reader.js import 已删, grep 确认零残留
- ~~D. 书单 API 重复书籍~~ ✅ — 根因: novel_workspace/ 有 3 个"从武道杀到修仙" md
  (07-30 旧版 246KB + 07-31 两份 2484947B md5 相同=纯副本)。
  修法: 保留最新 003150, 另两份 mv 到 novel_workspace/duplicates/ (可恢复)。
  API 验证: 10→8 本, 无重复。
  ✅ 代码层已防(已写未部署): crawler.rs scan_shelf 已加"同 slug 保留最大 file_size"(retain+HashSet),
  编译未完成(机器内存耗尽 swap 满 IO 风暴, 41min CPU 仅 228s, 已止损中断)。
  将来: 内存宽裕时 `RUSTC=agent1-stable-toolchain/bin/rustc cargo build --release` 后 `systemctl restart museai-server`。
  (2026-07-31 另修复: systemd 服务 crash-loop 1985 次由野进程占 18766 导致, 已 kill 旧进程由 systemd 接管, active + 8 本无重复)

## 探针要点
- probe-real-selection.js 需等 [data-action="open"] 按钮出现(书单加载)再点击
- 书架有书时按钮会出现; 偶发等不到 → curl 确认 API 正常
- view.renderer.getContents() 在 open 后可能延迟就绪 → 轮询/重试
- iframe selection: content doc 是 same-origin, 可 doc.getSelection() + range

## 真实批注链路契约（foliate-js）
- `view.addAnnotation({value: CFI, ...})` — **value 必须是 CFI 不是文本**!
  (内部 resolveNavigation(cfi) → getOverlayer → overlayer.add + emit)
- view 会 emit `draw-annotation` 事件 {draw, annotation, doc, range}
  — **必须监听并提供 SVG draw 函数**, 否则高亮永远不绘制 (原 bug)
- `view.getCFI(index, range)` 生成 cfi; index = `view.renderer.getContents()[0].index`
- overlayer.add(key, range, draw, options): range 可函数 (doc)=>Range; draw(rects,opts)→SVG 元素
- 高亮 SVG: <g><rect x y width height rx fill opacity/></g> (svgNS)
- _loadAnnotations 也要用 ann.cfi 做 value (存 IDB 的 annotation 有 cfi 字段)
- 菜单 ID: `foliate-reader-annotation-menu` (不是 foliate-annotation-menu)
- 打开书按钮: `button img[src*="/api/v1/crawler/novels/"]` 的 closest('button')
  (不存在 [data-action="open"] 选择器!)
- 选中文本内容会含 URI 噪声(如 ": https://fa") — epubcfi 尾部, 正常
