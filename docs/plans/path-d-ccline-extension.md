# 实施计划：CCometixLine 路径 D 扩展（Rust 原生扩展 3 项缺失指标）

**任务时间戳**：2026-05-06
**计划版本**：v3.1（v2 阈值染色 + v3 多行布局 + v3.1 行长均衡）
**任务类型**：[x] 后端（Rust 数据层 + 新 segment）+ [x] 前端（终端 ANSI 视觉 + 阈值染色 + 多行布局 + 行长均衡）
**关联文档**：`docs/plans/claude-hud-migration.md`（路径 A 与 D 的决策来源）
**前置依赖**：CCometixLine v1.1.2 源码（当前仓库）+ Rust 1.70+ + chrono 0.4（已在 Cargo.toml）+ crossterm 0.29（已在依赖中，用于读终端宽度）
**v2 变更**：椰椰反馈复刻 claude-hud "低上下文绿/临界黄/危险红" 阈值染色机制 → 新增 §三.5 + Step 7
**v3 变更**：椰椰反馈复刻 claude-hud "expanded 多行布局" 机制 → 新增 §三.6 + Step 8
**v3.1 变更**：椰椰反馈"避免短文本单独占一行 / 行长尽量均衡" → 新增 §三.6.6 行长均衡算法 + Step 8.1（claude-hud 没做的能力升级）

---

## 一、目标与背景

椰椰已试用路径 A（claude-hud），但因 WSL2 下 Node.js 启动税（50–200ms × 每 ~300ms 刷新）放弃。路径 D 将剩余 3 项缺失指标**反向扩展进 CCometixLine** 的 Rust 单 binary，保持 < 5ms 启动，零 Node.js 依赖。

**3 项 Gap 现状对照表**：

| Gap | 需求 | 当前 ccline 状态 | 路径 D 改动 |
|-----|------|-----------------|-----------|
| **A** | 距上次 AI 回复时长（正向计时）| ❌ 不支持。`TranscriptEntry` 未捕获 `timestamp`；无对应 segment | 新增 `since_reply` segment |
| **B** | 最近一轮 token 与会话总和并列（`in: 12k\|142k` 风格） | ⚠️ 半支持。现存 segment 仅看 last-turn，无累计 | 新增 `session_tokens` segment + 数据层会话累加器 |
| **C** | 总缓存命中率 | ⚠️ 半支持。`cache_hit` segment 已实现 last-turn 命中率（公式与 Anthropic 官方一致），但无累计 | 给 `cache_hit` 加 `mode` 选项（last/total/both） |
| **D（v2 新增）** | 阈值染色（claude-hud 风格：低绿/中黄/高红） | ❌ 不支持。`ColorConfig` 仅 icon/text/background 静态色 | 扩 `ColorConfig` + 新增 `ThresholdLevel`，4 个百分比 segment 计算 level，renderer 选色 |
| **E（v3 新增）** | 多行布局（claude-hud expanded：每元素一行 + mergeGroups 同行合并） | ❌ 不支持。`generate()` 主路径恒返回单行字符串；TUI preview 有宽度回退但不在终端输出路径上 | 新增 `[layout]` 顶层配置块；`StatusLineGenerator` 输出 `Vec<String>`；mergeGroups 同行合并；超宽度自动拆回 |

**关键发现**：路径 D 核心算法（命中率公式、JSONL 解析）已存在，工作量主要在数据层重构 + 视觉层动态染色装配 + 渲染层多行重构。

---

## 二、降级说明（多模型协作）

| 角色 | 状态 |
|------|------|
| Claude（合成） | ✅ 直接 Read 源码 + 真实 JSONL 样本，已掌握全部上下文 |
| codex 后端架构师 | ❌ HTTP 401 — `wss://s2a.282560.xyz/v1/responses` 鉴权失效（与 `claude-hud-migration.md` 第九节失败原因一致） |
| gemini 前端架构师 | ❌ `GEMINI_API_KEY` 未设置 |

两路凭证级失败为非瞬态错误，已跳过重试。本计划由 Claude 单模型合成，**置信度依据**：源码已 Read 至关键文件全文 + 已用真实 JSONL 验证 schema（`timestamp` 字段确实存在为 ISO-8601 UTC）。

---

## 三、架构总览

### 3.1 现有架构（已侦察）

```
src/
├── config/
│   ├── types.rs          # InputData, RawUsage, NormalizedUsage, TranscriptEntry, SegmentId
│   ├── defaults.rs       # 默认 Config
│   └── loader.rs         # TOML 加载
├── core/
│   ├── transcript.rs     # parse_last_assistant_usage() — 仅向后扫描首个 assistant
│   ├── statusline.rs     # collect_all_segments() 大 match 注册表
│   └── segments/
│       ├── mod.rs        # trait Segment + re-export
│       ├── cache_hit.rs  # ✅ 已实现 last-turn 命中率（Gap C 半成品）
│       ├── context_window.rs / cost.rs / session.rs / ...
└── ui/
    ├── themes/theme_gruvbox.rs   # 颜色基准（208/142/109）
    └── components/segment_list.rs # SegmentId → 显示名映射
```

**Segment 注册触点（共 4 处必改）**：

| 文件 | 位置 | 当前条目数 | 改动 |
|------|------|-----------|------|
| `src/config/types.rs` | `enum SegmentId` | 10 | +2 (`SinceReply`, `SessionTokens`) |
| `src/core/segments/mod.rs` | `pub mod` + `pub use` | 10 | +2 |
| `src/core/statusline.rs` | `collect_all_segments` 大 match | 10 | +2 match 臂 |
| `src/ui/components/segment_list.rs` | `SegmentId → &'static str` 映射 | 10 | +2 |

### 3.2 新架构

新增**会话级累加器**作为数据层基础设施，3 个 segment 都消费它：

```
                ┌────────────────────────────────┐
                │  parse_session_stats(path)     │  ← 新数据层 API
                │  → SessionUsageStats {         │
                │      totals: NormalizedUsage,  │
                │      last_turn: NormalizedUsage,│
                │      last_assistant_at:        │
                │        Option<DateTime<Utc>>,  │
                │      assistant_count: u32,     │
                │  }                             │
                └────┬───────────┬───────────┬───┘
                     │           │           │
              since_reply   cache_hit    session_tokens
              (Gap A)       (Gap C)      (Gap B)
```

**保留 `parse_last_assistant_usage`** 作为兼容包装：内部调 `parse_session_stats` 取 `last_turn`；老 segment（`context_window`）零改动。

---

## 三.5、claude-hud 阈值染色机制移植（v2 新增）

### 3.5.1 claude-hud 现状（已 Read 源码 `~/.claude/plugins/cache/claude-hud/.../src/render/colors.ts`）

```typescript
// colors.ts:124-134
function getContextColor(percent, colors, thresholds) {
  const critical = thresholds?.critical ?? 85;
  const warning = thresholds?.warning ?? 70;
  if (percent >= critical) return resolveAnsi(colors?.critical, RED);     // 红
  if (percent >= warning)  return resolveAnsi(colors?.warning,  YELLOW);  // 黄
  return resolveAnsi(colors?.context, GREEN);                              // 绿
}

// colors.ts:136-140
function getQuotaColor(percent, colors) {           // usage 专用，阈值 hardcoded
  if (percent >= 90) return RED;
  if (percent >= 75) return BRIGHT_MAGENTA;
  return BRIGHT_BLUE;
}
```

**调色板配置位置**：`config.json::colors.{context, warning, critical, usage, usageWarning}`，颗粒度 = 命名色 / 256-color index / hex。

### 3.5.2 移植到 ccline 的设计选择

**选 B（推荐）：分级 ColorConfig + Segment 自决 ThresholdLevel**。其他选项（A 全段统一 / C 渲染层硬编码）已评估并放弃，理由见下表。

| 选项 | 描述 | 优势 | 劣势 | 决策 |
|------|------|------|------|------|
| A | 全局调色板 + 渲染层根据 segment.id 硬编码阈值 | 配置极简 | segment 阈值不可调；与 ccline "每段配色独立" 哲学冲突 | ❌ |
| **B** | **`ColorConfig` 加 `warning`/`critical` 字段；segment 在 `collect()` 计算 `ThresholdLevel`；renderer 根据 level 选色** | **对齐 ccline 现有架构；阈值可配置；零侵入老 segment** | 需改 `ColorConfig` + `SegmentData` schema | ✅ |
| C | 给 segment 加 `dynamic_color` flag，renderer 根据 metadata.percentage 选色 | 不改 collect() | metadata 字段名约定脆弱；类型不安全 | ❌ |

### 3.5.3 数据模型变化（B 方案）

**3.5.3.1** 扩展 `ColorConfig`（`src/config/types.rs`）：

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ColorConfig {
    pub icon: Option<AnsiColor>,
    pub text: Option<AnsiColor>,
    pub background: Option<AnsiColor>,
    // 新增 ↓ 阈值染色（可选，None 时退化为 text 色）
    #[serde(default)]
    pub warning: Option<AnsiColor>,
    #[serde(default)]
    pub critical: Option<AnsiColor>,
}
```

向后兼容性：旧 `config.toml` 没有 `warning` / `critical` 字段，serde 默认 `None`，renderer 行为不变。

**3.5.3.2** 扩展 `SegmentData`（`src/core/segments/mod.rs`）：

```rust
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub enum ThresholdLevel {
    #[default]
    Normal,    // 用 colors.text
    Warning,   // 用 colors.warning，缺省回退 text
    Critical,  // 用 colors.critical，缺省回退 warning，再缺省回退 text
}

