import type { EngineInterface, Register } from 'claude-code'
import { expect, mock, test } from 'claude-code/testing'

const START = 1_000_000
const HOME = '/home/u/.claude/ccline'
type Options = {
  config?: string
  env?: Record<string, string>
  ttl?: string
  warm?: boolean
  expires?: number
  used?: number | null
  auto?: boolean
  surfaces?: string[]
  nativeSetting?: boolean
  reject?: boolean
  unreadable?: boolean
  session?: string
  context?: object
  skip?: boolean
  clockOffset?: number
  savedBridge?: boolean
  idUnreadable?: boolean
}

function setup($: EngineInterface, on: Parameters<Register>[0], options: Options = {}) {
  const clock = mock.clock((event, hook) => {
    if (event === 'clock.now') return on(event, () => ({ value: clock.now() + (options.clockOffset ?? 0) }))
    return on(event, hook)
  }, { now: START })
  let session = options.session ?? 'session-1'
  const attempts: number[] = []
  const writes: string[] = []
  const reads: string[] = []
  on('env.get', (_, e) => ({ value: options.env?.[e.name] }))
  on('settings.read', () => ({ value: { idleCompaction: options.nativeSetting } }))
  on('session.id', () => {
    if (options.idUnreadable) throw new Error('session id unavailable')
    return { value: session }
  })
  on('session.surfaces', () => ({ value: options.surfaces ?? ['terminal'] }))
  on('session.usage', () => ({ value: { context: { breakdown: {
    isAutoCompactEnabled: options.auto ?? true,
    totalTokens: options.used === null ? undefined : options.used ?? 220_000,
    categories: [],
  } } } }))
  on('agent.list', () => ({ value: [] }))
  on('fs.read', (_, e) => {
    reads.push(e.path)
    if (e.path.endsWith('/bridge.json')) {
      if (options.savedBridge === false) throw new Error('no saved bridge')
      return { value: JSON.stringify({ dir: `${HOME}/bridge` }) }
    }
    if (options.unreadable) throw new Error('unreadable')
    if (e.path === `${HOME}/config.toml`) return { value: options.config ?? '[idle_compact]\nenabled = true' }
    if (e.path.includes('/state/sessions/')) return { value: JSON.stringify({ last_input: {
      prompt_cache: { ttl: options.ttl ?? '1h', warm: options.warm ?? true, expires_at: options.expires ?? (START / 1000 + 240) },
      context_window: options.context ?? { current_usage: { input_tokens: 220_000 } },
    } }) }
    throw new Error(`unexpected read: ${e.path}`)
  })
  on('fs.write', (_, e) => {
    writes.push(e.text)
    return { value: undefined }
  })
  on('session.compact', () => {
    attempts.push(clock.now())
    if (options.reject) return Promise.reject(new Error('turn is running'))
    if (options.skip) return { skip: 'compaction was blocked' }
    return { messages: [{ role: 'user', text: 'summary', toolUses: [], toolResults: [] }], tokensBefore: 220_000, tokensAfter: 20_000 }
  })
  on('turn.complete', () => ({ text: 'done' }))
  on('session.measure', () => ({ value: undefined }))
  on('session.end', () => ({ sessionId: session }))
  const complete = (turnId = 't1') => $.turn.complete({ answer: 'done', durationMs: 1_000, isAborted: false, turnId, reason: 'answer' })
  return { clock, attempts, reads, complete, options,
    session: (id: string) => { session = id },
    status: () => JSON.parse(writes[writes.length - 1] ?? '{}').idle_compact,
  }
}

const ELIGIBLE = { DISABLE_TELEMETRY: '1' }

test('enabled and eligible compacts once inside the warm cache window', async ($, on) => {
  const t = setup($, on, { env: ELIGIBLE })
  await t.complete()
  await t.clock.advance(59_999)
  expect(t.attempts).toEqual([])
  await t.clock.advance(1)
  expect(t.attempts).toEqual([START + 60_000])
  await t.clock.advance(300_000)
  expect(t.attempts.length).toBe(1)
  expect(t.status()).toEqual(expect.objectContaining({ state: 'done', at: START + 60_000 }))
})

