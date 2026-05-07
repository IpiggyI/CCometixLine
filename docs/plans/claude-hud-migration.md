# 实施计划：CCometixLine → claude-hud 迁移

**任务时间戳**：2026-05-06
**计划版本**：v2（已整合独立审查发现）
**任务类型**：[x] 全栈（配置 + 可选 Fork 扩展）
**依赖工具**：claude-hud v0.1.0 plugin / Node.js 18+ / Claude Code 2.1.115+
**用户决策**：路径 A — 先纯配置试用，5 分钟后再决定下一步

---

## 一、需求映射表（9 项指标 × 支持度）

| # | 椰椰需求 | 支持度 | 实现方式 | 备注 |
|---|----------|:-----:|----------|------|
| 1a | 模型名称 | ✅ 原生 | `display.showModel: true`（默认） | `[Opus 4.7]` |
| 1b | 思考强度 | ✅ 原生 | 自动渲染 stdin.effort.level | Claude Code ≥ 2.1.115，符号 `○low ◔medium ◑high ◕xhigh ●max` |
| 2 | 项目名称 | ✅ 原生 | `pathLevels: 1-3` | 默认 1 级 |
| 3 | git 分支 | ✅ 原生 | `gitStatus.enabled: true` | 可选 dirty / aheadBehind / fileStats |
| 4 | 上下文 % + 数值 | ✅ 原生 | `display.contextValue: "both"` | `45% (45k/200k)` |
| 5 | 花费 | ✅ 原生 | `display.showCost: true` | 用 stdin `cost.total_cost_usd`，缺失时本地估算 |
| 6 | **距离上次 AI 回复时长** | ⚠️ 半支持 | Stage 1 用 promptCache 倒计时反推；Stage 2 加正向计时 | claude-hud 有 `lastAssistantResponseAt` 但只用作 cache 倒计时 |
| 7 | 工具/Agent/Todo | ✅ 原生 | `showTools/Agents/Todos: true` | 椰椰当前 config.json 已开启 |
| 8a | 总输入/输出 token | ✅ 原生 | `display.showSessionTokens` | `tokens 142k (in: 12k, out: 8k, cache: 122k)` |
| 8b | **最近一轮 token + 总数并列** | ❌ 不支持（Stage 1 妥协） | Stage 2 扩展 transcript.ts | 现状只累计，不保留 last-turn 字段 |
| 9a | **总缓存命中率 %** | ❌ 不支持（Stage 1 妥协） | Stage 2 扩展 | 数据具备，缺渲染 |
| 9b | 缓存倒计时 | ✅ 原生 | `display.showPromptCache: true` + `promptCacheTtlSeconds: 3600`（Max 套餐） | `⏱ 3m 15s` |

**Stage 1 满足度：6/9 全显 + 3/9 妥协**（妥协项：6/8b/9a）。

---

## 二、决策路径图

```
┌─────────────────────────────────────┐
│  Stage 1：纯配置切换（5 分钟）        │
│  - 切 statusLine → claude-hud       │
│  - 写完整 config.json                │
│  - 备份 ccline 配置                  │
└──────────────┬──────────────────────┘
               │ 椰椰试用 5 分钟评估
               ▼
       ┌───────┴───────┐
       │   体验如何？    │
       └───────┬───────┘
       ┌───────┴────────────────────────┐
       │                                │
   完美     可接受妥协            性能不行  /  缺 3 项扩展
   ✅       ✅                    ❌            ❌
   →保持   →保持            →路径 D       →路径 B (Fork)
                          (扩 ccline)    (扩 claude-hud)
```

---

## 三、Stage 1：纯配置切换（详细步骤）

### 3.1 当前现状（已侦察）

```
~/.claude/settings.json:
  statusLine: { type: "command", command: "ccline", padding: 0 }  ← 当前指向 CCometixLine

~/.claude/plugins/claude-hud/config.json:                          ← 已存在，部分已配置
  ✅ showModel, showTools, showAgents, showTodos, showSpeed, showDuration
  ❌ 缺 showCost, showPromptCache, language=zh, contextValue=both（已设）

~/.claude/ccline/config.toml:                                      ← 当前 CCometixLine 配置
  segments: model, directory, git, context_window, usage（disabled）
```

### 3.2 关键文件备份（防止破坏现有配置）

```bash
cp ~/.claude/settings.json                       ~/.claude/settings.json.pre-claude-hud
cp ~/.claude/plugins/claude-hud/config.json      ~/.claude/plugins/claude-hud/config.json.pre-claude-hud
# ccline 配置已经在 ~/.claude/ccline/，无需移动
```

### 3.3 修改步骤

#### Step 1 — 让 claude-hud 自己注入 statusLine 命令

```
# 在 Claude Code 中输入：
/claude-hud:setup
```

