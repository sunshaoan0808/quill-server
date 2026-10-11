# ReadAware 全盘 Copy — 实施计划 📚

> 状态追踪文件。完成一项就在 `[ ]` 前打钩 `[x]`。

---

## 现状

```
已就绪：
  ✅ foliate-js 引擎（24 模块）已 vendored，loader.js 已挂载
  ✅ foliate-bridge.js（384 行）已部署（内容重构 + 消毒 + makeBook 导出）
  ✅ index.html 正确加载所有脚本
  ✅ API 端点全部可用（8 部小说，100K-150K chars/部）

缺失的核心：
  ❌ React bundle 阅读器对无标题内容（6/8 部）产生 0 章节 → 空白
  ❌ foliate-js 的 <foliate-view>/<foliate-paginator> 从未实例化
  ❌ 所有高级功能（搜索/批注/TTS/词典/分页/进度/书签）未接线
```

---

## Phase 0 — 基础修复

> 已全部完成 ✅

- [x] **0.1** `index.html` — 修复语法错误，正确加载 foliate-js loader + bridge
- [x] **0.2** `foliate-bridge.js` — 内容重构 `restructureContent`, 章节归一化 `normalizeChapters`, HTML 消毒 `sanitize`
- [x] **0.3** API 验证 — 所有 `/api/v1/crawler/novels` 和 `*/content` 正常返回

---

## Phase 1 — Foliate 阅读器集成（核心替换）

> **目标**：用 `<foliate-view>` + `<foliate-paginator>` 替换 React bundle 的 `dangerouslySetInnerHTML` 阅读器
>
> 技术方案：把 API markdown 内容编译为单个 HTML 文件 → `new File()` → `makeBook(file)` → 喂给 `<foliate-view>`。这样 paginator 自动分页，overlayer/search/tts 全部可用。
>
> 预计工期：**4-5 天**

- [x] **1.1** `foliate-reader.js` **新建** — 封装 `FoliateReader` 类
  - 创建 `<foliate-view>` 实例挂载到容器
  - `async open(slug)` 方法：fetch API → markdown→HTML → makeBook → view.open
  - 生命周期管理（close / destroy / resize）
  - [x] index.html 追加 script 标签
  - [x] 修复 bugs: load() 返回 blob URL（非 Document）、移除 sanitize 调用
  - [x] 覆盖 all 8 种 API 返回格式（对象/纯字符串/normalize 后）
- [x] **1.2** mkdir `foliate-reader/` 目录，将 reader 拆分为多个子模块
  - `foliate-reader/reader.js` — FoliateReader 核心
  - `foliate-reader/markdown.js` — markdown→HTML 转换管道
  - `foliate-reader/ui.js` — 工具栏/控件 UI
  - `foliate-reader/style.css` — 阅读器样式
- [x] **1.3** `markdown→HTML 转换` — 内联在 reader.js 中
  - 简易 markdown 转换器：h1-h6、p、ul/ol、table、pre/code、blockquote、hr、img、strong/em、br
  - 注入 foliate-js 可识别的 CSS 类名和结构
  - 支持代码块/表格/图片等格式
- [x] **1.4** **劫持 React 阅读按钮**（实现在 reader.js 中）
  - 分析 React bundle 确定卡片结构：`<button>` 内含 `<img>` 封面 + 标题
  - slug 存在 JS 闭包中，不在 DOM data 属性中
  - 解决方案：capture-phase click 拦截 + 封面图 URL 提取 slug
  - hash 路由监听 `#/reader/{slug}` 支持直接 URL 访问
- [x] **1.5** `index.html` / CSS — FoliateReader 样式
  - 全屏/模态容器
  - 暗色/亮色模式基础变量
  - 工具栏固定定位
- [x] **1.6** **验证** — 所有 8 部小说可正常分页阅读，无白屏 ✓

---

## Phase 2 — 阅读器控件

> **目标**：主题/字体/全屏/手势/进度条
>
> 预计工期：**3-4 天**

- [x] **2.1** 主题切换 ✓ — 亮/暗/棕/羊皮纸 4 种主题
  - 通过 CSS custom properties 实现
  - `view.element.style.setProperty('--theme-bg', ...)`
  - 存储偏好到 localStorage
- [x] **2.2** 字体大小 ✓ — 滑动条控制
  - `view.element.style.fontSize = value + 'px'`（paginator 自动响应）
  - 预设 5 档（小/中/大/超大/巨大）
  - 字体族切换（衬线/无衬线/等宽）
- [x] **2.3** 全屏 ✓ — Fullscreen API
  - `document.documentElement.requestFullscreen()`
  - 图标切换（进入/退出）
  - 快捷键 `f`
- [x] **2.4** 进度条 ✓ — 底部进度显示
  - 监听 view 的 `relocate` 事件 → `{ fraction, page, totalPages }`
  - 显示百分比 + "第 X 页 / 共 Y 页"
  - 可拖拽跳转
- [x] **2.5** 手势翻页 ✓ — paginator 原生支持
  - 点击左/右侧 1/3 区域翻页
  - 触摸滑动翻页
  - 键盘 `←` `→` 翻页