for (const [name, options] of Object.entries({
  disabled: { config: '[idle_compact]\nenabled = false' },
  'missing table': { config: '[git]\ncache_ms = 2000' },
  'malformed boolean': { config: '[idle_compact]\nenabled = "true"' },
  'malformed margin': { config: '[idle_compact]\nenabled = true\nmargin_secs = "180"' },
  'unreadable inputs': { unreadable: true },
  'native capable': { env: {} },
  'host managed provider': { env: { CLAUDE_CODE_USE_BEDROCK: 'true', CLAUDE_CODE_PROVIDER_MANAGED_BY_HOST: '1' } },
  'false provider flag': { env: { CLAUDE_CODE_USE_VERTEX: '0' } },
  'lineage without a gateway': { env: { CLAUDE_CODE_HOST_GATEWAY_LINEAGE: '1' } },
  '5m TTL': { ttl: '5m' },
  'cold cache': { warm: false },
  'below threshold': { used: 199_999 },
  'measured context overrides state': { used: 10 },
  'expired cache': { expires: START / 1000 - 1 },
  'past last safe second': { expires: START / 1000 + 9 },
  'auto compaction off': { auto: false },
  'explicit auto disable': { env: { ...ELIGIBLE, DISABLE_AUTO_COMPACT: '0' } },
  'native setting off': { nativeSetting: false },
  'no terminal': { surfaces: ['web'] },
} satisfies Record<string, Options>)) {
  test(`${name} does not compact`, async ($, on) => {
    const t = setup($, on, { env: ELIGIBLE, ...options })
    await t.complete()
    await t.clock.advance(300_000)
    expect(t.attempts).toEqual([])
    expect(['off', 'skipped']).toContain(t.status()?.state)
  })
}

test('compact rejection is caught and consumes the attempt until a new turn', async ($, on) => {
  const t = setup($, on, { env: ELIGIBLE, reject: true })
  await t.complete()
  await t.clock.advance(60_200)
  expect(t.attempts.length).toBe(1)
  expect(t.status()?.state).toBe('failed')
  await t.clock.advance(30_000)
  expect(t.attempts.length).toBe(1)
  t.options.reject = false
  await t.complete()
  await t.clock.advance(200)
  expect(t.attempts.length).toBe(2)
  expect(t.status()?.state).toBe('done')
})

test('session change disarms the previous timer even without a hook', async ($, on) => {
  const t = setup($, on, { env: ELIGIBLE })
  await t.complete()
  t.session('session-2')
  await t.clock.advance(300_000)
  expect(t.attempts).toEqual([])
})

test('session end disarms the timer', async ($, on) => {
  const t = setup($, on, { env: ELIGIBLE })
  await t.complete()
  await $.session.end({ reason: 'exit' })
  await t.clock.advance(300_000)
  expect(t.attempts).toEqual([])
  expect(t.status()?.state).toBe('off')
})

test('quoted table and keys, comments and a clamped margin are read from ccline home', async ($, on) => {
  const t = setup($, on, { env: ELIGIBLE, session: '../abc!汉字' + 'x'.repeat(90),
    config: 'palette = "#fff"\n[ "idle_compact" ] # comment\n\'enabled\' = true\n"margin_secs" = 2 # clamps to 30',
  })
  await t.complete()
  await t.clock.advance(209_999)
  expect(t.attempts).toEqual([])
  await t.clock.advance(1)
  expect(t.attempts).toEqual([START + 210_000])
  expect(t.reads).toContain(`${HOME}/state/sessions/${('abc' + 'x'.repeat(90)).slice(0, 80)}.json`)
})

for (const raw of ['999', '100000', 'not-a-number']) {
  test(`token threshold ${raw} follows the minimum and default rules`, async ($, on) => {
    const t = setup($, on, { env: { ...ELIGIBLE, CLAUDE_CODE_IDLE_COMPACT_MIN_TOKENS: raw }, used: 100_000 })
    await t.complete()
    await t.clock.advance(60_000)
    expect(t.attempts.length).toBe(raw === 'not-a-number' ? 0 : 1)
  })
}

test('state context tokens are used when measurement has no token count', async ($, on) => {
  const t = setup($, on, { env: ELIGIBLE, used: null })
  await t.complete()
  await t.clock.advance(60_000)
  expect(t.attempts.length).toBe(1)
})

test('a skipped compaction is recorded without another attempt', async ($, on) => {
  const t = setup($, on, { env: ELIGIBLE, skip: true })
  await t.complete()
  await t.clock.advance(300_000)
  expect(t.attempts.length).toBe(1)
  expect(t.status()?.state).toBe('skipped')
})

test('waking after the window skips until a completed turn re-arms it', async ($, on) => {
  const t = setup($, on, { env: ELIGIBLE })
  await t.complete()
  t.options.clockOffset = 300_000
  await t.clock.advance(15_200)
  expect(t.attempts).toEqual([])
  expect(t.status()?.state).toBe('skipped')
  t.options.expires = (START + 600_000) / 1000
  await t.clock.advance(120_000)
  expect(t.attempts).toEqual([])
  await t.complete('t2')
  expect(t.attempts.length).toBe(1)
})

