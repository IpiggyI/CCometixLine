# ContextLibrary: 阈值动态染色（path-d V2 Step 7）

## 基本信息

- **创建时间**: 2026-05-07
- **修改时间**: 2026-05-07
- **涉及文件**: src/config/types.rs, src/core/segments/mod.rs, src/core/segments/{cache_hit,context_window,since_reply,usage,git,cost,directory,model,output_style,session,session_tokens,update}.rs, src/core/statusline.rs, src/ui/components/{settings,segment_list,preview}.rs, src/ui/app.rs, src/ui/themes/theme_*.rs (9 个主题), tests/render_threshold.rs（新建）, .gitignore
- **摘要**: 移植 claude-hud 阈值染色机制到 ccline，segment 自决 ThresholdLevel，渲染层按 critical→warning→text 回退链动态选色，5 个 segment 接入阈值（含 cache_hit 反向），9 主题补默认警告/严重配色，TUI 编辑器同步支持

## 需求详情

椰椰指令："docs/plans/path-d-ccline-extension.md V1已经完成，现在跳到 Step 7 开始执行V2"

V1（Steps 1-6 非 v2 部分）已完成 since-reply、session-tokens、cache-hit segment 和 transcript 解析层。V2 = Step 7 渲染层阈值染色装配 + 关联的 Step 1 v2 数据模型扩展（ColorConfig.warning/critical, SegmentData.level）+ Steps 3/4/5 的 level 计算 + Step 6.4 主题默认值。

## 解决方案

### 数据模型（B 方案：分级 ColorConfig + Segment 自决 ThresholdLevel）

```rust
// src/config/types.rs
pub struct ColorConfig {
    pub icon: Option<AnsiColor>,
    pub text: Option<AnsiColor>,
    pub background: Option<AnsiColor>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub warning: Option<AnsiColor>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub critical: Option<AnsiColor>,
}

// src/core/segments/mod.rs
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum ThresholdLevel {
    #[default] Normal,
    Warning,
    Critical,
}

pub struct SegmentData {
    pub primary: String,
    pub secondary: String,
    pub metadata: HashMap<String, String>,
    pub level: ThresholdLevel,  // 新增
}
```

向后兼容：旧 `config.toml` 没有 warning/critical 字段时 serde 默认 None；`level` 默认 Normal，旧 segment 无变化。

### 渲染层回退链（src/core/statusline.rs）

```rust
fn pick_text_color(config: &SegmentConfig, level: ThresholdLevel) -> Option<&AnsiColor> {
    match level {
        ThresholdLevel::Normal   => config.colors.text.as_ref(),
        ThresholdLevel::Warning  => config.colors.warning.as_ref()
            .or(config.colors.text.as_ref()),
        ThresholdLevel::Critical => config.colors.critical.as_ref()
            .or(config.colors.warning.as_ref())
            .or(config.colors.text.as_ref()),
    }
}

fn pick_icon_color(config: &SegmentConfig, level: ThresholdLevel) -> Option<&AnsiColor> {
    match level {
        ThresholdLevel::Normal => config.colors.icon.as_ref(),
        _ => pick_text_color(config, level).or(config.colors.icon.as_ref()),
    }
}
```

`render_segment` 中 4 处 `config.colors.{text,icon}` 替换为 `pick_*_color(config, data.level)`。

### Segment 阈值计算

| Segment | 度量 | warning | critical | options key | 方向 |
|---------|------|---------|----------|-------------|------|
| context_window | 上下文占比 % | 70 | 85 | warning_threshold/critical_threshold | 越大越坏 |
| usage | 5 小时配额 % | 75 | 90 | warning_threshold/critical_threshold | 越大越坏 |
| cache_hit | 命中率 %（**反向**）| 60↓ | 30↓ | warning_below/critical_below | 越小越坏 |
| since_reply | 距上次回复秒数 | 300 | 1800 | warning_seconds/critical_seconds | 越大越坏 |
| git | ahead commit 数 | 5 | 20 | push_warning_threshold/push_critical_threshold | 越大越坏 |