- [x] **2.6** 工具栏 UI ✓ — 顶栏 + 底栏
  - 顶栏：返回 / 书名 / 设置齿轮 / 全屏
  - 底栏：进度 / 主题 / 字体 / 目录 / 搜索 / TTS
  - 自动隐藏（空闲 3 秒渐隐，触摸/鼠标移动显示）

---

## Phase 3 — 导航与目录

> **目标**：章节侧栏、导航历史、进度持久化
>
> 预计工期：**2-3 天**

- [x] **3.1** 目录（TOC）提取
  - 从 markdown headings（`#`, `##`, `##`）提取章节树 ✓
  - 构建 `book.toc` 结构供 `<foliate-view>` 识别 ✓
  - 支持三级标题嵌套 ✓
- [x] **3.2** TOC 侧栏 UI
  - 左侧滑出面板 ✓
  - 缩进展示层级 ✓
  - 当前章节高亮（待完善）
  - 点击跳转（`view.goTo(href)`) ✓
  - 快捷键 `t` ✓
- [x] **3.3** 导航历史
  - `view.history.back()` / `view.history.forward()`（foliate-view 原生支持）✓
- [x] **3.4** 阅读进度持久化
  - 整合 `progress.js` 的 `SectionProgress` ✓
  - 存储到 localStorage（`readaware:progress:{slug}`) ✓
  - 打开时自动恢复 ✓
  - 多本书独立跟踪 ✓
- [x] **3.5** 快捷键完整支持
  - `←` `→` 翻页 ✓
  - `f` 全屏 ✓
  - `t` 目录 ✓
  - `s` 搜索（占位）

---

## Phase 4 — 搜索与批注

> **目标**：全文搜索 + 高亮/下划线/笔记
>
> 预计工期：**5-6 天**

- [x] **4.1** `search.js` 集成
  - `view.search(query)` 全文搜索
  - 搜索框 UI（工具栏或快捷键 `s` 弹出）
  - 结果列表显示（上下文片段 + 页码）
  - 条目跳转（点击结果→跳转到对应位置）
  - 上/下一个结果导航
- [x] **4.2** Overlayer 集成
  - `view.addAnnotation(key, range, draw)` 高亮匹配
  - 搜索匹配自动高亮
  - 不同颜色高亮方案
- [x] **4.3** 批注系统
  - 选择文本 → 浮动菜单（高亮/下划线/波浪线/笔记）
  - Overlayer 渲染批注（SVG）
  - 点击批注可编辑/删除
  - 颜色选择器
- [x] **4.4** `footnotes.js` 集成
  - 检测 HTML 中的脚注链接
  - 弹出层显示脚注内容
  - 支持返回阅读位置
- [x] **4.5** 批注持久化
  - IndexedDB 存储（`readaware:annotations:{slug}`)
  - 数据结构：`[{id, slug, chapterIndex, range, type, color, text, note, createdAt}]`
  - 重新打开时恢复所有批注
  - 导出/导入批注

---

## Phase 5 — TTS 朗读与词典

> **目标**：语音朗读 + 查词 + 引用图片
>
> 预计工期：**4-5 天**

- [x] **5.1** `tts.js` 集成
  - 使用 `view.getContents()[0].doc.body.innerText` 获取文本
  - 支持 Web Speech API / SSML
  - 语速控制（0.5x ~ 2.0x）
  - 语音选择（可用语音列表）
- [x] **5.2** TTS UI
  - 播放/暂停/停止控件
  - 朗读高亮当前句子（Overlayer 联动）
  - 从当前位置/章节/选中文本开始朗读
  - 后台朗读（切换到其他 Tab 继续）
- [x] **5.3** 词典集成 (`foliate-reader.js` 内联)
  - 使用 FreeDictionaryAPI (api.dictionaryapi.dev) 在线查词
  - 选中自动查词（500ms 防抖）
  - 工具栏「查词」按钮 + 快捷键 `D`
  - 支持 `view.renderer.getContents()[0].doc` 获取 shadow DOM 内选区
- [x] **5.4** 词典 UI
  - 浮动释义弹出窗口（跟随选区位置，不遮挡阅读）
  - 发音播放按钮（UK/US 音标 + audio）
  - 生词本（IndexedDB 存储 + 收藏/取消收藏）
- [x] **5.5** `quote-image.js` 集成（浏览器实测 PASS 2026-07-31：test-quote-image.js 全流程 — 选中→词典弹窗→引用图片 800x1000→PNG 下载→toBlob 剪贴板）
  - 选中文字 → "生成图片" 按钮（查词弹窗内自动附加）
  - Canvas 渲染引用卡片（带书名/背景/装饰）
  - 下载 PNG / 复制到剪贴板

---

## Phase 6 — 多格式上传

> **目标**：EPUB/PDF/MOBI 上传阅读 + OPDS 网书库
>
> 预计工期：**4-5 天**

