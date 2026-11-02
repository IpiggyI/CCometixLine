// The ccline companion mod watches the
// session from inside Claude Code and writes what the status line's JSON
// lacks to `<ccline home>/bridge/<session id>.json`, which ccline reads when
// it renders. When explicitly enabled in ccline's config, it also compacts
// idle terminal sessions whose native idle compaction cannot run. It draws
// no UI; every hook passes its event on untouched.
//
// Written by ccline from the copy built into it. Format: src/state/bridge.rs
// in CCometixLine.

import type { EngineInterface, Register } from 'claude-code'

type $ = EngineInterface

/** Bridge format version; ccline ignores files with another. */
const VERSION = 1
/** Events within this many milliseconds share one write. */
const FLUSH_MS = 200
/** A response that streamed for less than this gives no speed. */
const MIN_STREAM_MS = 100

type Category = { name: string; kind: string; tokens: number }
type Context = {
  threshold?: number
  auto: boolean
  window?: number
  used?: number
  categories: Category[]
}
/** The main loop's last model request, and how long its last turn took. */
type Turn = {
  model?: string
  ttft_ms?: number
  tps?: number
  /** The last finished turn's wall-clock length. */
  duration_ms?: number
  /** The last request's. */
  output_tokens?: number
  ended_at?: number
}
type Tool = { name: string; since: number; agent?: string }
type Agent = { id: string; type: string; status: string; description?: string }
type Chunk = { kind?: string; usage?: { output_tokens?: number; model?: string } | null }
// Optional extension to v1; the Rust bridge reader ignores unknown fields.
type IdleCompact = { state: 'off' | 'armed' | 'done' | 'skipped' | 'failed'; at: number; reason: string }

/** What the bridge holds, kept between events. */
const live = {
  dir: undefined as string | undefined,
  session: undefined as string | undefined,
  context: undefined as Context | undefined,
  turn: undefined as Turn | undefined,
  agents: [] as Agent[],
  tools: new Map<string, Tool>(),
  pending: false,
  seq: 0,
  idle: undefined as IdleCompact | undefined,
  idleGeneration: 0,
  running: undefined as string | undefined,
  ended: false,
}

// Where to write: the path the installer left beside the mod, else the
// home ccline uses by default.
async function where($: $): Promise<string | undefined> {
  if (live.dir !== undefined) return live.dir
  try {
    const saved = JSON.parse(String(await $.fs.read(`${$.plugin.root}/bridge.json`)))
    if (typeof saved?.dir === 'string' && saved.dir !== '') live.dir = saved.dir
  } catch {}
  if (live.dir === undefined) {
    const own = await $.env.get('CCLINE_HOME')
    const home = (await $.env.get('HOME')) ?? (await $.env.get('USERPROFILE'))
    if (own) live.dir = `${own}/bridge`
    else if (home) live.dir = `${home}/.claude/ccline/bridge`
  }
  return live.dir
}

// Forget what the bridge held for another session.
function reset(session: string) {
  live.idleGeneration++
  live.idle = undefined
  live.running = undefined
  live.ended = false
  live.session = session
  live.context = undefined
  live.turn = undefined
  live.agents = []
  live.tools.clear()
}

// The session can change under a running process (/clear, /resume), and
// each one's figures are its own: start over when the id moves.
async function follow($: $) {
  try {
    const id = await $.session.id()
    if (id && id !== live.session) reset(id)
    return Boolean(id)
  } catch {
    return false
  }
}

async function write($: $) {
  try {
    const to = await where($)
    live.session ??= await $.session.id()
    if (to === undefined || !live.session) return
    const doc = {
      v: VERSION,
      session: live.session,
      updated_at: await $.clock.now(),
      context: live.context,
      turn: live.turn,
      tools: [...live.tools.values()],
      agents: live.agents,
      idle_compact: live.idle,
    }
    await $.fs.write(`${to}/${live.session}.json`, JSON.stringify(doc))
  } catch {}
}

