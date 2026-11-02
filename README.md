# CCometixLine

[English](README.md) | [中文](README.zh.md)

A fast, composable status line for Claude Code, written in Rust, with a full-screen configurator.

![Language:Rust](https://img.shields.io/static/v1?label=Language&message=Rust&color=orange&style=flat-square)
![License:MIT](https://img.shields.io/static/v1?label=License&message=MIT&color=blue&style=flat-square)

![CCometixLine in Claude Code](assets/hero.webp)

## Highlights

- **Your session at a glance**: model and effort, directory, git, context use, prompt cache countdown and hit ratio, the 5-hour and weekly limits with your pace against the clock, cost, PR and more: 30+ metrics in all.
- **Any metric, any form**: each item can be text, a progress bar, a gauge, a sparkline or a pace bar, with three text templates for when room runs short.
- **Fits the terminal**: lines wrap between items when the terminal is narrow; a right-hand group stays flush right.
- **Looks**: four skins (plain, bracket, pill, powerline), 13 palettes that follow your terminal's colors or bring their own, Nerd Font, Unicode or ASCII glyphs, and color depth from truecolor down to none.
- **Nine presets** to start from, and your own saved alongside them.
- **A configurator** that edits everything with a live preview of sample sessions, and points Claude Code at ccline for you.
- **Accurate for third-party models**: a model registry fixes names and context windows Claude Code can't know.
- **Background tasks**: the same binary draws the line under each running subagent.
- **Light**: a render costs a few milliseconds over process start; git runs in the background, never on the render path.

## Install

```bash
npm install -g @cometix/ccline
```

<details>
<summary>yarn, pnpm, the npm mirror</summary>

```bash
yarn global add @cometix/ccline
pnpm add -g @cometix/ccline
npm install -g @cometix/ccline --registry https://registry.npmmirror.com
```

</details>

The package puts the binary at `~/.claude/ccline/ccline` and the `ccline` command on your PATH. To update later: `npm update -g @cometix/ccline`.

<details>
<summary>Manual download</summary>

Download the archive for your platform from [Releases](https://github.com/Haleclipse/CCometixLine/releases) and put the binary at `~/.claude/ccline/ccline` (`ccline.exe` on Windows).

| Platform | Archive |
|---|---|
| Linux x64 (glibc 2.35+) | `ccline-linux-x64.tar.gz` |
| Linux x64, static | `ccline-linux-x64-static.tar.gz` |
| Linux arm64 | `ccline-linux-arm64.tar.gz` |
| Linux arm64, static | `ccline-linux-arm64-static.tar.gz` |
| macOS Intel | `ccline-macos-x64.tar.gz` |
| macOS Apple Silicon | `ccline-macos-arm64.tar.gz` |
| Windows x64 | `ccline-windows-x64.zip` |

```bash
mkdir -p ~/.claude/ccline
tar -xzf ccline-macos-arm64.tar.gz
mv ccline ~/.claude/ccline/ && chmod +x ~/.claude/ccline/ccline
```

</details>

<details>
<summary>Build from source</summary>

```bash
git clone https://github.com/Haleclipse/CCometixLine.git
cd CCometixLine
cargo build --release
mkdir -p ~/.claude/ccline
cp target/release/ccometixline ~/.claude/ccline/ccline
```

</details>

## Set up Claude Code

Run `ccline` in a terminal. The first run walks you through checking your font, picking a preset and pointing Claude Code at ccline; the **Claude Code** page shows the change to `settings.json` before writing it, and keeps a backup.

Or edit `~/.claude/settings.json` yourself:

```json
{
  "statusLine": {
    "type": "command",
    "command": "~/.claude/ccline/ccline",
    "padding": 0
  },
  "subagentStatusLine": {
    "type": "command",
    "command": "~/.claude/ccline/ccline"
  }
}
```

`subagentStatusLine` is optional: it draws ccline's line under each background task. `~` works on Windows too (Claude Code 2.1.47+); with a global npm install, `"command": "ccline"` also works.

## Presets

![Presets](assets/presets.webp)

| Preset | Lines | Look | What it shows |
|---|---|---|---|
| `cometix` (default) | 1 | plain, terminal colors | Classic look: model, directory, git, context and cache |
| `minimal` | 1 | plain, no icons | Model, context, cache and cost |
| `dashboard` | 3 | pill, Catppuccin Mocha | Identity and git; context, its trend, 5-hour pace and cache; code activity |
| `budget` | 2 | plain, Tokyo Night | 5-hour and weekly limits with pace and run-out, spend rates; context, compaction and cache |
| `cache-aware` | 2 | plain, Rosé Pine | The usual line; cache countdown bar, hit ratio, misses and their cause |
| `focus` | 2 | plain, Unicode | A wide context gauge, growth per turn, room before compaction, cache |
| `zen` | 1 | ASCII, no color | Works in any terminal |
| `powerline` | 1 | powerline, Tokyo Night | Powerline segments with a 5-hour gauge |
| `hud` | 3 | plain, terminal colors | Labelled bars: context with used and window size; 5-hour and weekly quotas with countdown and reset time; tools, agents, todos and skills on one line |

Apply one in the configurator: all of it, only its layout, or only its look. Save your own setup as a preset with `N` on the Presets page; it is a file in `~/.claude/ccline/presets/`.

## The configurator

Run `ccline` (or `ccline -c`). Six pages, switched with `1`–`6`; every change shows at once in the preview bar, drawn by the same code as the real status line.

| Page | What it does |
|---|---|
| 1 Home | Checks your setup (Claude Code version, its status line settings, `config.toml`, `models.toml`, updates) with a key to fix each problem, and shows your line with your last real session |
| 2 Presets | The gallery, each card rendered live |
| 3 Layout | Pick metrics from the library, arrange them on lines and sides, and tune the selected item: form, bar glyphs and cells, templates, icon, colors, thresholds |
| 4 Style | Skin, palette, glyphs, color depth, fill, and color overrides on top of the palette |
| 5 Models | The model registry: unrecognized ids from recent sessions, your rules with hit counts, and a tester |
| 6 Claude Code | `statusLine`, `subagentStatusLine` and `refreshInterval`, update checks and the configurator's language, the companion mod |

![Layout page](assets/tui-layout.webp)

The icon picker covers all 10,617 icons of Nerd Fonts 3.5.1, searchable by name, class (`md-`, `fa-`, `cod-` …), codepoint or a pasted glyph.

![Icon picker](assets/tui-icons.webp)

Keys everywhere: `^S` save, `^Z` / `^Y` undo and redo, `^P` command palette, `[` `]` sample session, `-` `+` preview width, `{` color depth, `}` glyphs, `?` help, `q` quit.

The configurator speaks English and Chinese. `language` in `config.toml` picks `en` or `zh`; `auto`, the default, uses Chinese when the first of `LC_ALL`, `LC_MESSAGES` and `LANG` that is set starts with `zh`, and English otherwise. Change it on the Claude Code page, with `L` in the first-run setup, or from `^P`. The status line itself is not translated.

## Metrics

| Category | Metrics |
|---|---|
| Identity | `model` (with reasoning effort and fast mode), `advisor`², `output_style`, `vim`, `agent` |
| Session | `session_name`, `duration`, `session.start`², `session.last_reply`² |
| Context | `context`, `context.trend`, `context.to_compact`, `context.breakdown`¹ |
| Quota | `quota.5h`, `quota.7d`, `quota.spend`, `quota.eta` |
| Cost | `cost`, `cost.rate` (per hour), `cost.today` (all sessions), `cost.week` (since the 7-day window opened), `cost.month` (since the 1st) |
| Cache | `cache` (time until cold, hit ratio, misses), `cache.cold_cost` |
| Code | `dir`, `git`, `repo`, `pr`, `worktree`, `lines` |
| Activity, timing | `activity.tools`², `activity.agents`², `activity.todos`², `activity.skills`², `activity.hooks`², `env`³, `mcp`³, `timing.ttft`¹, `timing.tps`¹ |
| System | `cc_version`, `update` |
| Custom | `cmd`: the first line of any shell command, cached and run in the background |

¹ Needs the companion mod and Claude Code 2.1.287+. `cache` and `quota.spend` need Claude Code 2.1.251+. The configurator marks what your Claude Code can't show and won't let you add it.

² Read from the session transcript, over the whole session. Each render reads at most 8 MiB of new transcript and remembers where it stopped, so a very long transcript catches up over a few renders and later renders read only what was appended. `activity.hooks` covers the hook results since your latest prompt. Without a transcript, `activity.tools` and `activity.agents` show what the companion mod reports.

³ `env` lists the `CLAUDE.md` files, rules and configured hooks of your user and project settings, and the MCP servers. `mcp` lists the enabled MCP servers: those in your user and project configuration, plus the servers in the project's `.mcp.json` that your `settings.json`, `.claude/settings.json` or `.claude/settings.local.json` approve (`enabledMcpjsonServers`, or `enableAllProjectMcpServers`, less `disabledMcpjsonServers`). Windows project paths match however they are written. Servers that plugins provide are not included.

The activity metrics show what Claude is doing now on the left of `⇒` and the session totals on the right:

```
◐ Bash cargo test | ✓ Edit lib.rs ⇒ Edit×1 Read×1
◐ Explore [haiku-4.5 · high] · scan the crate (1m 2s) ⇒ Agent×1 (1 running)
▸ cover the new segment (0/1)
✓ review ⇒ 1 skill
✓ format ⇒ Post×1
```

- `activity.tools`: running tools (`◐`) and the latest finished ones (`✓`, `✗` when one failed) with their target, then the most used tools; `top` sets how many (default 4).
- `activity.agents`: running subagents and those that ended in the last minute, with model, effort, description and run time, then how many ran and how many still run.
- `activity.todos`: the todo in progress (`▸`) and how many are done.
- `activity.skills`: the skill running or last used, and how many skills ran.
- `activity.hooks`: the latest hook result since your last prompt, and counts per hook event.
- `env`: `CLAUDE.md: …`, `rules: …`, `MCPs: …` and `hooks: …`, each only when there is something to list.

Items show only when they have something to say: the cache when Claude Code reports one, the update when a newer ccline is out, `output_style` unless it is the default.

`quota.5h` and `quota.7d` draw a two-row pace bar by default: the upper row is what you have used, the lower row how much of the window has passed. `⇡8%` means you are 8 points ahead of the clock, `⇣` behind, `±0%` on pace.

`cost.today`, `cost.week` and `cost.month` add up the sessions ccline has seen, per local day. The week starts when your 7-day limit window opened (a window opening mid-day counts that whole day), the month on the first local day. `cache.cold_cost` prices the rewarm with Anthropic's official price table, including the 1-hour cache write rate and fast mode.

Background task lines have their own metrics: `task.label`, `task.name`, `task.status`, `task.elapsed` and `task.trend`, next to session metrics such as `model` and `context`.

### Display options

These are set per item under `options`, or in the inspector on the Layout page:

| Metric | Option | Values |
|---|---|---|
| `quota.5h`, `quota.7d`, `quota.spend` | `countdown` | `compact` (`3:42`, `2h07m`, `2d4h`; default), `spaced` (`2h 7m`), `coarse` (`2h`) |
| `quota.5h`, `quota.7d`, `quota.spend` | `reset_format` | A [strftime](https://docs.rs/chrono/latest/chrono/format/strftime/index.html) format for the local reset time, such as `"%-m月%-d日 %H:%M"` |
| `context` | `token_style` | `short` (`1M`, default) or `decimal` (`1.0M`) |
| `dir` | `style`, `depth` | `style = "depth"` shows the last `depth` path components (default 3) |
| `git` | `branch_prefixes` | Shortens branch names: `"feature/=✨, fix/=🐛"` shows `feature/login` as `✨login` (the longest matching prefix wins) |

`model` also has a `fast` field, `{fast}`, shown when fast mode is on (`Opus 5.5 · xHigh · fast`). In terminals that support OSC 8 hyperlinks, the `git` branch links to its page on GitHub or GitLab.

## Configuration

Everything lives in `~/.claude/ccline/` (`CCLINE_HOME` moves it). The configurator writes `config.toml`; you can edit it by hand too:

```toml
preset  = "cometix"
palette = "terminal"
skin    = "plain"
glyphs  = "nerd"
separator = " | "

[[line]]
left = [
  "model",
  { metric = "dir", icon_color = "bright_yellow" },
  "git",
  { metric = "context", widget = "text", format = "{pct}%[ · {tokens}]" },
  "cache",
]
right = ["update"]

[[line]]
left = [{ metric = "quota.5h", cells = 10 }, { metric = "quota.7d", cells = 10 }]
right = [{ metric = "cmd", options = { run = "kubectl config current-context", cache_s = 30 } }]
```

Each `[[line]]` has a `left` and a `right` group; the right one is flush right. An item is a metric id, or a table:

| Key | Meaning |
|---|---|
| `metric` | Metric id |
| `widget` | `text`, `bar`, `gauge`, `sparkline` or `pace` |
| `bar`, `cells` | Bar glyphs (`blocks`, `eighths`, `shapes`, `dots`, `braille`, `ascii`) and length |
| `format`, `compact`, `minimal` | Text templates: `{field}` inserts a value, `[ … ]` drops its contents when a field inside is empty |
| `icon` | Icon override; `""` for none |
| `color`, `fg`, `icon_color` | A palette token (`accent`, `warn`, `cache` …), a color name (`bright_yellow`), `#rrggbb`, `256:n` or `16:n` |
| `levels`, `coloring` | Warning thresholds such as `[[50, "warn"], [80, "crit"]]`; `steps` or `gradient`. For `cache` they are percent of the cache lifetime used up |
| `options` | Metric options, such as `dir`'s `style` or `cmd`'s `run` |

Palettes: `terminal` (follows your terminal theme), `catppuccin-mocha`, `catppuccin-latte`, `dracula`, `nord`, `tokyo-night`, `rose-pine`, `gruvbox`, `anthropic-clay`, and v1's powerline colors as `powerline-dark`, `powerline-light`, `powerline-rose-pine` and `powerline-tokyo-night`. Your own go in `palettes/*.toml`; `[palette_overrides]` changes single tokens. The `terminal` palette shows session items in bright cyan.

The `cache` countdown changes color with the share of the cache lifetime (5 minutes or 1 hour) that is left. With more than a quarter left it keeps the item's color; in the last quarter it turns `warn` (1:15 of 5 minutes, 15:00 of 1 hour), in the last tenth `crit` (0:30, 6:00). A cold cache is shown in the muted color. `levels` moves the thresholds, as percent of the lifetime used up: `levels = [[60, "warn"], [85, "crit"]]`.

A config that fails to parse never blanks the status line: ccline draws the default preset and adds `! config` at the end.

### Third-party models

Claude Code assumes a 200k window for models it doesn't know, so the context percentage of a third-party model can be wrong. `models.toml` sets the right name and window:

```toml
[[models]]
pattern = "kimi-k2"
display_name = "Kimi K2"
context_limit = 256000
match = "contains"          # contains, prefix, exact or regex

[[context_modifiers]]
pattern = "[1m]"
display_suffix = " 1M"
context_limit = 1000000
```

The Models page lists the ids it has seen that nothing recognizes, so you can add a rule with one key.

## Companion mod

On Claude Code 2.1.287+ an optional mod passes ccline what the status line input lacks: time to first token, output speed, running tools and agents, what fills the context, and the exact auto-compact point. It draws no interface and leaves the session alone, with one opt-in exception: idle compaction.

```bash
ccline mod install     # or from the Claude Code page
ccline mod status
ccline mod uninstall
```

Live values need Claude Code to redraw the status line on a timer: set `refreshInterval` to 2 seconds on the Claude Code page.

### Idle compaction

Off by default. When you turn it on, the mod compacts a session that has gone idle, shortly before its 1-hour prompt cache expires, so the next prompt doesn't pay to rewrite a large context from scratch. It acts on the session by calling compact, and only when all of these hold:

- Claude Code's own idle compaction can't run, for example because `CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC` is set (so do `DISABLE_TELEMETRY`, `DO_NOT_TRACK`, `DISABLE_GROWTHBOOK`, a gateway, Bedrock, Vertex or Foundry). When the native one can run, the mod does nothing.
- Auto-compact is on, the session is at a terminal and no turn or tool is running.
- The prompt cache is warm with a 1-hour TTL and expires within `margin_secs`.
- The context holds at least 200k tokens (`CLAUDE_CODE_IDLE_COMPACT_MIN_TOKENS` changes this, with a floor of 100k).

Switch it on from the **Claude Code** page (`idle`, `margin`), or in `config.toml`:

```toml
[idle_compact]
enabled = false      # the mod does nothing until this is true
margin_secs = 180    # compact this long before the cache expires; kept within 30–900
```

## Commands

| Command | |
|---|---|
| `ccline` | Opens the configurator in a terminal; renders the status line when Claude Code pipes it a session |
| `ccline preview` | Renders your config with the built-in sample sessions (`--all`, `-s <name or JSON file>`, `--subagent`, `--list`) |
| `ccline update` | Checks for a newer version now. ccline also checks once a day in the background and shows `update` when one is out; it never downloads anything |
| `ccline mod` | Installs, updates or removes the companion mod |

## Requirements

- Claude Code 2.1.153 or newer; some metrics need newer, see above.
- A [Nerd Font](https://www.nerdfonts.com/) for icons, powerline and pills. Without one, use the `unicode` or `ascii` glyphs: the configurator's font check helps you choose.
- Git, for the `git` metric.

## Upgrading from v1

v2 is a rewrite with a new config format; it doesn't read v1's `config.toml` or themes. Pick a preset in the configurator to start, or `cometix`, the default, which keeps v1's look. `models.toml` carries over unchanged. `--theme` and the Claude Code patcher are gone.

## Development

```bash
cargo build
cargo test             # unit tests and snapshots of every preset in every sample session
cargo build --release
```

## Related projects

- [tweakcc](https://github.com/Piebald-AI/tweakcc): customize Claude Code's themes, thinking verbs and more.

## License

[MIT](LICENSE)

## Star History

[![Star History Chart](https://api.star-history.com/svg?repos=Haleclipse/CCometixLine&type=Date)](https://star-history.com/#Haleclipse/CCometixLine&Date)
