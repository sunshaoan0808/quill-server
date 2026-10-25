# 小说 → 可穿越剧本：完整流水线

> 本文档梳理 Kaleido 故事馆如何把一部小说（txt/md/番茄链接）变成可穿越、可交互的剧本（StoryPack），
> 以及穿越者在其中如何获得"穿书"体验。代码级依据（2026-08-06 复核）。

## 0. 术语速查

| 概念 | 定义 | 代码位置 |
|---|---|---|
| StoryPack | 剧本包（一部小说的完整剧本化产物） | `crates/kaleido-core/src/story_tavern.rs:533` |
| StoryChapter | 章（来自小说章节切分） | 同文件 :263 |
| StoryNode | 剧情节点（章内的可推进单元） | 同文件 :203 |
| NodeExit | 节点出口（推进到下一节点的条件） | 同文件 :249 附近 |
| Playable | 玩法：P1私聊 / P2组队 / P3穿书 / P4同人 | 同文件 :113 |
| EntryRole | 玩家身份：Protagonist/Supporting/Isekai/Extra | 同文件 :132 |
| RewriteIntensity | 改写强度：Canon（严格原著）/ Rewrite（大改） | 同文件 :154 |
| PlayMode | 主线 / 自由 / 支线（游标冻结语义） | 同文件 :75 |
| StageDirectorConfig | 导演台配置（goal/pressure/cost 牵引主线） | 同文件 :558 |

## 1. 小说导入（两条路）

### 1.1 番茄小说爬虫（`crates/kaleido-server/src/crawler.rs`）
- 端点：`POST /api/v1/novels/import`（`crawler::novels_import`，main.rs:3629）
- 输入：`url` 或 `articleId`（articleId 自动归一化为 `/page/{id}`）+ `novelType`（长篇/短篇）
- `crawl_real()` 抓 `fanqienovel.com` 页面 HTML，从 `__INITIAL_STATE__.page` 提取元数据（`extract_novel_meta`），正文保存为 md 文本
- 短篇会抓取全部章节，长篇按 reader 链接逐章抓
- 默认 **CRAWLER_DISABLED**（`defaultOff:true`），需显式开启

### 1.2 直接贴文本 / 文件（`crates/kaleido-server/src/outline.rs`）
- 端点：`POST /api/v1/outline/reverse/preview`（`preview_reverse`）
- 输入：`text` 或 `filePaths`（workspace 相对路径的 md/txt）
- 产出 `ReverseOutlinePreviewResponse`：切好的章节 + 大纲 markdown + 模式标注

## 2. 章节切分（`outline.rs` `heuristic_split_chapters` :372）

逐行扫描 `detect_chapter_marker`：
- 中文标记：`第…章/回/节/部`（可带标题）
- 英文标记：`Chapter N` / `CHAPTER N`
- **无标记回退**：
  - ≤4000 字 → 单章整篇
  - >4000 字 → 按空行段落打包，每包 ~4000 字（"段落组 N"）

可选 `useLlm=true` → `polish_chapters_llm` 用 LLM 润色章节摘要（失败自动降级 heuristic，不阻塞）。

**实测**（白昼之下，`docs/BAIZHOU_IMPORT_SCENE_SMOKE.md`）：18 章 → 18/18 切分成功。

## 3. 剧本化：章节 → StoryPack（`story_tavern.rs` `StoryPack` :533）

每个 pack 包含：

### 3.1 章节 ↔ 节点（一一对应）
- `chapters: Vec<StoryChapter>` — id（ch01…）、title、order
- `nodes: Vec<StoryNode>` — id（n1…）、title、entry、exit、locked_beats、allowed_divergence、present_characters、location_id、summary
- 典型节点：`{id:"n1", title:"第1章", entry:"本章开始", exit:[{id:"e1", when:"继续", next:"n2"}]}`

### 3.2 角色卡（`characters: Vec<PackCharacterRef>`）
- 从正文自动提取（如孕船 pack 提取出 陆小峰/肖静/陆川… 18 角色；安全屋 8 角色）
- 旁白/读者 两个固定角色兜底

### 3.3 可玩判定（`is_playable()` :571）
```
≥2 角色 && ≥2 章 && ≥2 节点 && 至少一个节点有 exit 边 → 可开玩
```

### 3.4 扩展配置
- `stage_director: StageDirectorConfig` — 演出机导演台（S1 骨架，缺省空策略）
- `actor_state_config: ActorStatePackConfig` — Actor 状态模板，create_from_pack 时初始化进 session
- `event_packages` — 事件卡包（吞噬 denova）

## 4. 穿越初始化：pack → session（`create_from_pack` :2754）

点"开玩"后：
1. **tier 收敛**：user_tier × pack.max_tier × 各角色 content_tier 取 min
2. **入口身份**：`req.entry.vessel_character_id`（缺省取 pack 第一个角色）
3. **初始节点**：`node_id = pack.first_node_id()`（第一章首个节点）
4. **在场角色**：首节点的 `present_characters`（缺省全角色）
5. **章节游标**：`chapter_cursor = pack.first_chapter_id()`
6. 按 `playable` 选玩法 P1-P4；P2/P3/P4 开启 `speaker_rotation`