// Coalesce a burst of events into one write shortly after.
function flush($: $) {
  if (live.pending) return
  live.pending = true
  try {
    $.clock.after(FLUSH_MS, () => {
      live.pending = false
      void write($)
    })
  } catch {
    live.pending = false
  }
}

async function measure($: $) {
  try {
    const usage = await $.session.usage({ breakdown: 'summary' })
    const b = usage.context.breakdown
    if (b === undefined) return
    live.context = {
      threshold: b.autoCompactThreshold,
      auto: b.isAutoCompactEnabled,
      window: b.rawMaxTokens,
      used: b.totalTokens,
      categories: b.categories.map((c) => ({ name: c.name, kind: c.kind, tokens: c.tokens })),
    }
  } catch {}
}

async function listAgents($: $) {
  try {
    live.agents = (await $.agent.list()).map((a) => ({
      id: a.id,
      type: a.type,
      status: a.status,
      description: a.description,
    }))
  } catch {}
}

async function now($: $): Promise<number> {
  return $.clock.now()
}

// Strip TOML comments without treating a quoted '#' as a comment.
function uncomment(line: string): string | undefined {
  let quote = ''
  for (let i = 0; i < line.length; i++) {
    const c = line[i]
    if (quote === '"' && c === '\\') i++
    else if (quote && c === quote) quote = ''
    else if (!quote && (c === '"' || c === "'")) quote = c
    else if (!quote && c === '#') return line.slice(0, i).trim()
  }
  return quote ? undefined : line.trim()
}

// Read only this small table, leaving the rest of ccline's TOML alone.
function idleConfig(text: string): { enabled: boolean; margin: number } | undefined {
  const config = { enabled: false, margin: 180 }
  let inside = false
  const seen = new Set<string>()
  for (const raw of text.split(/\r?\n/)) {
    const line = uncomment(raw)
    if (line?.startsWith('[')) {
      inside = /^\[\s*(?:idle_compact|"idle_compact"|'idle_compact')\s*\]$/.test(line)
      continue
    }
    if (!inside || line === '') continue
    const pair = line?.match(/^(?:([\w-]+)|"([^"\\]+)"|'([^']+)')\s*=\s*(.*?)\s*$/)
    if (!pair) return
    const key = pair[1] ?? pair[2] ?? pair[3]
    if (key !== 'enabled' && key !== 'margin_secs') continue
    if (seen.has(key)) return
    seen.add(key)
    if (key === 'enabled') {
      if (!/^(true|false)$/.test(pair[4])) return
      config.enabled = pair[4] === 'true'
    } else {
      if (!/^[+-]?\d(?:_?\d)*$/.test(pair[4])) return
      const margin = Number(pair[4].replace(/_/g, ''))
      if (!Number.isFinite(margin)) return
      config.margin = Math.max(30, Math.min(900, margin))
    }
  }
  return config
}

const truthy = (value: string | undefined) => /^(1|true|yes|on)$/i.test(value?.trim() ?? '')

async function flagsUnavailable($: $): Promise<boolean> {
  if (await $.env.get('CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC') ||
      await $.env.get('DISABLE_GROWTHBOOK') || await $.env.get('DISABLE_TELEMETRY') ||
      await $.env.get('DO_NOT_TRACK')) return true
  if (truthy(await $.env.get('CLAUDE_CODE_USE_GATEWAY'))) return true
  if (await $.env.get('CLAUDE_CODE_PROVIDER_MANAGED_BY_HOST')) return false
  return truthy(await $.env.get('CLAUDE_CODE_USE_BEDROCK')) ||
    truthy(await $.env.get('CLAUDE_CODE_USE_VERTEX')) || truthy(await $.env.get('CLAUDE_CODE_USE_FOUNDRY'))
}

function stateTokens(context: any): number | undefined {
  const usage = context?.current_usage
  if (usage && typeof usage.input_tokens === 'number') {
    return usage.input_tokens + (usage.cache_creation_input_tokens ?? 0) + (usage.cache_read_input_tokens ?? 0)
  }
  if (typeof context?.used_percentage === 'number' && typeof context.context_window_size === 'number') {
    return context.used_percentage * context.context_window_size / 100
  }
  return context?.total_input_tokens
}