> 这条命令会自动找到当前最新版的 dist/index.js 路径并写入 `~/.claude/settings.json` 的 `statusLine` 字段。比手动写路径稳健（版本号会随 plugin 升级变化）。

#### Step 2 — 全量重写 `~/.claude/plugins/claude-hud/config.json`

**目标配置（覆盖现有）**：

```json
{
  "language": "zh",
  "lineLayout": "expanded",
  "pathLevels": 1,
  "showSeparators": false,
  "elementOrder": [
    "project",
    "context",
    "usage",
    "promptCache",
    "tools",
    "agents",
    "todos"
  ],
  "display": {
    "showModel": true,
    "showContextBar": true,
    "contextValue": "both",
    "showCost": true,
    "showSpeed": true,
    "showDuration": true,
    "showSessionTokens": true,
    "showTokenBreakdown": true,
    "showPromptCache": true,
    "promptCacheTtlSeconds": 3600,
    "showTools": true,
    "showAgents": true,
    "showTodos": true,
    "showSessionName": false,
    "showConfigCounts": false,
    "showUsage": true,
    "usageBarEnabled": true,
    "usageCompact": false,
    "showResetLabel": true,
    "timeFormat": "relative",
    "showOutputStyle": false,
    "showMemoryUsage": false,
    "showClaudeCodeVersion": false
  },
  "gitStatus": {
    "enabled": true,
    "showDirty": true,
    "showAheadBehind": true,
    "showFileStats": false,
    "branchOverflow": "truncate",
    "pushWarningThreshold": 5,
    "pushCriticalThreshold": 20
  },
  "colors": {
    "context": "green",
    "usage": "brightBlue",
    "warning": "yellow",
    "usageWarning": "brightMagenta",
    "critical": "red",
    "model": "208",
    "project": "142",
    "git": "magenta",
    "gitBranch": "109",
    "label": "dim"
  }
}
```

**配置说明**：

- `language: "zh"` — 中文 label
- `pathLevels: 1` — 项目名只显示当前目录（与 ccline 一致）
- `contextValue: "both"` — 满足需求 4（`45% (45k/200k)`）
- `promptCacheTtlSeconds: 3600` — Max 套餐用 1 小时缓存（如果是 Pro 套餐改为 300）
- `gitStatus.showAheadBehind: true` — 显示 `↑3` 推送计数
- 颜色 256 色码：208 橙（model）、142 黄绿（project）、109 青（git branch）→ Gruvbox 调色板对齐

#### Step 3 — 重启 Claude Code

```
# 在 Claude Code 中输入：
/reload-plugins
# 或退出后重新进入
```

### 3.4 视觉模拟（终端宽度 ≥ 100 列）

```
[Opus 4.7] ●max │ ccline git:(main* ↑3)
Context ████████░░ 78% (155k/200k) │ Usage ██░░░░░░ 25% (resets in 1h 30m)
⏱ Cache 47m 12s
◐ Edit: config.toml | ✓ Read ×8 | ✓ Grep ×3
◐ explore [haiku]: Locating effort handler (1m 22s)
▸ Migrate statusline to claude-hud (3/7)
tokens 142k (in: 12k, out: 8k, cache: 122k)
$0.42 │ ⏱️ 12m
```

**信息分层**：

| 行 | 内容 | 注视频率 |
|---|------|---------|
| L1 | 模型 + 思考强度 + 项目 + git | 始终 |
| L2 | 上下文 + 用量限额 | 高频（context 警戒） |
| L3 | Cache 倒计时（仅在缓存活跃时） | 中频 |
| L4 | 工具活动（仅在工具运行时） | 中频 |
| L5 | Agent 活动（仅在 Agent 运行时） | 中频 |
| L6 | Todo 进度（仅在有待办时） | 中频 |
| L7 | Token 累计 | 低频参考 |
| L8 | Cost + Duration | 低频参考 |

**窄终端策略（< 80 列）**：claude-hud 内置 `wrapLineToWidth`，自动折行。

### 3.5 Stage 1 妥协项

| 妥协项 | 妥协方式 |
|--------|----------|
| 需求 6（距离上次 AI 回复） | **复用** `showPromptCache` 倒计时反推：`总 TTL - 剩余 = 已过`，需脑算 |
| 需求 8b（最近一轮 token） | **暂只显示**总 token，最近一轮**不展示** |
| 需求 9a（缓存命中率%） | **暂不展示**，仅有 cache 倒计时 |

### 3.6 5 分钟评估检查表

试用 5 分钟后，按以下指标自评，决定下一步：