cache_hit `mode=both` / `total` 时基于 total_pct 染色，`mode=last` 基于 last_pct。

`collect_all_segments` 通过 `option_f64/i64/u32` helper 从 `SegmentConfig.options` 注入构造器：

```rust
let segment = ContextWindowSegment::new()
    .with_thresholds(
        option_f64(segment_config, "warning_threshold", 70.0),
        option_f64(segment_config, "critical_threshold", 85.0),
    );
```

### 主题默认配色（9 个文件）

- Gruvbox c256 系（gruvbox/cometix/default/minimal）：warning c256=214 (黄)，critical c256=167 (红)；usage warning c256=175 (粉，区别 context_window 的黄)
- Nord：warning #EBCB8B + critical #BF616A
- Tokyo Night：warning #E0AF68 + critical #F77698
- Rose Pine：warning #F6C177 + critical #EB6F92
- Powerline Dark：warning #E5C07B + critical #E06C75
- Powerline Light：warning #FFEB3B (Material yellow-500) + critical #FF8A80 (Material red A100)，针对 gray-500 段背景调亮以提升 WCAG 对比度

### TUI 编辑器扩展（settings.rs + segment_list.rs + app.rs）

- `FieldSelection` 新增 `WarningColor`/`CriticalColor` 变体
- `app.rs` 字段计数 7→9，导航 match 同步；`open_color_picker` / `apply_selected_color` 接入 warning/critical
- `settings.rs` 抽公共 helper `ansi_to_ratatui` / `describe_ansi_color` 替换 5 段重复 ~120 行 → 合计净减 ~80 LOC
- 编辑面板顺序：Enabled → Icon → IconColor → TextColor → BackgroundColor → **WarningColor** → **CriticalColor** → TextStyle → Options

### 测试

- statusline.rs 内联 5 个单元测试覆盖 pick_text_color / pick_icon_color 全回退路径
- tests/render_threshold.rs 新建端到端 snapshot 测试：3 用例验证 Normal/Warning/Critical 输出包含正确 ANSI 256 色码（142/214/167）+ 回退链 critical→warning→text

## 关键决策

| 决策点 | 选择 | 原因 |
|--------|------|------|
| 数据模型方案 | B：ColorConfig 加级别字段 + segment 自决 level | 对齐 ccline "每段配色独立" 哲学；阈值可配置；零侵入老 segment（A 无 segment 粒度，C metadata 类型不安全） |
| icon 染色行为 | Normal 保留静态，Warning/Critical 跟随 text 链路 | 视觉聚焦；保持平时调色板观感 |
| 回退链方向 | critical → warning → text → 无色 | 保证任何配置层级都有合理默认 |
| cache_hit 染色基准 | mode=both/total 用 total_pct，mode=last 用 last_pct | 与显示数值一致，避免视觉不匹配 |
| usage warning 默认色 | c256=175 (粉) | 与 context_window 的 214 (黄) 拉开层级，避免颜色 "Christmas tree" |
| session_tokens 不染色 | 保持 Normal | 显示绝对值不天然匹配 % 阈值；避免视觉过载 |
| Powerline Light 染色 | #FFEB3B + #FF8A80 替代 Bootstrap #FFC107 + #DC3545 | 原 #DC3545 在 gray-500 bg 仅 1.04:1（远低于 WCAG AA 4.5:1）；新值接近 AA Large 阈值 |
| 应用方式 | 放弃 git apply diff，改 Edit 工具逐文件应用 | codex 输出 diff 的 @@ 行号有偏差，git apply --recount 失败；Edit 更可控 |
| TUI 编辑器扩展 | 当前 step 内做（gemini Critical） | 避免 "黑盒配置" 体验，用户能看到颜色变化但无 UI 入口编辑 |
| Powerline Light 修复 | 当前 step 内做（gemini Medium） | 一行修改，零成本 |