#[derive(Debug, Clone)]
pub struct SegmentData {
    pub primary: String,
    pub secondary: String,
    pub metadata: HashMap<String, String>,
    pub level: ThresholdLevel,         // 新增
}
```

老 segment（model/directory/git/cost/...）走 `..Default::default()` 自动得到 `Normal` → 行为零变化。

### 3.5.4 阈值默认值（与 claude-hud 对齐）

| Segment | 度量 | warning | critical | 配置 key | 选色基准（Gruvbox 256） |
|---------|------|:-------:|:--------:|---------|---------------------|
| `context_window` | 上下文占比 % | 70 | 85 | `options.warning_threshold / critical_threshold` | text=142(绿) / warning=214(黄) / critical=167(红) |
| `usage` | 用量配额 % | 75 | 90 | 同上 | text=109(青) / warning=175(粉) / critical=167(红) |
| `cache_hit` | 命中率 %（**反向**：低=坏） | 60↓ | 30↓ | `options.warning_below / critical_below` | text=142(绿，>=60%) / warning=214(黄，30-60%) / critical=167(红，<30%) |
| `since_reply` | 距上次回复秒数 | 300 (5min) | 1800 (30min) | `options.warning_seconds / critical_seconds` | text=223(浅黄) / warning=214 / critical=167 |
| `git`（推送积压） | ahead commit 数 | 5 | 20 | `options.push_warning_threshold / push_critical_threshold`（claude-hud 同名字段） | text=109 / warning=214 / critical=167 |

> **方向性约定**：「越小越坏」的指标（cache_hit 命中率）使用 `warning_below / critical_below`；「越大越坏」（context_window 占比）使用 `warning_threshold / critical_threshold`。命名一致性 > 通用性。

### 3.5.5 渲染层改造（`src/core/statusline.rs::render_segment`）

```rust
fn pick_text_color<'a>(config: &'a SegmentConfig, level: ThresholdLevel) -> Option<&'a AnsiColor> {
    match level {
        ThresholdLevel::Normal   => config.colors.text.as_ref(),
        ThresholdLevel::Warning  => config.colors.warning.as_ref()
                                     .or(config.colors.text.as_ref()),
        ThresholdLevel::Critical => config.colors.critical.as_ref()
                                     .or(config.colors.warning.as_ref())
                                     .or(config.colors.text.as_ref()),
    }
}

fn pick_icon_color<'a>(config: &'a SegmentConfig, level: ThresholdLevel) -> Option<&'a AnsiColor> {
    // icon 默认随 text 一起染色（视觉聚焦）。如需 icon 始终静态，可后续加选项分离
    pick_text_color(config, level)
        .or(config.colors.icon.as_ref())
}
```

`render_segment` 内部把 `apply_color(&icon, config.colors.icon.as_ref())` / `apply_style(...)` 中的 `config.colors.text.as_ref()` 替换为 `pick_text_color(config, data.level)`，`apply_color(&icon, ...)` 替换为 `pick_icon_color(...)`。

**回退链**：critical → warning → text → 无色，确保任何配置层级都有合理默认。

### 3.5.6 视觉效果（colorized）

```
低占比（绿色平静）：
⚡️ 23% · 47k    （c256=142 绿）

中占比（黄色提示）：
⚡️ 72% · 144k   （c256=214 黄）

高占比（红色警告）：
⚡️ 88% · 176k   （c256=167 红）

cache_hit 反向（命中率低=坏）：
 25% 12k/47k   红色 — 命中率太低，cache 没有被复用
 65% 30k/47k   绿色 — 健康
```

**视觉语义一致性**：所有 segment 的 critical 颜色统一用 c256=167（Gruvbox red），warning 统一用 c256=214（Gruvbox yellow），形成"颜色 = 严重程度"的全局认知锚点。

---

## 四、实施步骤（Step-by-step）

### Step 1：扩展类型层（`src/config/types.rs` + `src/core/segments/mod.rs`）

**改动 1.0（v2 新增）**：`ColorConfig` 增加 `warning` / `critical` 可选字段；`SegmentData` 增加 `level: ThresholdLevel`，并定义 enum。详见 §三.5.3。

**改动 1.1**：`TranscriptEntry` 新增 2 个字段（serde 默认 `None` 保证向后兼容）。

```rust
#[derive(Deserialize)]
pub struct TranscriptEntry {
    pub r#type: Option<String>,
    pub message: Option<Message>,
    #[serde(rename = "leafUuid")]
    pub leaf_uuid: Option<String>,
    pub uuid: Option<String>,
    #[serde(rename = "parentUuid")]
    pub parent_uuid: Option<String>,
    pub summary: Option<String>,
    // 新增 ↓
    pub timestamp: Option<String>,                  // ISO-8601 UTC, e.g. "2026-04-29T09:07:15.987Z"
    #[serde(rename = "isSidechain", default)]
    pub is_sidechain: Option<bool>,                 // 子代理（agent）入口默认排除
}
```

**改动 1.2**：新增 `SessionUsageStats` 结构体（导出至 `crate::config`）。

```rust
#[derive(Debug, Clone, Default)]
pub struct SessionUsageStats {
    /// 全会话累计（不含 sidechain，可选项控制）
    pub totals: NormalizedUsage,
    /// 最近一轮（最末 assistant 消息的 usage）
    pub last_turn: NormalizedUsage,
    /// 最近一轮 assistant 的 timestamp（UTC）
    pub last_assistant_at: Option<chrono::DateTime<chrono::Utc>>,
    /// 累计 assistant 轮次数（用于命中率分母调试）
    pub assistant_count: u32,
}
```

**改动 1.3**：`enum SegmentId` 新增 2 个变体。

```rust
pub enum SegmentId {
    Model, Directory, Git, ContextWindow, Usage, Cost,
    Session, OutputStyle, Update, CacheHit,
    SinceReply,        // 新增
    SessionTokens,     // 新增
}
```

预期产物：编译通过；`SegmentConfig` 反序列化兼容旧 `config.toml`（新 ID 出现在 TOML 才会启用）。

---

### Step 2：扩展数据层（`src/core/transcript.rs`）

**核心：单次前向扫描，同时累计与捕获 last-turn**。性能预算：典型 JSONL 100KB-10MB，serde_json 行级解析约 5-50µs/行 × 1000 行 ≈ 5-50ms。**风险：极长会话（>5MB）可能突破 5ms 预算**——见 §6 缓解策略。

```rust
pub fn parse_session_stats<P: AsRef<Path>>(transcript_path: P) -> Option<SessionUsageStats> {
    // 1. 解析路径，回退到 project 历史最新 jsonl（沿用现有 fallback）
    let path = resolve_active_transcript(transcript_path.as_ref())?;

    let file = fs::File::open(&path).ok()?;
    let reader = BufReader::new(file);

    let mut stats = SessionUsageStats::default();
    let mut had_any = false;

    for line in reader.lines().flatten() {
        let trimmed = line.trim();
        if trimmed.is_empty() { continue; }

        let entry = match serde_json::from_str::<TranscriptEntry>(trimmed) {
            Ok(e) => e,
            Err(_) => continue,                                  // 容错：跳过坏行
        };

        // 默认排除 sidechain（子代理）轮次。后续可加 config 选项放开
        if entry.is_sidechain.unwrap_or(false) { continue; }

        if entry.r#type.as_deref() != Some("assistant") { continue; }

        let message = match entry.message { Some(m) => m, None => continue };
        let raw = match message.usage { Some(u) => u, None => continue };
        let normalized = raw.normalize();

        // 累计：累加各 token 字段（不重新规整 calculation_source/raw_data_available）
        accumulate_into(&mut stats.totals, &normalized);

        // 覆写：last_turn 始终保留最后一次写入
        stats.last_turn = normalized;
        stats.assistant_count += 1;
        had_any = true;

        if let Some(ts) = entry.timestamp.as_deref() {
            if let Ok(dt) = chrono::DateTime::parse_from_rfc3339(ts) {
                stats.last_assistant_at = Some(dt.with_timezone(&chrono::Utc));
            }
        }
    }

    if had_any { Some(stats) } else { None }
}

fn accumulate_into(acc: &mut NormalizedUsage, turn: &NormalizedUsage) {
    acc.input_tokens               = acc.input_tokens.saturating_add(turn.input_tokens);
    acc.output_tokens              = acc.output_tokens.saturating_add(turn.output_tokens);
    acc.total_tokens               = acc.total_tokens.saturating_add(turn.total_tokens);
    acc.cache_creation_input_tokens = acc.cache_creation_input_tokens.saturating_add(turn.cache_creation_input_tokens);
    acc.cache_read_input_tokens    = acc.cache_read_input_tokens.saturating_add(turn.cache_read_input_tokens);
    acc.cache_creation_5m          = acc.cache_creation_5m.saturating_add(turn.cache_creation_5m);
    acc.cache_creation_1h          = acc.cache_creation_1h.saturating_add(turn.cache_creation_1h);
    // calculation_source / raw_data_available 不累计（语义无效）
}