async function idleWindow($: $): Promise<string | { expires: number; margin: number }> {
  const bridge = await where($)
  if (!bridge) return 'ccline home unavailable'
  const home = bridge.replace(/[\\/]+$/, '').replace(/[\\/][^\\/]+$/, '')
  const config = idleConfig(String(await $.fs.read(`${home}/config.toml`)))
  if (!config?.enabled) return 'disabled in ccline config'
  if (await $.env.get('DISABLE_AUTO_COMPACT')) return 'DISABLE_AUTO_COMPACT is set'
  if (!live.context?.auto) return 'auto compaction is off or unknown'
  if (await $.settings.read({ key: 'idleCompaction' }) === false) return 'idleCompaction is false'
  if (!(await $.session.surfaces()).includes('terminal')) return 'no terminal surface'
  if (!await flagsUnavailable($)) return 'native idle compaction is available'
  const id = live.session?.replace(/[^A-Za-z0-9_-]/g, '').slice(0, 80)
  if (!id) return 'no safe session id'
  const input = JSON.parse(String(await $.fs.read(`${home}/state/sessions/${id}.json`)))?.last_input
  const cache = input?.prompt_cache
  if (cache?.ttl !== '1h' || cache.warm !== true) return 'cache is not warm with a 1h TTL'
  if (typeof cache.expires_at !== 'number' || !Number.isFinite(cache.expires_at)) return 'cache expiry unavailable'
  const raw = await $.env.get('CLAUDE_CODE_IDLE_COMPACT_MIN_TOKENS')
  const threshold = raw?.trim() && Number.isFinite(Number(raw)) ? Math.max(100_000, Number(raw)) : 200_000
  const used = live.context.used ?? stateTokens(input?.context_window)
  if (typeof used !== 'number' || !Number.isFinite(used) || used < threshold) return 'context below idle threshold'
  return { expires: cache.expires_at * 1000, margin: config.margin * 1000 }
}

async function idleStatus($: $, state: IdleCompact['state'], reason: string) {
  const generation = live.idleGeneration
  const at = await now($)
  if (generation !== live.idleGeneration) return
  live.idle = { state, at, reason }
  flush($)
}

async function compactIdle($: $, at: number) {
  // Disarm before invoking compact: a rejection also consumes this attempt.
  const attempt = ++live.idleGeneration
  live.idle = { state: 'armed', at, reason: 'compaction requested' }
  flush($)
  try {
    const result = await $.session.compact()
    if (attempt !== live.idleGeneration) return
    await idleStatus($, result?.skip ? 'skipped' : 'done', result?.skip ?? 'idle compaction completed')
  } catch {
    if (attempt === live.idleGeneration) await idleStatus($, 'failed', 'session.compact rejected')
  }
}

// Generation checks invalidate both queued timers and work awaiting host calls.
async function checkIdle($: $, generation: number) {
  const current = () => generation === live.idleGeneration && !live.ended
  try {
    const known = await follow($)
    if (!current()) return
    if (!known) return await idleStatus($, 'off', 'session id unavailable')
    const window = await idleWindow($)
    const confirmed = await follow($)
    if (!current()) return
    if (!confirmed) return await idleStatus($, 'off', 'session id unavailable')
    if (typeof window === 'string') return await idleStatus($, 'off', window)
    const at = await now($)
    if (!current()) return
    if (at > window.expires - 10_000) return await idleStatus($, 'skipped', 'cache compaction window missed')
    if (live.running !== undefined || live.tools.size > 0 || at < window.expires - window.margin) {
      await idleStatus($, 'armed', 'waiting for an idle warm cache window')
      $.clock.after(Math.min(15_000, window.expires - 10_000 - at || 15_000), () => checkIdle($, generation))
      return
    }
    await compactIdle($, at)
  } catch {
    if (current()) await idleStatus($, 'off', 'idle compaction inputs unreadable')
  }
}