## 问题与解决

| 问题 | 解决方法 |
|------|----------|
| codex diff 中 hunk 头 `@@ -1,6 +1,6 @@` 实际 body 仅 5 行（少 1 行尾部上下文） | git apply --recount 也失败；改用 Edit 工具按 codex 设计意图逐文件手工应用 1607 行 diff |
| codex 首次输出被 `[PROGRESS] message` 截断只显示 ColorConfig 第一个 hunk | resume session 二次输出，定向要求 "complete unified diff in fenced code block" |
| 12 个 SegmentData 字面量 + 9 个主题 ColorConfig 字面量需要批量补字段 | Python 脚本：状态机扫描 `SegmentData {`/`ColorConfig {` 入口，深度计数到匹配的 `}`，按 segment 类型注入对应字段 |
| codex 审计发现 usage warning 误用 c256=214（应为 175） | 二次脚本修复 default/cometix/gruvbox/minimal 4 个主题的 usage_segment |
| codex 审计发现 tests/ 被 .gitignore 屏蔽 → 集成测试不会被 git add | 移除 .gitignore 中 tests/ 行 |
| gemini 审计发现 settings.rs 没有 warning/critical 编辑入口 → "黑盒配置" | 扩展 FieldSelection 枚举 + app.rs 导航 + settings.rs 渲染（同时抽公共 helper 净减 80 LOC） |
| gemini 审计发现 Powerline Light warning #FFC107 在 gray-500 上对比度仅 3.0:1 | 改为 Material yellow-500 #FFEB3B + Material red A100 #FF8A80，提升至接近 AA Large（无法严格 AA，gray-500 bg 对任何饱和色都难达 4.5:1） |
| 用户既有 ~/.claude/ccline/config.toml 缺 warning/critical 字段 | serde(default) 让 None 反序列化，回退链返回 text 静态色 → 行为零变化 ✓ |

## 成果

- [x] V2 Step 7 全量落地：26 文件修改 + 1 新建（tests/render_threshold.rs）+ 1 .gitignore 修改
- [x] cargo build/test/clippy 三连绿：24 测试通过（3 unit suites + 1 integration suite）+ clippy 零警告
- [x] 真实二进制端到端验证：25%→c256=142(绿) / 75%→c256=214(黄) / 90%→c256=167(红)
- [x] 向后兼容验证：旧 config.toml 无 warning/critical → 自动回退 text 静态色
- [x] 双模型审计闭环：codex 后端审计（83/100→修复 2 项 Medium）+ gemini UX 审计（85/100→修复 1 项 Critical + 1 项 Medium）
- [x] settings.rs 重构：抽 ansi_to_ratatui/describe_ansi_color 公共 helper，5 段重复逻辑合并

## 备注

- **基础架构**：base feature/path-d-extension 分支 commit `6050e0c`（V1）→ 本次 commit V2
- **TOML 配置示例**：

  ```toml
  [[segments]]
  id = "context_window"
  enabled = true

  [segments.colors.text]
  c256 = 142
  [segments.colors.warning]
  c256 = 214
  [segments.colors.critical]
  c256 = 167

  [segments.options]
  warning_threshold = 70.0
  critical_threshold = 85.0
  ```

- **后续可扩展方向**:
  1. Powerline Light 主题 v2：将 segment 背景从 gray-500 调浅至 gray-300/400 以彻底满足 WCAG AA strict
  2. 阈值默认值常量化：现各 segment 内 unwrap_or(70.0) 散落，可抽 `mod thresholds_default` 集中
  3. cache_hit `mode=both` 基于 total 染色的设计在 README 文档化（避免用户困惑：显示 "78%/64%" 但红色基于 64%）
- **回滚命令**:
  ```bash
  git checkout 6050e0c -- src/ tests/ .gitignore
  cargo build --release
  cp target/release/ccometixline ~/.claude/ccline/ccline
  ```