// 兼容包装：保留旧 API
pub fn parse_last_assistant_usage<P: AsRef<Path>>(p: P) -> Option<NormalizedUsage> {
    parse_session_stats(p).map(|s| s.last_turn)
}
```

**关键设计决策**：

1. **sidechain 默认排除**：子代理（Task/Agent 调用）的 token 不应混入主会话累计；保留扩展点（未来可加 `include_sidechain: bool` 选项）。
2. **summary leafUuid 跳跃不再需要特殊处理**：`parse_last_assistant_usage` 老逻辑中跳到 leafUuid 的快路径，在前向扫描中天然不需要——文件被完整走完。极端情况：summary 后还有断 transcript，行为与原版一致（取最后实际 assistant 行）。
3. **解析 timestamp 用 `parse_from_rfc3339`**：兼容 ISO-8601 with offset/Zulu。
4. **错误恢复**：单行 JSON 解析失败仅 `continue`，不污染整体结果（与现有 `try_parse_transcript_file` 容错策略一致）。

---

### Step 3：新增 `since_reply` segment（`src/core/segments/since_reply.rs`，新建）

```rust
use super::{Segment, SegmentData, ThresholdLevel};
use crate::config::{InputData, SegmentId};
use crate::core::transcript;
use std::collections::HashMap;

#[derive(Default)]
pub struct SinceReplySegment;

impl SinceReplySegment {
    pub fn new() -> Self { Self }

    /// 复用 session.rs 的相对时间格式（s/m/h），但用秒粒度更新更频繁
    fn format_elapsed(seconds: i64) -> String {
        let s = seconds.max(0);                      // 时钟回拨保护
        match s {
            0..=59          => format!("{}s", s),
            60..=3599       => format!("{}m {}s", s / 60, s % 60),
            3600..=86_399   => format!("{}h {}m", s / 3600, (s % 3600) / 60),
            _               => format!("{}d", s / 86_400),
        }
    }

    /// 阈值染色：>1800s 红，>300s 黄（与 §三.5.4 默认值一致；可经 options 覆盖）
    fn classify(seconds: i64, warning: i64, critical: i64) -> ThresholdLevel {
        if seconds >= critical { ThresholdLevel::Critical }
        else if seconds >= warning { ThresholdLevel::Warning }
        else { ThresholdLevel::Normal }
    }
}

impl Segment for SinceReplySegment {
    fn collect(&self, input: &InputData) -> Option<SegmentData> {
        let stats = transcript::parse_session_stats(&input.transcript_path)?;
        let last_at = stats.last_assistant_at?;

        let now = chrono::Utc::now();
        let elapsed = (now - last_at).num_seconds().max(0);

        // TODO: 从 SegmentConfig.options 读 warning_seconds / critical_seconds，
        // 默认 300 / 1800。此处先 hardcode，Step 7 把 options 解析抽出为统一帮助函数
        let level = Self::classify(elapsed, 300, 1800);

        let primary = Self::format_elapsed(elapsed);

        let mut metadata = HashMap::new();
        metadata.insert("seconds".to_string(), elapsed.to_string());
        metadata.insert("last_assistant_at".to_string(), last_at.to_rfc3339());

        Some(SegmentData {
            primary,
            secondary: String::new(),
            metadata,
            level,
        })
    }

    fn id(&self) -> SegmentId { SegmentId::SinceReply }
}
```

**视觉设计**：
- icon plain `⏳`，nerd_font `\u{f43a}` (nf-md-clock_outline)
- color c256 `223`（Gruvbox light yellow，与 cache_hit/cost 拉开层级，区隔"时间敏感"语义）
- 默认 `enabled = false`

---

### Step 4：新增 `session_tokens` segment（`src/core/segments/session_tokens.rs`，新建）

**显示语义**：`{last}|{total}`，对齐 claude-hud 风格。

```rust
use super::{Segment, SegmentData};
use crate::config::{InputData, SegmentId};
use crate::core::transcript;
use std::collections::HashMap;

#[derive(Default)]
pub struct SessionTokensSegment;

impl SessionTokensSegment {
    pub fn new() -> Self { Self }

    fn format_tokens(value: u32) -> String {
        if value < 1000 { return value.to_string(); }
        let k = value as f64 / 1000.0;
        if k >= 1000.0 {
            let m = k / 1000.0;
            return format!("{:.1}M", m);             // 1.2M（大写区分 milli）
        }
        if k.fract() == 0.0 { format!("{}k", k as u32) } else { format!("{:.1}k", k) }
    }
}

impl Segment for SessionTokensSegment {
    fn collect(&self, input: &InputData) -> Option<SegmentData> {
        let stats = transcript::parse_session_stats(&input.transcript_path)?;

        let in_pair = format!(
            "{}|{}",
            Self::format_tokens(stats.last_turn.input_tokens),
            Self::format_tokens(stats.totals.input_tokens),
        );
        let out_pair = format!(
            "{}|{}",
            Self::format_tokens(stats.last_turn.output_tokens),
            Self::format_tokens(stats.totals.output_tokens),
        );
        let cache_pair = format!(
            "{}|{}",
            Self::format_tokens(stats.last_turn.cache_read_input_tokens),
            Self::format_tokens(stats.totals.cache_read_input_tokens),
        );

        let primary = format!("in {} · out {} · cache {}", in_pair, out_pair, cache_pair);

        let mut metadata = HashMap::new();
        metadata.insert("turns".to_string(), stats.assistant_count.to_string());
        metadata.insert("total_input".to_string(), stats.totals.input_tokens.to_string());
        metadata.insert("total_output".to_string(), stats.totals.output_tokens.to_string());
        metadata.insert("total_cache_read".to_string(), stats.totals.cache_read_input_tokens.to_string());

        Some(SegmentData { primary, secondary: String::new(), metadata })
    }

    fn id(&self) -> SegmentId { SegmentId::SessionTokens }
}
```

**视觉设计**：
- icon plain `Σ`，nerd_font `\u{f1c0}` (nf-fa-database)
- color c256 `175`（Gruvbox pink，与 model 橙/cache_hit 紫拉开）
- 默认 `enabled = false`

---

### Step 4.5：`session_tokens` 阈值染色（v2 补充）

`session_tokens` 显示的是绝对值（k|k 形式），不天然适合 % 阈值。**保持 `level = Normal`**（不参与阈值染色），单纯依赖 `colors.text` 静态色。理由：避免视觉过载——三个 segment（context_window / cache_hit / since_reply）已染色，再加 session_tokens 会让状态行变成红绿灯。如未来需要可基于 `total_input vs context_limit × turns` 自定义指标。

### Step 5：升级 `cache_hit` segment（`src/core/segments/cache_hit.rs`，修改 ~30 LOC）

**新增配置项**：`mode` 选项，三档：

| `mode` | primary 显示 | 数据源 |
|--------|------------|-------|
| `"last"`（默认，向后兼容） | `78%`（保持现状） | last_turn |
| `"total"` | `64%` | totals 累计 |
| `"both"` | `78%/64%` | 两者（last/total） |

```rust
impl Segment for CacheHitSegment {
    fn collect(&self, input: &InputData) -> Option<SegmentData> {
        let stats = transcript::parse_session_stats(&input.transcript_path)?;
        let mode = self.mode_or_default();           // 从 SegmentConfig.options.get("mode")

        let last_pct = compute_hit_rate(&stats.last_turn);
        let total_pct = compute_hit_rate(&stats.totals);

        let primary = match mode.as_str() {
            "total" => format_pct(total_pct),
            "both"  => format!("{}/{}", format_pct(last_pct), format_pct(total_pct)),
            _       => format_pct(last_pct),         // last（默认）
        };

        // secondary 保持原 TTL + last_turn 分子分母逻辑（向后兼容）
        let ttl = ttl_label(&stats.last_turn);
        let secondary = build_secondary(&stats.last_turn, ttl);

        // metadata 同时暴露 last 和 total 供调试
        let mut metadata = HashMap::new();
        metadata.insert("hit_rate_last".to_string(),  format!("{:.2}", last_pct.unwrap_or(-1.0)));
        metadata.insert("hit_rate_total".to_string(), format!("{:.2}", total_pct.unwrap_or(-1.0)));
        metadata.insert("mode".to_string(),           mode);
        metadata.insert("turns".to_string(),          stats.assistant_count.to_string());
        metadata.insert("ttl".to_string(),            ttl.to_string());

        Some(SegmentData { primary, secondary, metadata })
    }
    // ...
}

fn compute_hit_rate(u: &NormalizedUsage) -> Option<f64> {
    let denom = u.input_tokens + u.cache_read_input_tokens + u.cache_creation_input_tokens;
    if denom == 0 { return None; }
    Some((u.cache_read_input_tokens as f64 / denom as f64) * 100.0)
}

fn format_pct(v: Option<f64>) -> String {
    match v {
        Some(p) if p.fract() == 0.0 => format!("{:.0}%", p),
        Some(p)                     => format!("{:.1}%", p),
        None                        => "-".to_string(),
    }
}
```

**Gap C 推荐选择**：默认 `last`（零行为变更），椰椰可在 TOML 中改成 `both` 一次看清两者。**理由**：单 ANSI 单元密度受限，`78%/64%` 7 字符无副作用；`total` 单独使用时不知道当前轮 anomaly。`both` 是最大信息密度且仍 ≤ 8 字符。

**v2 阈值染色**：cache_hit 是**反向指标**（命中率越高越好），所以阈值方向反转：
```rust
// 取 mode=both 时用 total，否则用 last 作为染色依据
let pct_for_color = match mode {
    "both" | "total" => total_pct,
    _                => last_pct,
};
let level = match pct_for_color {
    Some(p) if p < critical_below => ThresholdLevel::Critical,  // 默认 30
    Some(p) if p < warning_below  => ThresholdLevel::Warning,   // 默认 60
    _                             => ThresholdLevel::Normal,
};
```

---

### Step 6：注册 4 处触点

**6.1** `src/core/segments/mod.rs`：

```rust
pub mod cache_hit;
pub mod context_window;
pub mod cost;
pub mod directory;
pub mod git;
pub mod model;
pub mod output_style;
pub mod session;
pub mod since_reply;        // 新增
pub mod session_tokens;     // 新增
pub mod update;
pub mod usage;