function turnStarted(turn: string) {
  live.running = turn
  live.idleGeneration++
}

// One main-loop request is done: its time to first token and its speed,
// shown while the turn goes on rather than once it ends.
function timed(sent: number, first: number, last: number, output?: number, model?: string) {
  const streamed = last - first
  live.turn = {
    ...live.turn,
    model: model ?? live.turn?.model,
    ttft_ms: first - sent,
    tps: output && streamed >= MIN_STREAM_MS ? (output * 1000) / streamed : live.turn?.tps,
    output_tokens: output,
    ended_at: last,
  }
}

export const register: Register = (on) => {
  live.dir = undefined
  live.pending = false
  live.idleGeneration++
  live.idle = undefined
  live.running = undefined
  live.ended = false

  on('session.start', async ($, e, next) => {
    const started = await next(e)
    try {
      await follow($)
      await measure($)
      await write($)
    } catch {}
    return started
  })

  on('turn.start', async ($, e, next) => {
    if (e.agentId === undefined) {
      turnStarted(e.turnId)
      await follow($)
      live.running = e.turnId
    }
    return next(e)
  })

  // After each main-thread turn, and when a rate limit moves.
  on('session.measure', async ($, e, next) => {
    const echoed = await next(e)
    await follow($)
    if (e.changed.includes('context')) {
      await measure($)
      flush($)
    }
    return echoed
  })

  // Time each model request on the main loop as Claude Code does: from
  // sending it to the first thing back (the response's envelope), then the
  // output its stop counted over the time it streamed, thinking included.
  // Subagents' requests pass straight through.
  on('turn.step', async function* ($, e, next) {
    if (e.agentId !== undefined) return yield* next(e)
    turnStarted(e.turnId)
    await follow($)
    live.running = e.turnId
    const sent = await now($)
    let first: number | undefined
    let output: number | undefined
    let model: string | undefined
    const stream = next(e)
    let ended = false
    try {
      for (;;) {
        const piece = await stream.next()
        if (piece.done) {
          ended = true
          try {
            if (first !== undefined) {
              const usage = piece.value?.usage
              timed(sent, first, await now($), output ?? usage?.output_tokens, model ?? usage?.model)
              flush($)
            }
          } catch {}
          return piece.value
        }
        const c = piece.value as Chunk
        if (first === undefined) first = await now($)
        if (c.kind === 'stop' && c.usage) {
          output = c.usage.output_tokens
          model = c.usage.model
        }
        yield piece.value
      }
    } finally {
      // Left early (the answer was interrupted): close the stream beneath,
      // as a for-await loop would.
      if (!ended) await stream.return?.(undefined as never)
    }
  })

  on('turn.complete', async ($, e, next) => {
    const answered = await next(e)
    try {
      await follow($)
      if (e.agentId === undefined) {
        live.turn = { ...live.turn, duration_ms: e.durationMs, ended_at: await now($) }
        if (live.running === undefined || live.running === e.turnId) {
          live.running = undefined
          await measure($)
          await checkIdle($, ++live.idleGeneration)
        }
      }
      await listAgents($)
      flush($)
    } catch {}
    return answered
  })

  // Tools running now, the main loop's and the agents'.
  on('tool.call', async ($, e, next) => {
    const key = e.tool_use_id ?? `ccline-${live.seq++}`
    try {
      await follow($)
      live.tools.set(key, { name: e.tool, since: await now($), agent: e.agentId })
      flush($)
    } catch {}
    try {
      return await next(e)
    } finally {
      live.tools.delete(key)
      flush($)
    }
  })

  on('agent.spawn', async ($, e, next) => {
    const spawned = await next(e)
    await follow($)
    await listAgents($)
    flush($)
    return spawned
  })

  on('session.end', async ($, e, next) => {
    live.ended = true
    live.idleGeneration++
    await idleStatus($, 'off', 'session ended')
    live.tools.clear()
    live.agents = []
    await write($)
    return next(e)
  })
}