## 5. 穿越本体：玩家身份三态（`EntryRole` + `rebind-vessel` :801）

| 身份 | control_character_id | 说明 |
|---|---|---|
| Protagonist 主角 | vessel 原书主角 id | 附身主角 |
| Supporting 配角 | vessel 原书配角 id | 附身配角 |
| **Isekai 穿书者** | **`player_isekai`** | **不占原书角色位**，以穿越者身份混入（A9 规则：isekai 不要求 vessel） |
| Extra 路人 | `player_extra` | 路人身份 |

- `POST /sessions/{id}/rebind-vessel` 中途换身子：改 `entry.vesselCharacterId` + `player.controlCharacterId` + 追加 vessel_change 记录；vessel 自动补进在场角色；focus 移到其他 NPC
- 规则：protagonist/supporting 换绑必须给 vessel；isekai/extra 可清空

## 6. 剧本约束注入（`build_tavern_system_prompt` :1619）

每次 turn 把剧本编译成 LLM 指令（`start_turn` 调用）：

1. **身份锚定**：`玩家当前 vessel/身子：**{name}**（{id}）。请按此身份叙事，勿让玩家同时操控其他 NPC 身子。`
2. **模式语义**：
   - Mainline：优先服务节点目标与 exits，可推进剧情
   - Free：不强行推主线，允许闲聊
   - Side：主线游标冻结，可展开支线；回主线锚点 `resumeNodeId`
3. **偏离档（allowed_divergence）**：
   - `strict`：只能叙述原书情节，禁止编造/跳章；偏离要求必须拒绝拉回
   - `branch`：可适度分支，禁止引入原书不存在的人物/地点/能力；分支后回归主线
4. **改写强度（RewriteIntensity）**：
   - Canon：严格遵循原著（同 strict 语义）
   - Rewrite：允许大改，保持人设与世界风格连贯
5. **出口表**：列出所有 `exit.when → exit.next`，指令"若故事推进到合适出口，叙事末尾加【节点推进:节点ID】"
6. **硬节拍（locked_beats）**：必须逐一呈现，禁止跳过/合并/改写
7. **剧情概要（node.summary）**：必须严格遵循，不得偏离跳章
8. 可选注入：strict_mode_boost / pacing / style_guidance（`player.flags` 配置）

## 7. 穿越式阅读的运转

```
turn 请求
  → build_tavern_system_prompt（上述约束全量注入）
  → LLM 按当前节点叙事（stream）
  → split_*_from_narrative 剥离【选项】/【询问】/【面板】/【节点推进】/【状态更新】/【检查】等协议标记
  → 【节点推进:nX】命中 → try_advance_node（story_tavern.rs:3747）
  → node_id/chapter_cursor 前进 → 下一章节点继续
```

- **director 导演台**（`generate_director_plan_llm`，故事馆手动触发/自动）：生成 goal/pressure/cost + 命中 locked_beats 的导演计划，牵引主线不跑偏（2026-08-06 修复：必须走 resolve_llm + max_tokens 4096，见 kaleido-server-ops skill）
- **支线/番外**：`PlayMode::Side` 冻结主线游标，`resume_node_id` 提供回主线锚点；`build_side_branch_catalog` 生成整本总结 + 重要节点目录

## 8. 关键文件地图

| 文件 | 职责 |
|---|---|
| `crates/kaleido-server/src/crawler.rs` | 番茄小说爬虫导入 |
| `crates/kaleido-server/src/outline.rs` | 章节切分 + 反向大纲 + LLM 润色 |
| `crates/kaleido-core/src/story_tavern.rs` | StoryPack/Node/Session 领域模型 + create_from_pack + try_advance_node |
| `crates/kaleido-server/src/story_tavern.rs` | 路由 + build_tavern_system_prompt + 协议标记解析 + director |
| `crates/kaleido-server/src/st_export.rs` | pack export.zip / import zipBase64（回灌） |
| `crates/kaleido-server/src/st_import.rs` | 外部存档导入 |
| `data/story-packs/` | 33 个实际剧本包（pack.json + chapters/） |
| `docs/BAIZHOU_IMPORT_SCENE_SMOKE.md` | 导入 + 四大场景实测记录 |

## 9. 一句话总结

> **小说文本 → 章节切分 → 章节自动映射为"节点+出口+硬节拍+角色"的剧本包 → 玩家选身份（主角/配角/穿书者/路人）→ LLM 严格按当前节点约束叙事 → 节点出口触发【节点推进】自动游标前进 = 穿越式阅读。**

---

*维护：改动导入/节点/穿越语义时同步更新本文档；数据实测命令 `python3 /tmp/inspect_packs.py`（可复制到项目 scripts/）。*