pub use cache_hit::CacheHitSegment;
// ... 其他原有 ...
pub use since_reply::SinceReplySegment;
pub use session_tokens::SessionTokensSegment;
```

**6.2** `src/core/statusline.rs::collect_all_segments`：

```rust
crate::config::SegmentId::SinceReply => {
    SinceReplySegment::new().collect(input)
}
crate::config::SegmentId::SessionTokens => {
    SessionTokensSegment::new().collect(input)
}
```

**6.3** `src/ui/components/segment_list.rs`：

```rust
SegmentId::SinceReply    => "Since Reply",
SegmentId::SessionTokens => "Session Tokens",
```

**6.4** `src/ui/themes/theme_gruvbox.rs`（可选——为 Gruvbox 主题加默认配置；用户也可手动加 TOML）。**v2 同步给 context_window / usage / cache_hit / since_reply / git 默认配置补 warning/critical 颜色**（默认值见 §三.5.4 表格，c256: 214 黄 / 167 红）：

```rust
pub fn since_reply_segment() -> SegmentConfig {
    SegmentConfig {
        id: SegmentId::SinceReply,
        enabled: false,                              // 默认关闭
        icon: IconConfig {
            plain: "⏳".to_string(),
            nerd_font: "\u{f43a}".to_string(),
        },
        colors: ColorConfig {
            icon: Some(AnsiColor::Color256 { c256: 142 }),  // 绿（< 5min 平静）
            text: Some(AnsiColor::Color256 { c256: 142 }),
            background: None,
            warning:  Some(AnsiColor::Color256 { c256: 214 }), // 黄 5-30min
            critical: Some(AnsiColor::Color256 { c256: 167 }), // 红 ≥ 30min
        },
        styles: TextStyleConfig { text_bold: false },
        options: HashMap::new(),
    }
}

pub fn session_tokens_segment() -> SegmentConfig {
    SegmentConfig {
        id: SegmentId::SessionTokens,
        enabled: false,
        icon: IconConfig {
            plain: "Σ".to_string(),
            nerd_font: "\u{f1c0}".to_string(),
        },
        colors: ColorConfig {
            icon: Some(AnsiColor::Color256 { c256: 175 }),
            text: Some(AnsiColor::Color256 { c256: 175 }),
            background: None,
            warning:  None,                           // 不参与阈值染色（见 Step 4.5）
            critical: None,
        },
        styles: TextStyleConfig { text_bold: false },
        options: HashMap::new(),
    }
}
```

**对老 segment 的同步补色**（同文件，已存在的函数追加字段）：

| 函数 | 新增字段 |
|------|---------|
| `context_window_segment()` | `warning: c256=214`, `critical: c256=167` |
| `usage_segment()` | `warning: c256=175`, `critical: c256=167` |
| `cache_hit_segment()` | `warning: c256=214`, `critical: c256=167` |
| `git_segment()` | `warning: c256=214`, `critical: c256=167`（针对 ahead 阈值） |
| 其余（model/directory/cost/session/output_style/update） | `warning: None, critical: None`（不参与阈值染色） |

---

### 三.6、claude-hud 多行布局（lineLayout）机制移植（v3 新增）

#### 3.6.1 claude-hud 现状（已 Read 源码 `~/.claude/plugins/cache/claude-hud/.../src/render/index.ts`）

```typescript
// render/index.ts:485-545
function render(ctx) {
  const lineLayout = ctx.config?.lineLayout ?? 'expanded';
  if (lineLayout === 'expanded') {
    // 每个 element 一行，按 elementOrder 顺序
    // mergeGroups 中相邻 element 在同一行用 ' │ ' 拼接
    // 拼接后超宽 → 拆回单行
    const renderedLines = renderExpanded(ctx, terminalWidth);
    // ...
  } else {
    // compact 模式：renderSessionLine 出单行 header
    const headerLines = renderCompact(ctx);
  }
  // 最终对每行做 wrapLineToWidth 兜底
  for (const line of visibleLines) console.log(`${RESET}${line}`);
}
```

**关键机制**：
1. `elementOrder: HudElement[]` — 元素从上到下顺序
2. `mergeGroups: HudElement[][]` — 同行合并组（如 `[["context","promptCache"]]`）
3. 合并优先：若组内多元素相邻则拼接为一行；超宽度则**降级为多行**
4. 最终再次按 `wrapLineToWidth` 对每行做软换行兜底

#### 3.6.2 移植到 ccline 的设计

**新增顶层配置块** `[layout]`（`src/config/types.rs`）：

```rust
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct LayoutConfig {
    /// "compact"（默认，单行）| "expanded"（每段一行）
    #[serde(default = "default_layout_mode")]
    pub mode: LayoutMode,
    /// expanded 模式下，将列出的 segment id 组同行合并；为空表示无合并
    /// 例：[["context_window", "cache_hit"], ["since_reply"]]
    #[serde(default)]
    pub merge_groups: Vec<Vec<SegmentId>>,
    /// 超过终端宽度时是否自动拆回单行（默认 true）
    #[serde(default = "default_true")]
    pub auto_stack_on_overflow: bool,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Default)]
#[serde(rename_all = "snake_case")]
pub enum LayoutMode {
    #[default]
    Compact,             // 现状：所有 segment 单行 + 分隔符
    Expanded,            // 新增：默认每段一行，merge_groups 同行
}

fn default_layout_mode() -> LayoutMode { LayoutMode::Compact }
fn default_true() -> bool { true }
```

**Config 顶层加字段**：

```rust
pub struct Config {
    pub style: StyleConfig,
    pub segments: Vec<SegmentConfig>,
    pub theme: String,
    #[serde(default)]
    pub layout: LayoutConfig,           // 新增
}
```

**向后兼容**：旧 TOML 没有 `[layout]` 块 → `LayoutConfig::default()` → `mode = Compact` → 与 v1.1.2 单行行为完全一致。

#### 3.6.3 关键设计决策

| 维度 | 选择 | 理由 |
|------|------|------|
| 默认模式 | `compact` | 不破坏老用户体验；椰椰新增 `mode = "expanded"` 才启用 |
| **不复用 claude-hud `elementOrder`** | 沿用 ccline 现有 `[[segments]]` 数组顺序 | ccline 的 segment 顺序信息已经在配置数组里，再加 `elementOrder` 是冗余；遵循"单一真相源" |
| `merge_groups` 用 `SegmentId` 而非任意字符串 | enum 类型安全 | TOML 里写错的 segment id 会反序列化失败，提早暴露 |
| 自动拆回逻辑 | 与 claude-hud 一致：merged 超宽 → 按组内段拆成多行 | 行为可预测；用户在窄终端无需重配 |
| 终端宽度获取 | `crossterm::terminal::size()` （已是依赖） + `$COLUMNS` env 兜底 | 与 ccline UI 模块一致 |
| 输出格式 | `generate()` 改返回 `Vec<String>`；`main.rs` 用 `\n` join 输出 | Claude Code 状态行支持多行（claude-hud 同样靠 `\n`） |

#### 3.6.4 渲染流程伪代码

```rust
fn generate(&self, segments: Vec<(SegmentConfig, SegmentData)>) -> Vec<String> {
    let enabled: Vec<_> = segments.into_iter().filter(|(c, _)| c.enabled).collect();
    if enabled.is_empty() { return vec![]; }

    match self.config.layout.mode {
        LayoutMode::Compact => {
            // 老路径：所有段拼一行（保留 Powerline / 普通分隔符逻辑）
            vec![self.render_single_line(&enabled)]
        }
        LayoutMode::Expanded => {
            self.render_expanded(&enabled)
        }
    }
}

fn render_expanded(&self, enabled: &[(SegmentConfig, SegmentData)]) -> Vec<String> {
    let merge_lookup = build_merge_lookup(&self.config.layout.merge_groups);
    let term_width = detect_terminal_width().unwrap_or(usize::MAX);
    let auto_stack = self.config.layout.auto_stack_on_overflow;

    let mut lines = Vec::<String>::new();
    let mut i = 0;
    while i < enabled.len() {
        let id_i = enabled[i].0.id;
        if let Some(group) = merge_lookup.get(&id_i) {
            // 收集相邻同组段
            let mut consumed = vec![i];
            let mut j = i + 1;
            while j < enabled.len() && group.contains(&enabled[j].0.id) {
                consumed.push(j);
                j += 1;
            }

            if consumed.len() > 1 {
                let merged = self.render_segments_joined(
                    &consumed.iter().map(|&k| &enabled[k]).collect::<Vec<_>>()
                );
                if !auto_stack || visible_width(&merged) <= term_width {
                    lines.push(merged);
                } else {
                    // 拆回：每段独占一行
                    for &k in &consumed {
                        lines.push(self.render_segment(&enabled[k].0, &enabled[k].1));
                    }
                }
                i = j;
                continue;
            }
        }

        // 不在合并组内 → 每段独占一行
        lines.push(self.render_segment(&enabled[i].0, &enabled[i].1));
        i += 1;
    }
    lines
}
```

**主入口 `main.rs` 输出方式**：

```rust
// before: println!("{}", generator.generate(segments));
// after:
for line in generator.generate(segments) {
    println!("{}", line);
}
```

#### 3.6.5 配置示例（椰椰可直接采用）

```toml
[layout]
mode = "expanded"
auto_stack_on_overflow = true