test('a main-loop request disarms the timer until the turn completes', async ($, on) => {
  const t = setup($, on, { env: ELIGIBLE })
  on('turn.step', async function* (_, e) {
    yield { kind: 'text', index: 0, text: 'done' }
    return { turnId: e.turnId, index: e.index, answer: 'done', toolUses: [], stopReason: 'end_turn' }
  })
  await t.complete()
  const stream = $.turn.step({ turnId: 't2', index: 0, model: 'claude-opus-5-5', messageCount: 1 })
  await stream.next()
  await t.clock.advance(90_000)
  expect(t.attempts).toEqual([])
  await stream.next()
  await t.clock.advance(15_000)
  expect(t.attempts).toEqual([])
  await t.complete('t2')
  expect(t.attempts.length).toBe(1)
})

test('a running tool delays compaction until it returns', async ($, on) => {
  const t = setup($, on, { env: ELIGIBLE })
  let release: () => void = () => {}
  const held = new Promise<void>((resolve) => { release = resolve })
  on('tool.call', async () => {
    await held
    return { result: 'done' }
  })
  await t.complete()
  const tool = $.tool.call({ tool: 'Glob', pattern: '*.json' })
  await t.clock.advance(60_000)
  expect(t.attempts).toEqual([])
  release()
  await tool
  await t.clock.advance(15_000)
  expect(t.attempts.length).toBe(1)
})

for (const name of ['CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC', 'DISABLE_GROWTHBOOK', 'DO_NOT_TRACK',
  'CLAUDE_CODE_USE_BEDROCK', 'CLAUDE_CODE_USE_VERTEX', 'CLAUDE_CODE_USE_FOUNDRY', 'CLAUDE_CODE_USE_GATEWAY']) {
  test(`${name} makes native compaction unavailable`, async ($, on) => {
    const t = setup($, on, { env: { [name]: 'true' } })
    await t.complete()
    await t.clock.advance(60_000)
    expect(t.attempts.length).toBe(1)
  })
}

for (const env of [{ ...ELIGIBLE, CCLINE_HOME: HOME }, { ...ELIGIBLE, HOME: '/home/u' }]) {
  test(`resolves ccline home from ${env.CCLINE_HOME ? 'CCLINE_HOME' : 'HOME'}`, async ($, on) => {
    const t = setup($, on, { env, savedBridge: false })
    await t.complete()
    await t.clock.advance(60_000)
    expect(t.attempts.length).toBe(1)
    expect(t.reads).toContain(`${HOME}/config.toml`)
  })
}

test('the last safe second is included in the compaction window', async ($, on) => {
  const t = setup($, on, { env: ELIGIBLE, expires: START / 1000 + 10 })
  await t.complete()
  expect(t.attempts).toEqual([START])
})

test('large margins clamp to 900 seconds', async ($, on) => {
  const t = setup($, on, { env: ELIGIBLE, expires: START / 1000 + 901,
    config: '[idle_compact]\nenabled = true\nmargin_secs = 1000',
  })
  await t.complete()
  expect(t.attempts).toEqual([])
  await t.clock.advance(15_000)
  expect(t.attempts).toEqual([START + 15_000])
})

test('provider truthy values trim whitespace as Claude Code does', async ($, on) => {
  const t = setup($, on, { env: { CLAUDE_CODE_USE_VERTEX: ' TRUE ' } })
  await t.complete()
  await t.clock.advance(60_000)
  expect(t.attempts.length).toBe(1)
})

test('a nonempty telemetry switch disables flag fetching even when it says zero', async ($, on) => {
  const t = setup($, on, { env: { DISABLE_TELEMETRY: '0' } })
  await t.complete()
  await t.clock.advance(60_000)
  expect(t.attempts.length).toBe(1)
})

for (const failure of ['unreadable', 'empty']) {
  test(`${failure} session id disarms the timer instead of using an old id`, async ($, on) => {
    const t = setup($, on, { env: ELIGIBLE })
    await t.complete()
    if (failure === 'unreadable') t.options.idUnreadable = true
    else t.session('')
    await t.clock.advance(60_200)
    expect(t.attempts).toEqual([])
    expect(t.status()).toEqual(expect.objectContaining({ state: 'off', reason: 'session id unavailable' }))
    t.options.idUnreadable = false
    t.session('session-1')
    await t.clock.advance(30_000)
    expect(t.attempts).toEqual([])
    await t.complete()
    expect(t.attempts.length).toBe(1)
  })
}