- [ ] 终端是否流畅？是否感受到明显卡顿？（**性能税评估**）
- [ ] 信息排布是否清晰？多行是否影响输入？
- [ ] 缓存倒计时是否够用？（替代正向计时的妥协是否可接受）
- [ ] 总 token 是否够用？（替代最近一轮的妥协）
- [ ] 缓存命中率不展示，是否影响决策？

**通过 → 保持 Stage 1 终态**
**不通过 → 进入 Stage 2 选择路径 B 或 D**

---

## 四、Stage 2 选项（按需启动）

### 路径 B：Fork claude-hud（Node.js）

| 维度 | 评估 |
|------|------|
| 工作量 | ~75 LOC，4 文件改动，半天工作量 |
| 性能 | Node.js 启动 ~50-200ms × 每 300ms 调用，**WSL2 下电池/CPU 影响明显** |
| 维护 | claude-hud 是 v0.x，semver 前 1.0，TRANSCRIPT_CACHE_VERSION 已迭代到 3，**每次上游升级需 rebase 3 个 patch** |
| 上游同步成本 | 中-高 |
| 适合场景 | 需要 Tools/Agents/Todos **可视化** + 接受 Node.js 性能税 |

#### 4.1.B 三处扩展点

**Gap A — 距离上次 AI 回复正向计时**（~40 LOC）：

新增 `src/render/lines/since-reply.ts`，逻辑：`now - lastAssistantResponseAt.getTime()`，注册到 HudElement union 和 render/index.ts。

**Gap B — 最近一轮 token 拆分**（~30 LOC）：

修改 `src/transcript.ts:271` 在 usage 累加循环中保留 lastTurn 快照（覆盖式写）。修改 `session-tokens.ts` 显示 `in: 12k|142k, out: 8k|34k`。注意：**TRANSCRIPT_CACHE_VERSION 需要 +1**，会一次性失效现有 cache。

**Gap C — 总缓存命中率**（~5 LOC）：

公式（采用 Anthropic 官方语义）：
```
hit_rate = cache_read_input_tokens / (input_tokens + cache_read_input_tokens + cache_creation_input_tokens) × 100
```

> ⚠️ 命中率公式有歧义。也可以理解为 `cache_read / (cache_read + uncached_input)`，排除 cache_creation。**确认前先按 Anthropic 官方定义实现，留 flag 切换**。

### 路径 D：反向扩展 CCometixLine（Rust，⭐ 审查推荐）

| 维度 | 评估 |
|------|------|
| 工作量 | ~150 LOC（Rust），1-2 天工作量 |
| 性能 | Rust 二进制启动 < 5ms，**零性能税** |
| 维护 | 自有 fork，无上游 rebase 成本 |
| 上游同步成本 | 低（CCometixLine 上游变化也较慢） |
| 风险 | **需先确认 ccline 是否已支持 Tools/Agents/Todos 解析**——如果未支持，需自建 transcript JSONL 解析器（Rust），工作量翻倍 |

#### 4.1.D 评估前置任务

在选择路径 D 前需要：

1. 检查 `~/.claude/ccline/ccline --help` 或 `ccline --version` 确认当前 CCometixLine 版本能力
2. 找到 ccline 源码仓库（README/二进制元数据）
3. 评估其 transcript 解析能力是否覆盖 Tools/Agents/Todos
4. 评估 Rust 实现的 3 项扩展工作量

### 路径选择决策

| 优先选择 | 触发条件 |
|---------|----------|
| **B (Fork)** | 重视 Tools/Agents/Todos 可视化，能接受 ~100ms statusline 刷新延迟 |
| **D (扩 ccline)** | 重视性能/续航，可放弃 Tools/Agents/Todos 视觉效果 |
| **B + D 并行** | 实验型——同时维护两套，对比 1 周后定 |

---

## 五、风险与缓解

| 风险 | 概率 | 影响 | 缓解措施 |
|------|:---:|:---:|----------|
| Node.js 启动开销在 WSL2 上明显 | **高** | 中 | Stage 1 试用 5 分钟实测；不行就转路径 D |
| effort 显示需要实测确认 | 中 | 低 | 当前 Claude Code 2.1.129 ≥ 2.1.115，stdin 应有 effort 字段。如果不显示，查 effort.ts 是否需要 flag |
| claude-hud 上游 v0.x 频繁变更 | 中 | 中 | Fork 时锁定版本 tag；Stage 1 用插件原版规避 |
| Linux TMPDIR EXDEV 安装失败 | 低 | 低 | 已装过，无需再装 |
| Cost 字段缺失（首次 API 响应前） | 中 | 低 | 已知行为，会话开始几秒不显示，正常 |
| 切换后丢失 Gruvbox 视觉 | 低 | 低 | colors 配置 256 色码已对齐 |
| Cache 命中率公式歧义 | 中 | 低 | 路径 B 实现时按 Anthropic 官方语义，留 flag 切换 |