# 紧凑头部组（model + dir + git 同行）
[[layout.merge_groups]]
ids = ["model", "directory", "git"]

# 状态指标组（context + cache_hit 同行——claude-hud 同款）
[[layout.merge_groups]]
ids = ["context_window", "cache_hit"]

# 时间/token 总结组
[[layout.merge_groups]]
ids = ["since_reply", "session_tokens"]

# cost 与 session 单独成行（不进 merge_groups 即默认单段一行）
```

> 这里的 TOML 嵌套语法用 `[[layout.merge_groups]]` + `ids = [...]` 是因为 toml 数组的数组写法繁琐；为此 `LayoutConfig.merge_groups` 类型从 `Vec<Vec<SegmentId>>` 改为 `Vec<MergeGroup { ids: Vec<SegmentId> }>` 更友好。**实施时按 ergonomic 选择**。

#### 3.6.6 行长均衡算法（v3.1 新增）

##### 3.6.6.1 问题描述

claude-hud 没解决，但 ccline 必须解决的边角问题：

```
v3 expanded 模式下，segment 颗粒度比 claude-hud element 细，
易出现「极短行落单」尴尬：

⚡️ 72% · 144k │  78% 47k/60k [5m+1h]   ← 30+ 字符
⏳ 2m 14s                                 ← 9 字符 ❌ 太短
Σ in 12k|142k · out 8k|34k · cache 122k  ← 40+ 字符
$0.42                                     ← 5 字符 ❌ 太短
12m                                       ← 3 字符 ❌ 太短
```

##### 3.6.6.2 配置扩展

```rust
pub struct LayoutConfig {
    pub mode: LayoutMode,
    pub merge_groups: Vec<MergeGroup>,
    pub auto_stack_on_overflow: bool,            // v3 已有
    // v3.1 新增 ↓
    #[serde(default = "default_true")]
    pub balance: bool,                           // 默认 true（仅 expanded 生效）
    #[serde(default = "default_min_row_width")]
    pub min_row_width: usize,                    // 默认 20 字符；< 此宽度的行尝试合并
    #[serde(default)]
    pub target_row_width: Option<usize>,         // None → 自动 = 0.7 × term_width
}

fn default_min_row_width() -> usize { 20 }
```

##### 3.6.6.3 算法设计（贪心 + 多遍迭代）

**输入**：`rows: Vec<String>`（已经 mergeGroups 处理过的行集合）、`term_width`、`min_row_width`、`target_row_width`

**输出**：均衡后的 `Vec<String>`

**伪代码**：

```rust
fn balance_rows(
    mut rows: Vec<String>,
    term_width: usize,
    min_row_width: usize,
    target_row_width: usize,
    separator: &str,
) -> Vec<String> {
    const MAX_PASSES: usize = 5;          // 防退化
    let cap = term_width.min(target_row_width.max(min_row_width));

    for _ in 0..MAX_PASSES {
        // 找到最短行
        let (idx, width) = rows.iter()
            .enumerate()
            .map(|(i, r)| (i, visible_width(r)))
            .min_by_key(|(_, w)| *w)
            .unwrap();

        if width >= min_row_width { break; }   // 全部行已达标

        // 选邻居：取较窄的一侧（视觉更平衡）
        let prev_w = if idx > 0 { Some(visible_width(&rows[idx - 1])) } else { None };
        let next_w = if idx + 1 < rows.len() { Some(visible_width(&rows[idx + 1])) } else { None };

        let merge_dir = match (prev_w, next_w) {
            (Some(pw), Some(nw)) if pw <= nw => Direction::Up,
            (Some(_),  Some(_))               => Direction::Down,
            (Some(_),  None)                  => Direction::Up,
            (None,     Some(_))               => Direction::Down,
            (None,     None)                  => break,        // 单行无邻居
        };

        // 合并校验：合并后不能超 cap
        let combined_width = match merge_dir {
            Direction::Up   => prev_w.unwrap() + visible_width(separator) + width,
            Direction::Down => width + visible_width(separator) + next_w.unwrap(),
        };
        if combined_width > cap { break; }    // 合不进去，认输

        // 执行合并
        match merge_dir {
            Direction::Up => {
                let merged = format!("{}{}{}", rows[idx - 1], separator, rows[idx]);
                rows[idx - 1] = merged;
                rows.remove(idx);
            }
            Direction::Down => {
                let merged = format!("{}{}{}", rows[idx], separator, rows[idx + 1]);
                rows[idx] = merged;
                rows.remove(idx + 1);
            }
        }
    }

    rows
}
```

##### 3.6.6.4 关键设计决策

| 维度 | 选择 | 理由 |
|------|------|------|
| **何时执行** | mergeGroups 之后、wrap 之前 | 用户显式 `merge_groups` 是硬约束，均衡是兜底 |
| **均衡 cap** | `min(term_width, target_row_width)` | 不超出终端可视，又不超出用户期望的目标 |
| **邻居选择策略** | 取较窄的一侧 | 让总体方差减小，避免把短行并入已经最长的行 |
| **失败兜底** | `MAX_PASSES = 5` 或 cap 超限 → 直接返回当前状态 | 不死循环；尊重物理约束 |
| **不合并跨"硬合并组"** | 已经合并组成的多段行视为单元，可整体作为邻居 | 用户的 mergeGroups 永远是硬规则 |
| **Compact 模式跳过** | `if mode == Compact return rows;` | 单行布局无需均衡 |

##### 3.6.6.5 算法行为示例

**场景：5 行未均衡（`term_width=120`, `min_row_width=20`, `target_row_width=84`）**

```
输入（mergeGroups 已处理）：
  Row 0: ⚡️ 72% · 144k │  78% 47k/60k [5m+1h]    (33 chars)
  Row 1: ⏳ 2m 14s                                  (9 chars)  ← 最短
  Row 2: Σ in 12k|142k · out 8k|34k · cache 122k|1.2M (44 chars)
  Row 3: $0.42                                      (5 chars)
  Row 4: 12m                                        (3 chars)

第 1 遍：
  最短 = Row 4 (3 chars) < 20
  邻居：Row 3 (5 chars) [无下邻居]
  合并 Row 4 → Row 3：
  Row 3: $0.42 │ 12m   (10 chars)

第 2 遍：
  最短 = Row 3 (10 chars) < 20
  邻居：Row 2 (44) [无下邻居]
  合并 Row 3 → Row 2：
  Row 2: Σ in 12k|142k · out 8k|34k · cache 122k|1.2M │ $0.42 │ 12m   (57 chars) ✓

第 3 遍：
  最短 = Row 1 (9 chars) < 20
  邻居：Row 0 (33), Row 2 (57)
  选窄邻 Row 0 → 合并：
  Row 0: ⚡️ 72% · 144k │  78% 47k/60k [5m+1h] │ ⏳ 2m 14s   (44 chars) ✓

第 4 遍：
  最短 = Row 0 (44 chars) ≥ 20 ✓
  退出

输出：
  Row 0: ⚡️ 72% · 144k │  78% 47k/60k [5m+1h] │ ⏳ 2m 14s   (44 chars)
  Row 1: 🤖 Opus 4.7 │ 📁 ccline │ 🌿 main*                 (mergeGroups 硬合并)
  Row 2: Σ in 12k|142k · out 8k|34k · cache 122k|1.2M │ $0.42 │ 12m   (57 chars)
