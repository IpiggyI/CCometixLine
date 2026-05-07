# CacheHit Segment 实施方案

- 时间戳：2026-04-29
- 任务背景：椰椰提出在 ccline 状态栏增加缓存命中率展示。同时验证"思考强度档位"目前 Claude Code 未通过 stdin/transcript 暴露，本期不做。

## 核心诉求

| 诉求 | 决策 |
|------|------|
| 缓存命中率 | 新增独立 `CacheHit` segment |
| TTL (5m/1h) | 在 CacheHit 末尾追加 `[5m]` / `[1h]` / `[5m+1h]` 标签 |
| 思考强度 | 暂不做（API 未暴露 reasoning_effort） |
| 默认开关 | 默认禁用，跟 `Update` segment 一样走纯代码注册，不挂任何 theme |

## 数据来源（已实测）

Anthropic transcript jsonl 中 assistant message 的 `usage` 字段：

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

- `cache_read_input_tokens`：本轮命中缓存读取 token
- `cache_creation_input_tokens`：本轮新创建缓存 token（合计）
- `cache_creation.ephemeral_5m_input_tokens` / `ephemeral_1h_input_tokens`：5m / 1h TTL 细分（仅创建侧有，命中侧无 TTL 信息）

## 命中率公式

```
hit_rate = cache_read / (input_tokens + cache_read + cache_creation) × 100%
```

- 分母：本轮 prompt 侧总消耗 token（不含 output，因为 output 不参与下一轮 cache read 比较语义）
- 分子：本轮命中缓存读取的 token
- 分母为 0 → 显示 `-`

## TTL 标签判定（取最近一条 assistant message 的 cache_creation）

| 5m tokens | 1h tokens | 显示 |
|-----------|-----------|------|
| =0 | >0 | `[1h]` |
| >0 | =0 | `[5m]` |
| >0 | >0 | `[5m+1h]` |
| 都=0 / 字段缺失 | — | 不追加 |

## 显示设计

- primary：`92%`
- secondary：`45.2k/49.0k [1h]`（命中量/prompt 总量 + TTL 标签）
- 图标：Plain `⚡`，Nerd Font ``（沿用 ContextWindow 的紫色调）

## 涉及文件

| 文件 | 操作 |
|------|------|
| `src/config/types.rs` | 加 `CacheCreationDetails` 结构、RawUsage.cache_creation、NormalizedUsage.cache_creation_5m/1h、normalize 填充；SegmentId 加 `CacheHit` |
| `src/core/transcript.rs` | **新建**：抽公共 `parse_last_assistant_usage(path) -> Option<NormalizedUsage>` |
| `src/core/mod.rs` | 注册 transcript 模块 |
| `src/core/segments/context_window.rs` | 重构调用公共方法（行为不变） |
| `src/core/segments/cache_hit.rs` | **新建**：CacheHit segment 实现 |
| `src/core/segments/mod.rs` | 注册导出 CacheHitSegment |
| `src/core/statusline.rs` | `collect_all_segments` match 加 CacheHit 分支 |
| `src/ui/components/segment_list.rs` | match 加 `CacheHit => "Cache Hit"` |
| `src/ui/components/preview.rs` | match 加 mock 数据 |

不改主题文件（沿用 `Update` segment 的模式：纯代码注册，用户在 `~/.claude/ccline/config.toml` 手动添加或后续主题更新加入）。

## 步骤

1. 改 `src/config/types.rs`（新增结构 + SegmentId 变体）
2. 新建 `src/core/transcript.rs`（公共解析）+ 注册到 `src/core/mod.rs`
3. 重构 `src/core/segments/context_window.rs` 改用公共方法
4. 新建 `src/core/segments/cache_hit.rs` + 注册到 `src/core/segments/mod.rs`
5. `src/core/statusline.rs::collect_all_segments` 加 match 分支
6. `src/ui/components/segment_list.rs` + `preview.rs` 加 match 分支
7. `cargo check` 编译验证
8. 触发质量门：`verify-change` + `verify-quality`
