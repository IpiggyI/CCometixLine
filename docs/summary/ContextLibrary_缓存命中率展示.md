# ContextLibrary: 缓存命中率展示

## 基本信息

- **创建时间**: 2026-04-30
- **修改时间**: 2026-04-30
- **涉及文件**: src/config/types.rs, src/core/mod.rs, src/core/transcript.rs (新建), src/core/segments/cache_hit.rs (新建), src/core/segments/context_window.rs, src/core/segments/mod.rs, src/core/statusline.rs, src/ui/app.rs, src/ui/components/preview.rs, src/ui/components/segment_list.rs, src/ui/components/settings.rs
- **摘要**: ccline 状态栏新增 CacheHit segment，展示 prompt 缓存命中率与 5m/1h 双 TTL 标签，复用并抽出 transcript 公共解析模块

## 需求详情

用户在使用 Claude Code 状态栏工具 ccline (CCometixLine) 时提出两个诉求：

1. **当前模型思考强度**展示
2. **缓存命中率**展示

后续追问：缓存的 5min / 1h TTL 是否能区分展示。

## 解决方案

### 思考强度（暂不做）

调研结论：Claude Code statusline stdin JSON 与 transcript jsonl 都未暴露 `reasoning_effort` / `thinking_budget` 字段。transcript 中 assistant content 数组里偶有 `"type":"thinking"` block，但只是开关性质（用了/没用），无强度档位。本期跳过，等官方接口暴露。

### 缓存命中率（新增 CacheHit segment）

**数据来源**：复用 transcript jsonl 中最后一条 assistant message 的 `usage` 字段。Anthropic 实测结构含嵌套 TTL：

```json
"usage": {
  "input_tokens": 1,
  "cache_creation_input_tokens": 1260,
  "cache_read_input_tokens": 96260,
  "output_tokens": 2655,
  "cache_creation": {
    "ephemeral_5m_input_tokens": 0,
    "ephemeral_1h_input_tokens": 1260
  }
}
```

**命中率公式**：
```
hit_rate = cache_read / (input + cache_read + cache_creation) × 100%
```

**TTL 标签判定**（取 cache_creation 嵌套对象，仅"创建侧"，命中侧 TTL Anthropic 不暴露）：

| 5m tokens | 1h tokens | 显示 |
|-----------|-----------|------|
| =0 | >0 | `[1h]` |
| >0 | =0 | `[5m]` |
| >0 | >0 | `[5m+1h]` |
| 都=0 / 字段缺失 | — | 不追加 |

**显示格式**：`<hit_rate>% · <cache_read>/<prompt_total> [TTL]`，例如 `⚡ 98.5% 222.2k/225.7k [1h]`。

**默认禁用**：沿用 Update segment 的纯代码注册模式——不挂任何主题预设，用户在 `~/.claude/ccline/config.toml` 手动添加 `[[segments]] id = "cache_hit"` 块，或在 TUI 配置器（`ccline -c`）启用。

### 重构：抽公共 transcript 模块

ContextWindow 与 CacheHit 都需要"读取最后一条 assistant message 的 NormalizedUsage"。新建 `src/core/transcript.rs::parse_last_assistant_usage(path) -> Option<NormalizedUsage>` 统一入口，处理三种场景：
1. 普通 transcript：回溯找最后一条 assistant message
2. summary 行：通过 `leafUuid` 跨文件查找叶子节点
3. 文件不存在：降级到 project 目录下最近的 session jsonl

ContextWindow 重构改用公共方法（行为完全不变），删除原来 ~150 行重复代码。

## 关键决策

| 决策点 | 选择 | 原因 |
|--------|------|------|
| 思考强度展示 | 本期不做 | API 未暴露强度档位 |
| 缓存命中率展示形式 | 独立 CacheHit segment | 颗粒度高于合并到 ContextWindow/Usage secondary，可单独配色启停 |
| 命中率分母 | input + cache_read + cache_creation | 与 Anthropic prompt 侧总消耗一致；output 不参与 cache lookup |
| TTL 数据来源 | 仅 cache_creation 嵌套（创建侧） | cache_read 没有 TTL 嵌套字段，命中侧 TTL Anthropic 不暴露 |
| 默认开关 | 默认禁用 | 不破坏老用户布局；沿用 Update segment 模式 |
| transcript 解析复用 | 抽公共 src/core/transcript.rs | 避免 ContextWindow 与 CacheHit 重复 100+ 行 |
| Fork 提交策略 | origin → upstream，IpiggyI fork 设为新 origin | 上游 Haleclipse 非用户所有，需 fork 工作流 |

## 问题与解决

| 问题 | 解决方法 |
|------|----------|
| ccg verify-quality 工具不支持 Rust（扫到 0 文件） | 手动复核函数行数与文件复杂度，确认达标 |
| 9 处 SegmentId match 必须穷尽 | 跟编译错误指引逐个补全：collect_all_segments / segment_list / settings / app (2 处) / preview / segments mod 注册 / 默认主题 |
| 部署时定位真二进制：PATH 上 ccline 是 npm JS shim | 真正二进制在 `~/.claude/ccline/ccline`（npm 包安装时下载）；只替换它一个，settings.json 不动 |
| .gitignore 多了 .ace-tool/ 行（ace-tool MCP 自动加） | 与本次 feature 无关，不纳入提交 |

## 成果

- [x] CacheHit segment 落地，feature/cache-hit-segment 分支 commit `c3256cc`
- [x] cargo check 通过
- [x] 真实 transcript 端到端验证：`⚡ 98.5% 222.2k/225.7k [1h]`
- [x] 抽公共 transcript 模块，ContextWindow 共用（行为不变，净删 125 行）
- [x] verify-change 质量门通过
- [x] Fork 仓库 (IpiggyI/CCometixLine) push 完成
- [x] 本地 ccline 部署：备份旧版本 + 替换 release 二进制 + config.toml 启用 CacheHit

## 备注

- **Commit**: `c3256cc feat(segments): add cache-hit segment with TTL breakdown`
- **Fork URL**: `git@github.com:IpiggyI/CCometixLine.git`
- **上游 PR**: 用户可在 GitHub UI 手动给 Haleclipse/CCometixLine 提 PR
- **回滚命令**:
  ```bash
  cp ~/.claude/ccline/ccline.v1.1.2.bak ~/.claude/ccline/ccline
  cp ~/.claude/ccline/config.toml.bak ~/.claude/ccline/config.toml
  ```
- **npm 升级风险**: `npm update -g @cometix/ccline` 会覆盖 `~/.claude/ccline/ccline`，需重跑 `cp target/release/ccometixline ~/.claude/ccline/ccline`
- **后续可扩展**: Claude Code 若在 stdin JSON 暴露 `reasoning_effort`，再加 thinking 强度 segment