```

> **闭环效果**：行数从 5 → 3，最短行从 3 → 44，行长方差大幅下降。

##### 3.6.6.6 边界条件清单

| 边界 | 行为 |
|------|------|
| 仅 1 行 | 跳过均衡（无邻居） |
| 全部行 ≥ min_row_width | 第一遍即退出 |
| 终端宽度未知（stdout 非 tty） | `term_width = usize::MAX`，目标取 `target_row_width.unwrap_or(80)`，不冒进 |
| 用户显式 `balance = false` | 完全跳过均衡步骤 |
| `min_row_width = 0` | 退化为不均衡（全部行视为已达标） |
| 合并后超 cap | 当前最短行保持原状，但已合并过的不回滚（贪心可接受次优） |

---

### Step 7：渲染层阈值染色装配（v2 新增，`src/core/statusline.rs`，~30 LOC）

**核心**：在 `render_segment` 内部把 `apply_color`/`apply_style` 的 color 参数源由静态 `config.colors.text` 改为动态 `pick_text_color(config, data.level)`（见 §三.5.5）。

**改动颗粒度**：

1. 新增 free function `pick_text_color` / `pick_icon_color`（~16 LOC）
2. `render_segment` 内 4 处颜色取值替换（icon / primary text / secondary text / 在背景色分支内重复）（~8 LOC）
3. **不动** `apply_color` / `apply_style` / `apply_background_color` 本身（保持纯函数）
4. **不动** Powerline 箭头颜色逻辑（`create_powerline_arrow` 用 `colors.background`，与阈值无关）

**单元测试**（在 `statusline.rs` 内联或建 `statusline_tests.rs`）：
- `pick_text_color` 缺省回退链：critical 缺 → warning，warning 缺 → text，text 缺 → None
- `pick_icon_color` 走相同回退链
- 端到端：`generate(...)` 输出包含 `\x1b[38;5;167m`（红）当 level=Critical

**Snapshot 测试**（`tests/render_threshold.rs`）：
对 context_window 在 percent ∈ {30, 75, 90} 三个值下 generate 一次，断言输出包含的 ANSI 色码分别为 c256:142 / 214 / 167。

---

### Step 8：渲染层多行布局装配（v3 新增，`src/core/statusline.rs` + `src/main.rs`，~80 LOC）

**核心改动**：

1. **`src/config/types.rs`**：`Config` 加 `layout: LayoutConfig` 字段；新增 `LayoutConfig` / `LayoutMode` / `MergeGroup` 类型（详见 §三.6.2）。
2. **`src/core/statusline.rs`**：
   - `generate()` 返回类型从 `String` 改为 `Vec<String>`
   - 抽出 `render_single_line(&enabled)` 包装老逻辑（Powerline / white separator）
   - 新增 `render_expanded(&enabled)` 处理 expanded + merge_groups + 宽度回退
   - 新增 `detect_terminal_width()` 帮助函数（先 `crossterm::terminal::size()`，回退 `$COLUMNS` env，再回退 `usize::MAX`）
   - `build_merge_lookup(merge_groups) -> HashMap<SegmentId, &Vec<SegmentId>>` O(1) 查询
3. **`src/main.rs`**：输出方式从单 `println!` 改为循环输出。
4. **`src/ui/components/preview.rs`**（如果存在 TUI 实时预览）：同步改为消费 `Vec<String>`，每行单独渲染（保留现有 `generate_for_tui_preview` 的细粒度宽度处理逻辑）。
5. **TUI 编辑器中的布局编辑**（YAGNI 边界——本计划不实现）：用户用 TOML 直接编辑 `[layout]`。

**单元测试**：

| 用例 | 预期 |
|------|------|
| `test_compact_returns_single_line` | LayoutMode::Compact → `Vec` 长度 = 1 |
| `test_expanded_one_per_segment` | 5 个 enabled segment + 无 merge → `Vec` 长度 = 5 |
| `test_expanded_merge_two_segments` | merge_groups=[[a,b]] → a 与 b 在同行（` │ ` 拼接）|
| `test_expanded_merge_overflow_stacks_back` | term_width=40 + 合并行 width=80 + auto_stack=true → 拆回 2 行 |
| `test_expanded_merge_overflow_no_stack_when_disabled` | 同上但 auto_stack=false → 1 长行（即使超宽） |
| `test_expanded_disabled_segment_skipped` | segment.enabled=false 不出现在任何行 |
| `test_layout_default_is_compact` | TOML 无 `[layout]` → 默认 Compact，行为与 v1.1.2 一致 |

**端到端**（`tests/render_layout.rs`）：

- 加载示例 TOML（compact mode）→ generate → 唯一字符串与 v1.1.2 byte-equal
- 加载示例 TOML（expanded + merge_groups）→ generate → 行数与预期一致 + 每行 ANSI 拼接合规

---

### Step 8.1：行长均衡 pass（v3.1 新增，`src/core/statusline.rs`，~50 LOC）

**位置**：`render_expanded` 内部、wrap_to_width 之前。

```rust
fn render_expanded(&self, enabled: &[(SegmentConfig, SegmentData)]) -> Vec<String> {
    let term_width = detect_terminal_width().unwrap_or(usize::MAX);

    // 阶段 1：mergeGroups 硬合并
    let mut rows = self.expand_with_merge_groups(enabled, term_width);

    // 阶段 2（v3.1）：行长均衡兜底
    if self.config.layout.balance && rows.len() > 1 {
        let target = self.config.layout.target_row_width
            .unwrap_or((term_width as f64 * 0.7) as usize);
        rows = balance_rows(
            rows,
            term_width,
            self.config.layout.min_row_width,
            target,
            &self.separator(),
        );
    }

    rows
}
```

**单元测试**（`statusline.rs` 内联或 `tests/render_balance.rs`）：

| 用例 | 预期 |
|------|------|
| `test_balance_disabled_passthrough` | balance=false 时输入 == 输出 |
| `test_balance_skip_single_row` | 仅 1 行直接返回 |
| `test_balance_merge_short_to_narrower_neighbor` | 中间短行优先与较窄邻居合并 |
| `test_balance_short_at_end_merges_up` | 末尾短行向上合并 |
| `test_balance_short_at_start_merges_down` | 首位短行向下合并 |
| `test_balance_stops_at_cap` | 邻居+短行超 cap → 不合并 |
| `test_balance_max_passes_safety` | 退化输入下不超过 5 遍 |
| `test_balance_skipped_in_compact_mode` | LayoutMode::Compact 不调用 balance |

---

## 五、视觉布局示意（终端 ANSI）

### 5.1 全宽（≥ 120 列）

```
🤖 Opus 4.7 | 📁 ccline | 🌿 main* | ⚡️ 78% · 155k | ⏳ 2m 14s |  78%/64% 47k/60k [5m+1h] | Σ in 12k|142k · out 8k|34k · cache 122k|1.2M | $0.42 | 12m
```

### 5.2 中宽（~100 列，自动换行）

```
🤖 Opus 4.7 | 📁 ccline | 🌿 main* | ⚡️ 78% · 155k
⏳ 2m 14s |  78%/64% 47k/60k [5m+1h] | Σ in 12k|142k · out 8k|34k
$0.42 | 12m
```

### 5.3 窄屏（80 列）

```
🤖 Opus 4.7 | 📁 ccline | 🌿 main*
⚡️ 78% · 155k | ⏳ 2m 14s |  78%/64%
Σ in 12k|142k · out 8k|34k
$0.42 | 12m
```

### 5.3.5 阈值染色场景（v2 新增）

**场景 A：会话开始（context 23%, 命中率 0%, 刚回复 5s）**
```
🤖[橙] | 📁[绿] | 🌿 main*[青] | ⚡️ 23% · 47k[绿] | ⏳ 5s[绿] |  -[绿] | $0.01[默认]
```

**场景 B：会话中段（context 72%, 命中率 78%, 上次回复 8m）**
```
🤖[橙] | 📁[绿] | 🌿 main*[青] | ⚡️ 72% · 144k[黄] | ⏳ 8m 12s[黄] |  78% 47k/60k[绿] | $0.42[默认]
```

**场景 C：危险水位（context 88%, 命中率 25%, 上次回复 35m）**
```
🤖[橙] | 📁[绿] | 🌿 main* ↑23[红] | ⚡️ 88% · 176k[红] | ⏳ 35m 7s[红] |  25%[红] | $1.84[默认]
```

> 颜色 = 严重程度，全段统一锚点。模型/目录/成本永远静态色（不参与阈值），减少视觉噪音。

### 5.3.6 多行布局（v3 expanded 模式）

**配置示例**：

```toml
[layout]
mode = "expanded"
[[layout.merge_groups]]
ids = ["model", "directory", "git"]
```

**未启用 balance 的效果（120 列终端，v3 行为）**：

```
🤖 Opus 4.7 │ 📁 ccline │ 🌿 main*
⚡️ 72% · 144k
 78% 47k/60k [5m+1h]
⏳ 2m 14s
Σ in 12k|142k · out 8k|34k · cache 122k|1.2M
$0.42
12m
```

> ❌ 短行尴尬：`⏳ 2m 14s`、`$0.42`、`12m` 各自落单。

### 5.3.7 行长均衡（v3.1 expanded + balance）

**配置**：

```toml
[layout]
mode = "expanded"
balance = true                    # v3.1 默认启用
min_row_width = 20
# target_row_width 不设 → 自动 0.7 × 终端宽度（120 列时 = 84）

[[layout.merge_groups]]
ids = ["model", "directory", "git"]
```

**均衡后效果（120 列终端）**：

```
🤖 Opus 4.7 │ 📁 ccline │ 🌿 main*                                           ← mergeGroup 硬合并
⚡️ 72% · 144k │  78% 47k/60k [5m+1h] │ ⏳ 2m 14s                            ← 短行向窄邻居合并
Σ in 12k|142k · out 8k|34k · cache 122k|1.2M │ $0.42 │ 12m                   ← 末尾短行向上吸附
```

> ✅ 行数从 7 行 → 3 行；最短行从 3 字符提升到 ~38 字符；视觉权重均匀。

**窄终端（80 列）+ auto_stack=true + balance=true**：

`Σ` 行视宽 ~52 + `$0.42 │ 12m` (~14) → 合并 ≤ 67 列 ≤ 80 → 仍合并：

```
🤖 Opus 4.7 │ 📁 ccline │ 🌿 main*
⚡️ 72% · 144k │  78% 47k/60k [5m+1h]                  ← cap=56 时阻止 ⏳ 加入
⏳ 2m 14s │ Σ in 12k|142k · out 8k|34k · cache 122k... ← 自适应
$0.42 │ 12m                                            ← 已合并避免落单
```

### 5.4 推荐 segment 顺序

`model | directory | git | context_window | since_reply | cache_hit | session_tokens | cost | session`

**理由**：
- `since_reply` 紧跟 `context_window`：两者都是"当前会话即时态"，视线扫描连贯。
- `cache_hit` 紧跟 `since_reply`：缓存命中率与"距上次回复时长"语义相关（命中率反弹后多久 = 待新轮触发）。
- `session_tokens` 在 `cost` 前：低频参考，密度高，放右段。

---

## 六、风险与缓解

| 风险 | 概率 | 影响 | 缓解措施 |
|------|:---:|:---:|----------|
| 长会话 JSONL（>5MB）解析超 5ms 预算 | 中 | 中 | **v1**：直接前向扫描，依赖 serde 性能（实测）。**v2**（需要时）：基于 `(path, mtime, file_size)` 的内存 mmap + 增量解析；或在 `~/.cache/ccometixline/` 落盘 last-stats.bin 做 mtime cache。**先实测再优化**（YAGNI）。 |
| serde 反序列化 timestamp 失败 | 低 | 低 | 单字段 None，segment 优雅降级显示 "-" |
| 时钟回拨导致 `since_reply` 负值 | 低 | 低 | `s.max(0)` 兜底（已写入 §4 Step 3） |
| sidechain 子代理 usage 误算入总数 | 中 | 高 | 默认排除 `is_sidechain == true` 的轮次 |
| 加新 `SegmentId` 破坏旧 `config.toml` 反序列化 | 低 | 高 | serde `#[serde(rename_all = "snake_case")]` 已有，新变体只在 TOML 显式声明时启用。**已验证**：旧 TOML 无 `since_reply` / `session_tokens` 段时 `Vec<SegmentConfig>` 不会失败 |
| `cache_hit` 老用户启用 mode=last 但视觉变化 | 低 | 低 | 默认 `mode = "last"`，primary 与现版本完全一致；secondary 也保持 `47k/60k [5m+1h]` 不变 |
| chrono `parse_from_rfc3339` 拒绝 `2026-04-29T09:07:15.987Z` 类格式 | 低 | 低 | 已用真实样本验证；`Z` 等价 `+00:00`，`parse_from_rfc3339` 接受 |
| ccline 调用频率高，频繁全文件解析磁盘 I/O 压力 | 低 | 低 | OS 层 page cache 接管；ccline 单次 < 100KB 内存（typical session） |
| **v2** SegmentData 加 `level` 破坏第三方/历史 segment 实例化 | 中 | 低 | `level: ThresholdLevel` 用 `#[derive(Default)]`，所有 `SegmentData { primary, secondary, metadata }` 老代码用 `..Default::default()` 补齐即可。本仓库内部 segment 全部一次性补齐，无外部消费者 |
| **v2** ColorConfig 加 `warning`/`critical` 字段破坏老 TOML 反序列化 | 低 | 中 | `#[serde(default)]` 保证旧 TOML 缺这两个字段时反序列化为 `None`，回退链兜底 |
| **v2** 颜色变化太"花"，影响专注 | 中 | 低 | 默认仅 4 个 segment 参与染色（context_window/cache_hit/since_reply/git）；其余保持静态。Normal 状态下视觉与 v1 完全一致 |
| **v3** `generate()` 类型变更（`String` → `Vec<String>`）破坏调用方 | 高 | 中 | 仓库内仅 `main.rs` 与 `ui/components/preview.rs` 调用；一次性同步改完。**外部消费者风险**：cargo 库 `ccometixline` 暴露 `StatusLineGenerator`——若有第三方依赖会失败。**缓解**：保留 `generate_compact() -> String` 兼容方法 + 新增 `generate_lines() -> Vec<String>` 主接口，老 API 标 `#[deprecated]` 但保留一个 minor 版本周期 |
| **v3** Claude Code 状态行多行渲染兼容性 | 低 | 高 | 已用 claude-hud 验证（multi-line statusline 正常工作）；ccline 输出多行 = 多次 `println!`，Claude Code 收到 stdout 全文，渲染前会按 `\n` 分行 |
| **v3** mergeGroups 用 `SegmentId` enum 反序列化兼容性 | 低 | 低 | TOML 中拼写错误的 segment id 反序列化失败 → 启动时 panic 并打印行号——比静默忽略更安全 |
| **v3** 终端宽度检测在 stdout 不是 tty 时失败 | 中 | 低 | 三级回退：`crossterm::terminal::size()` → `$COLUMNS` env → `usize::MAX`（不做 auto_stack） |
| **v3.1** balance 算法贪心选择导致次优行长分布 | 中 | 低 | 接受次优；MAX_PASSES=5 限制总迭代；用户可关 `balance = false` 回到 v3 行为 |
| **v3.1** balance 与用户 mergeGroups 语义冲突 | 低 | 中 | mergeGroups 永远视为硬合并的"原子行"；balance 只动未在 mergeGroups 中的"自由段"产生的行（细节：balance 把已合并行整体作为邻居，不拆分） |
| **v3.1** target_row_width 默认 0.7 不适合所有终端宽度 | 中 | 低 | 用户可显式 set `target_row_width = N`；缺省值经验取 0.7 兼顾密度与留白 |

---

## 七、性能预算与验证

| 指标 | 目标 | 验证方法 |
|------|------|---------|
| ccline 单次启动总耗时 | < 5ms（典型）；< 20ms（10MB JSONL） | `hyperfine` 对比 v1.1.2 |
| `parse_session_stats` 解析耗时 | < 3ms（1000 行）；< 30ms（10000 行） | criterion bench（可选）+ 内置 `--bench` flag |
| 内存峰值 | < 5MB | `/usr/bin/time -v` |

**注**：当前 JSONL 解析逐行 `serde_json::from_str::<TranscriptEntry>` 已是文件已有模式，本计划只是把"扫描方向"从倒序改为正序+累加，**无新增 IO**。

---

## 八、测试策略

### 8.1 单元测试（新增 `src/core/transcript.rs` 内联 `#[cfg(test)]`）

| 用例 | 目的 |
|------|------|
| `test_session_stats_empty_file` | 空文件 → `None` |
| `test_session_stats_no_assistant` | 仅 user 行 → `None` |
| `test_session_stats_single_assistant` | 单 assistant → `last_turn == totals` |
| `test_session_stats_three_assistants` | 三 assistant → totals 是各项之和 |
| `test_session_stats_skips_sidechain` | sidechain=true 行不计入 totals |
| `test_session_stats_malformed_lines` | 脏 JSON 行被跳过，正常行仍累计 |
| `test_session_stats_parses_timestamp` | RFC3339 timestamp 被正确解析 |
| `test_session_stats_invalid_timestamp` | 坏 timestamp 字段，stats.last_assistant_at = None，其余字段不丢 |

### 8.2 Segment 测试（每个新 segment 内联）

| 用例 | 目的 |
|------|------|
| `since_reply::test_format_elapsed_zero_negative` | 0/负值 → "0s" |
| `since_reply::test_format_elapsed_seconds_minutes_hours_days` | 边界值 |
| `since_reply::test_classify_thresholds` | 299s→Normal, 300s→Warning, 1799s→Warning, 1800s→Critical |
| `session_tokens::test_format_tokens_units` | < 1000 / 1k / 12.3k / 1.2M 边界 |
| `cache_hit::test_mode_last_total_both` | 三种 mode 的 primary 输出格式 |
| `cache_hit::test_mode_default_is_last` | 缺省 mode 时回退到 last（向后兼容） |
| `cache_hit::test_level_reverse_direction` | 80%→Normal, 50%→Warning, 25%→Critical（反向阈值） |
| `context_window::test_level_thresholds` | 30%→Normal, 75%→Warning, 90%→Critical |
| `statusline::test_pick_color_fallback_chain` | critical 缺 → warning，warning 缺 → text，全缺 → None |
| `statusline::test_render_critical_emits_red_ansi` | data.level=Critical 时 generate() 输出含 `\x1b[38;5;167m` |

### 8.3 集成验证（手动）

构建二进制后，将以下 segment 加入 `~/.claude/ccline/config.toml`，重启 Claude Code，观察：

```toml
[[segments]]
id = "since_reply"
enabled = true
icon.plain = "⏳"
icon.nerd_font = ""
colors.icon.c256 = 223
colors.text.c256 = 223
styles.text_bold = false
options = {}

[[segments]]
id = "session_tokens"
enabled = true
icon.plain = "Σ"
icon.nerd_font = ""
colors.icon.c256 = 175
colors.text.c256 = 175
styles.text_bold = false
options = {}

[[segments]]
id = "cache_hit"
enabled = true
# ... 现有配置 ...
[segments.options]
mode = "both"            # 新选项：last / total / both
```

**预期**：
1. `since_reply` 在每次 Claude 回复后从 `0s` 开始，递增显示
2. `session_tokens` 显示 `last|total` 双数
3. `cache_hit` primary 变为 `XX%/YY%` 格式

---

## 九、回滚方案

| 阶段 | 回滚方式 |
|------|---------|
| 编译前 | `git stash` 或 `git checkout master -- src/` |
| 已 build 但未替换 | 删除新 build，使用 `~/.claude/ccline/ccline.v1.1.2.bak`（文档已知备份） |
| 已替换 binary，行为异常 | `cp ~/.claude/ccline/ccline.v1.1.2.bak ~/.claude/ccline/ccline` 即可（10 秒回滚） |
| TOML 已加新 segment | 注释掉 `[[segments]]` 块；旧 binary 不识别新 ID 会反序列化失败 → **必须在回滚 binary 前先回滚 TOML** |

**回滚顺序**：先 TOML，再 binary，再重启 Claude Code。

---

## 十、关键文件清单

| 文件 | 操作 | LOC 估算 |
|------|------|---------|
| `src/config/types.rs` | 修改：`TranscriptEntry` +2 字段；`ColorConfig` +2 字段；新增 `SessionUsageStats`；`SegmentId` +2 变体；**v3** 新增 `LayoutConfig` / `LayoutMode` / `MergeGroup`；`Config` 加 `layout` 字段 | +60 |
| `src/core/transcript.rs` | 新增 `parse_session_stats`、`accumulate_into`；改写 `parse_last_assistant_usage` 为包装 | +80 |
| `src/core/segments/mod.rs` | 注册 +2 mod / pub use；`SegmentData` +`level`；新增 `ThresholdLevel` enum | +20 |
| `src/core/segments/since_reply.rs` | **新建**（含 classify） | +85 |
| `src/core/segments/session_tokens.rs` | **新建** | +90 |
| `src/core/segments/cache_hit.rs` | 修改：新增 mode 选项 + total 计算 + 反向 level | +50 |
| `src/core/segments/context_window.rs` | 修改：新增 level 计算（占比阈值） | +15 |
| `src/core/segments/usage.rs` | 修改：新增 level 计算（quota 阈值） | +15 |
| `src/core/segments/git.rs` | 修改：ahead 阈值 → level | +12 |
| `src/core/statusline.rs` | v2: match +2 臂 / 染色装配 / `pick_text_color`；**v3**：`generate()` 返回 `Vec<String>`、抽 `render_single_line`、新增 `render_expanded` + `build_merge_lookup` + `detect_terminal_width`；**v3.1** 新增 `balance_rows` + 8 个均衡测试 | +180 |
| `src/main.rs` | **v3** 输出循环 println | +5 |
| `src/ui/components/preview.rs`（若存在） | **v3** 消费 `Vec<String>`，逐行渲染 | +20 |
| `src/ui/components/segment_list.rs` | 显示名 +2 | +2 |
| `src/ui/themes/theme_gruvbox.rs` | 默认 segment +2 + 老 segment 补色 + **v3** 默认 `layout: LayoutConfig::default()`（compact，零行为变更） | +95 |
| **小计** | | **~729 LOC**（含测试） |

**注**：v3.1 在 v3 基础上 +50 LOC（balance 算法 + 8 个测试），全部新增在 `statusline.rs` 内，不扩散。**核心代码（不含测试）约 380 LOC**。

---

## 十一、验收判据

### Stage 1：编译与单元测试
- [ ] `cargo build --release` 通过，无 warning
- [ ] `cargo test` 全绿，覆盖 §8.1 / §8.2 全部用例
- [ ] `cargo clippy -- -D warnings` 无新增 lint

### Stage 2：手动集成验收
- [ ] 旧 `config.toml`（不加新 segment + 不加 warning/critical 字段）不变 → ccline 输出 byte-equal v1.1.2
- [ ] 启用 `cache_hit.mode = "last"` → primary 与 v1.1.2 一致
- [ ] 启用 `cache_hit.mode = "both"` → primary 显示 `XX%/YY%`
- [ ] 启用 `since_reply` → 显示 `Xs` / `Xm Ys`，每次刷新递增
- [ ] 启用 `session_tokens` → `in: a|b · out: c|d · cache: e|f` 格式正确
- [ ] sidechain（Task agent）调用期间，session_tokens **不增加** totals
- [ ] WSL2 终端 `hyperfine '~/.claude/ccline/ccline < sample.json'` 中位数 < 10ms（10MB JSONL）
- [ ] **阈值染色 v2**：context_window 在 23% / 75% / 88% 时分别绿/黄/红
- [ ] **阈值染色 v2**：cache_hit 在 80% / 50% / 25% 时分别绿/黄/红（反向）
- [ ] **阈值染色 v2**：since_reply 在 30s / 8min / 35min 时分别绿/黄/红
- [ ] **阈值染色 v2**：旧 TOML 没设 warning/critical 时，颜色保持 `colors.text` 静态值（回退链工作）
- [ ] **多行布局 v3**：旧 TOML 不写 `[layout]` → 默认 compact 单行，输出与 v1.1.2 byte-equal
- [ ] **多行布局 v3**：`mode = "expanded"` + 无 merge_groups → 每段独占一行
- [ ] **多行布局 v3**：`merge_groups = [["model","directory","git"]]` → 三段同行用 ` │ ` 拼接
- [ ] **多行布局 v3**：终端宽度 80 列 + 合并行视宽 100 列 + auto_stack=true → 合并行自动拆回每段一行
- [ ] **多行布局 v3**：Claude Code 实际状态行多行渲染正常（各行无截断、无错位、ANSI 颜色不串色）
- [ ] **行长均衡 v3.1**：5 行输入（含 3 个 < 20 字符短行）→ 输出行数减少，最短行 ≥ min_row_width
- [ ] **行长均衡 v3.1**：`balance = false` 时输入 == 输出（passthrough）
- [ ] **行长均衡 v3.1**：mergeGroups 硬合并的多段行不被拆分（硬约束优先）
- [ ] **行长均衡 v3.1**：合并到 cap 上限时停止（不强行超 cap） |

### Stage 3：回归验证
- [ ] 7 天日常使用，状态行无视觉异常
- [ ] cargo bench（可选）显示 parse_session_stats 性能拐点

---

## 十二、未覆盖范围（YAGNI 边界）

明确**不在本计划内**的项，防止 scope creep：

- ❌ 不实现工具/Agent/Todo 可视化（路径 D 的明确放弃项；这些是 claude-hud 的强项，路径 D 与之拉开差异化）
- ❌ 不增加正向计时缓存（先单次扫描；后续仅在实测超 5ms 时再加）
- ❌ 不重构现有 segment 数据流（context_window 等老 segment 继续用 `parse_last_assistant_usage` 包装；只补 level 计算）
- ❌ 不引入新依赖（chrono 已在 Cargo.toml；不新增第三方 crate）
- ❌ 不改 TUI 编辑器对新 segment 的可视化编辑（使用 TOML 直接编辑即可；UI 增强排到下一轮）
- ❌ **v2** 不实现 claude-hud 的 `coloredBar` 进度条字符（`█`*N + `░`*M）；ccline 用文字百分比 + 阈值染色已足够；如需图形化 bar 后续单独 issue
- ❌ **v2** 不实现 icon 与 text 颜色分离（如 icon 始终静态、仅 text 染色）；v2 默认 icon/text 联动，视觉一致性优先
- ❌ **v3** 不实现 claude-hud 的 `showSeparators`（在 activity-line 之间插 `─` 横线分隔器）；ccline 路径 D 不引入 activity 概念
- ❌ **v3** 不实现 TUI 编辑器对 `[layout]` 块的可视化编辑；用户用 TOML 编辑
- ❌ **v3** 不引入 label-align 列对齐（claude-hud 用 `alignProgressLabels` 把进度条标签纵向对齐）；ccline segment 形状不固定，对齐成本高，YAGNI

---

## 十三、SESSION_ID（供 /ccg:execute 使用）

- CODEX_SESSION: **N/A**（401 鉴权失效，本次未启动）
- GEMINI_SESSION: **N/A**（GEMINI_API_KEY 未配置）
- CLAUDE_CONTEXT: 本计划全文 + 已 Read 的源码（`transcript.rs`/`types.rs`/`cache_hit.rs`/`cost.rs`/`statusline.rs`/`segments/mod.rs`/`segments/session.rs`/`segments/context_window.rs`/`theme_gruvbox.rs`）+ 真实 JSONL 样本 schema

`/ccg:execute` 重新进入会话时，至少应：
1. Read 上述 9 个源文件
2. 验证当前 git HEAD 仍是 master 或新建 feature 分支（建议 `feature/path-d-extension`）
3. 按 §四 Step 1–6 顺序执行（数据层先行，segment 后建，注册触点最后）

---

## 十四、下一步

> **v3.1 计划已写入 `docs/plans/path-d-ccline-extension.md`，待椰椰审查。**
>
> **v1 → v2 主要差异**：新增 §三.5 染色机制移植 + §四 Step 7 渲染层染色装配 + 4 处老 segment 补 level 计算 + LOC ~377 → ~524。
>
> **v2 → v3 主要差异**：新增 §三.6 多行布局机制移植 + §四 Step 8 渲染层多行装配 + `Config` 加 `[layout]` 块 + `generate()` 返回类型 `String → Vec<String>` + LOC ~524 → ~679。
>
> **v3 → v3.1 主要差异**：新增 §三.6.6 行长均衡算法 + §四 Step 8.1 balance pass + `LayoutConfig` 加 3 字段（balance/min_row_width/target_row_width）+ LOC ~679 → ~729。**这是对 claude-hud 的能力升级**（claude-hud 没做行长均衡，因为 element 颗粒度粗）。
>
> **实施顺序建议**：v1 数据层（Step 1–6）→ v2 阈值染色（Step 7）→ v3 多行布局（Step 8）→ v3.1 行长均衡（Step 8.1）。每个阶段都对老用户保持 byte-equal 兼容（默认 compact + 无 warning/critical 字段 + `balance = true` 仅 expanded 生效 = v1.1.2 行为）。