- [x] **6.1** 文件选择器 UI
  - 拖拽 + 点击上传区域
  - 支持 `.epub`, `.pdf`, `.mobi`, `.azw3`, `.fb2`, `.cbz`, `.cbr`
  - 文件大小限制检测
- [x] **6.2** `makeBook()` 直接调用
  - 上传文件 → `makeBook(file)` → `view.open(book)`
  - foliate-js 原生格式解析（epub.js, mobi.js, pdf.js 等）
  - 格式回退链（EPUB → HTML → 纯文本）
- [x] **6.3** 嵌入 PDF
  - 使用 `<embed>` 或 pdf.js 后备
  - 对于非 PDF 的固定布局格式检测
- [x] **6.4** 本地书籍管理
  - IndexedDB 存储上传文件（或 File System Access API）
  - 元数据提取（书名/作者/封面/页数）
  - 书架中显示"本地书籍"分栏
  - 删除/重新上传
- [x] **6.5** `opds.js` 集成
  - OPDS 目录输入（URL 配置）
  - 浏览/搜索/下载
  - 标准 OPDS 1.2 / 2.0 支持

---

## 架构图

```
index.html
│
├─ React Bundle (assets/index-DiShLB2e.js)
│   └─ 书架网格/列表展示
│       └─ "阅读" 按钮 ──(劫持)──►  FoliateBridge
│                                      │
│                                  FoliateReader
│                                  ┌─────────────────────┐
│                                  │  <foliate-view>     │
│                                  │  ┌───────────────┐  │
│                                  │  │ paginator.js  │  │
│                                  │  │ 分页引擎       │  │
│                                  │  ├───────────────┤  │
│                                  │  │ overlayer.js  │  │
│                                  │  │ 批注层 (SVG)  │  │
│                                  │  ├───────────────┤  │
│                                  │  │ search.js     │  │
│                                  │  │ 全文搜索       │  │
│                                  │  ├───────────────┤  │
│                                  │  │ tts.js        │  │
│                                  │  │ 语音朗读       │  │
│                                  │  │ dict.js       │  │
│                                  │  │ 词典查词       │  │
│                                  │  └───────────────┘  │
│                                  └─────────────────────┘
│
└─ UI Controls (foliate-reader/ui.js)
     ├─ 顶栏: 返回 | 书名 | ⚙️ 设置 | ⛶ 全屏
     ├─ 底栏: 进度条 | 🌙主题 | A🔤字体 | 📖目录
     │        | 🔍搜索 | 🔊TTS | ✏️批注
     └─ 侧栏: 目录树 | 书签 | 批注列表
```

---

## 文件结构（最终）

```
/web/bookshelf/
├── PLAN.md                              ★ 本文件
├── index.html                           ✓ 已修复
├── foliate-bridge.js                    ✓ 基石
├── foliate-reader/                      ★ 新建核心目录
│   ├── reader.js                        — FoliateReader 主类
│   ├── markdown.js                      — markdown 转换管道
│   ├── ui.js                            — 工具栏/控件 UI
│   └── style.css                        — 阅读器专用样式
├── foliate-tts.js                       ★ Phase 5 新建
├── foliate-dict.js                      ★ Phase 5 新建
├── foliate-search-ui.js                 ★ Phase 4 新建
├── foliate-annotations.js               ★ Phase 4 新建
├── foliate-book-manager.js              ★ Phase 6 新建
├── foliate-js/                          ✓ 已有（核心引擎）
│   ├── loader.js                        ✓ 已挂载
│   ├── view.js, paginator.js, ...
│   └── ...
└── assets/
    ├── index-DiShLB2e.js                (React bundle - 仅用作书架列表)
    └── index-D-rm2kp-.css
```

---

## 里程碑

| 里程碑 | Phase | 交付物 | 预计工期 |
|--------|-------|--------|----------|
| **M1** | P1 | 分页阅读器可用，API 小说可正常分页阅读 | 4-5 天 |
| **M2** | P2 | 主题切换、字体调整、全屏、进度条、手势翻页 | 3-4 天 |
| **M3** | P3 | TOC 侧栏、导航历史、进度持久化 | 2-3 天 |
| **M4** | P4 | 搜索高亮、批注系统（含存储） | 5-6 天 |
| **M5** | P5 | TTS 语音朗读、词典查词、引用图片 | 4-5 天 |
| **M6** | P6 | EPUB/PDF/MOBI 上传阅读、OPDS 网书库 | 4-5 天 |

**总计**：约 **28-35 个工作日**（~6-7 周）

---

## 风险

| 风险 | 概率 | 影响 | 缓解 |
|------|------|------|------|
| React bundle 劫持点失效 | 中 | 高 | MutationObserver + 多重选择器 fallback |
| `makeBook()` 对单 HTML 支持不完善 | 中 | 高 | 方案 B 备选（构造 Book 对象直接喂入 view） |
| 150K+ 字符分页性能 | 低 | 中 | 懒加载 + Web Worker 预处理 |
| server 端无 EPUB 接口 | — | — | 所有格式转换客户端完成，不依赖 server |
| IndexedDB 配额限制（大文件） | 低 | 低 | 检测配额，提示用户 |
