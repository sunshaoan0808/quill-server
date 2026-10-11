# 批注高亮链路诊断记录 (2026-07-31)

## 结论
高亮绘制链路已打通：cfi生成 → draw-annotation emit → e.detail 修复 → draw 回调收到 rects → svg rect 绘制（视觉确认黄带）。

## 关键物理发现
1. **foliate-paginator 是 closed shadowRoot**（`attachShadow({mode:'closed'})`）— 外部 JS/document.querySelectorAll **无法** 查到处 in shadow 的 svg/rect。
   - 验证方式必须用**截图颜色分析**，不能用 DOM 查询。
   - 手动 addAnnotation（有效 range）→ 截图出现黄色带 268px（x:162-294, y:223-240，17 行连续）→ 绘制成功。

2. **事件封装**：foliate-js view #emit 用 `CustomEvent`，payload 在 `e.detail`。`_onDrawAnnotation(e)` 原来解构 `{ann, draw}` 是错的（e 是 CustomEvent），已修复为 `e.detail.ann / e.detail.draw`。

3. **Overlayer**：view 创建 overlayer 时 `create-overlayer` 事件携带 `attach`，svg 挂在 closed shadowRoot 内的 view 元素上。`Overlayer` 类自带静态 `highlight/underline/squiggly` 方法（foliate 原版标准做法，支持多种样式）。

4. **根因（未决）**：UI 菜单点击 `_addAnnotation` 时 `this._selectedRange` 已为 false（diag-draw3 证据），导致 `getCFI` 返回 `epubcfi(/6/2)`（无范围 baseCFI）→ rects 空 → 不绘制。
   - 但 diag-ui 同场景 sel=true —— 两次运行矛盾，具体谁清 range 未定位。
   - 唯一清 `_selectedRange = null` 的代码：reader.js L228（_addAnnotation 尾部）。
   - ui.js `_onSelectionChange` / reader.js `_annotationSelHandler` 在空 selection 时只隐藏菜单，**不清 range**。
   - 猜测：evaluate 间 iframe selectionchange 时序/焦点差异（Playwright 合成 click 与真实 mousedown 焦点行为不同）。

## 修复方案
- `_addAnnotation` 里 range 兜底：`_selectedRange` 无效时遍历 contents 从 iframe `doc.getSelection()` 重新取（`!s.isCollapsed && s.rangeCount>0`）。
- 若真实用户跨 frame 点击导致 iframe selection 被清，需菜单按钮 `mousedown` preventDefault 阻止焦点转移。
- getCFI 失败时 cfi 为空 → draw 会空转，兜底后应生成完整 cfi（epubcfi(/6/2!/4/8,/3:2,/3:22) 格式）。

## 已验证
- IDB 持久化：点击高亮后 `ReadAwareAnnotations` count=1，type=highlight, color=#ffff00, hasCfi:true。
- `_addAnnotation` 无异常执行（marks: add-start → add-done → click-returned）。
- diag-ui 导航误报：实际 navs 只有 goto 初始导航，点击后无导航。

## 探针文件
- /root/workspaces/museai-web-android-brainstorm/museai-server/web/bookshelf/probe-annot-e2e.js （UI 全链路）
- /tmp/diag-ui.js, /tmp/diag-draw.js, /tmp/diag-draw2.js, /tmp/diag-draw3.js
- /tmp/vis-ann.js （手动 addAnnotation + 截图分析）