---

## 六、回滚方案

**Stage 1 回滚**（10 秒）：
```bash
cp ~/.claude/settings.json.pre-claude-hud ~/.claude/settings.json
# 重启 Claude Code，恢复 ccline 状态行
# claude-hud config 保留以便再次启用
```

**Stage 2 回滚**：
- 路径 B：把 statusLine.command 改回插件原版路径，保留 fork 备查
- 路径 D：把 statusLine.command 改回原 ccline 二进制（备份在 `~/.claude/ccline/ccline.v1.1.2.bak`）

---

## 七、关键文件 / 操作清单

### Stage 1（必做）

| 文件 | 操作 | 由谁执行 |
|------|------|---------|
| `~/.claude/settings.json` | 备份 + `/claude-hud:setup` 写入 | Claude Code 命令 |
| `~/.claude/plugins/claude-hud/config.json` | 备份 + 全量重写 | 工具 Edit/Write |
| `~/.claude/ccline/*` | **保留**（备份用，不动） | — |

### Stage 2（按需启动）

| 路径 | 文件 | 操作 |
|------|------|------|
| **B Fork** | `/home/hyy/develop/personal/DailyConversation/ccline/` | clone claude-hud 到此目录 |
| **B Fork** | `<fork>/src/render/lines/since-reply.ts` | 新增 |
| **B Fork** | `<fork>/src/render/lines/cache-hit-rate.ts` | 新增 |
| **B Fork** | `<fork>/src/transcript.ts` | 扩展 lastTurn 字段 + bump CACHE_VERSION |
| **B Fork** | `<fork>/src/render/lines/session-tokens.ts` | 加 last\|total 双展示 |
| **B Fork** | `<fork>/src/config.ts` | 扩 HudElement union + 3 个 display flags |
| **B Fork** | `<fork>/src/i18n/{en,zh}.ts` | 加 3 个 label |
| **B Fork** | `<fork>/src/render/index.ts` | 路由 3 个新元素 |
| **D 扩 ccline** | （待评估）| 视 ccline 源码情况而定 |

---

## 八、验收判据

### Stage 1 验收（5 分钟试用）

- [ ] 状态行已切换，不再是 CCometixLine
- [ ] L1 显示 `[模型 思考强度] | 项目 git:(分支)`
- [ ] L2 显示 `Context X% (Xk/Yk) | Usage X%`
- [ ] cost、cache 倒计时、tokens 累计在适当位置出现
- [ ] 中文 label 正常
- [ ] 推送计数 `↑N` 在有未推送 commit 时显示
- [ ] **5 分钟内未感受到明显卡顿**（性能验收）
- [ ] 信息密度可接受（不影响输入区视觉）

### Stage 2 验收（如启动）

- [ ] 三项新指标在状态行可见
- [ ] last\|total token 格式正确
- [ ] 缓存命中率公式正确（与 Anthropic 官方一致）
- [ ] 距上次回复时间随时间增长（每 ~300ms 刷新）
- [ ] 性能：路径 B 不超过 200ms / 路径 D < 10ms

---

## 九、模型协作日志

| 角色 | 状态 | SESSION_ID |
|------|------|------------|
| Claude（主导合成） | ✅ 完成 — 直接读 claude-hud 源码 | N/A |
| Codex 后端架构师 | ❌ Token 失效（401 Unauthorized），需椰椰重新登录 | N/A |
| Gemini 前端架构师 | ❌ Antigravity 版本被 deprecate | N/A |
| **独立审查 Agent** | ✅ **完成** — 抓出三个盲点 | adb022823e8ef89da |

### 审查 agent 关键发现（已纳入计划）

1. **性能税真实**：Node.js 启动 50-200ms × 每 300ms 调用 vs Rust < 5ms。WSL2 下尤甚。
2. **路径 D（扩 ccline）值得评估**：避免 Node.js 性能税 + v0.x 升级地狱。
3. **缓存命中率公式有歧义**：Anthropic 官方定义 ≠ 直觉定义，需明确。
4. **椰椰已云装但未试用**（Q&A 已确认），统一切换风险低。

---

## 十、下一步

椰椰决策：**路径 A — 先纯配置试用 5 分钟**

执行命令（待椰椰确认后）：
```
/ccg:execute .claude/plan/claude-hud-migration.md
```

执行流程将：
1. 备份 `~/.claude/settings.json` → `.pre-claude-hud`
2. 备份 `~/.claude/plugins/claude-hud/config.json` → `.pre-claude-hud`
3. 调用 `/claude-hud:setup` 写入 statusLine 命令
4. 用目标 JSON 覆盖 claude-hud 配置
5. 提示重启 Claude Code
6. 设置 5 分钟评估检查表，等待椰椰回报

