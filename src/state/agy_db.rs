//! Read-only reader for Antigravity CLI's per-conversation SQLite database
//! (`~/.gemini/antigravity-cli/conversations/<id>.db`), extracting protobuf
//! `steps` and `gen_metadata` for session cost, TTFT, output speed (`tok/s`),
//! and underlying API model id.

use crate::models::builtin;
use rusqlite::{Connection, OpenFlags};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::time::{Duration, SystemTime};

#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq)]
pub struct DbSummary {
    pub api_model: Option<String>,
    pub display_model: Option<String>,
    pub ttft_ms: Option<u64>,
    pub tps: Option<f64>,
    pub total_cost_usd: Option<f64>,
    pub first_call_at: Option<i64>,
    pub input_tokens: u64,
    pub cache_read_tokens: u64,
    pub cache_write_tokens: u64,
    pub output_tokens: u64,
    pub last_input_tokens: u64,
    pub last_cache_read_tokens: u64,
    pub last_cache_write_tokens: u64,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
struct DbStamp {
    db_len: u64,
    db_mtime: Duration,
    wal_len: u64,
    wal_mtime: Duration,
}

impl DbStamp {
    fn at(db_path: &Path) -> Option<Self> {
        let meta = db_path.metadata().ok()?;
        meta.is_file().then_some(())?;
        let db_mtime = meta
            .modified()
            .ok()?
            .duration_since(SystemTime::UNIX_EPOCH)
            .ok()?;
        let wal_path = format!("{}-wal", db_path.display());
        let (wal_len, wal_mtime) = Path::new(&wal_path)
            .metadata()
            .ok()
            .and_then(|w| {
                let m = w
                    .modified()
                    .ok()?
                    .duration_since(SystemTime::UNIX_EPOCH)
                    .ok()?;
                Some((w.len(), m))
            })
            .unwrap_or((0, Duration::ZERO));
        Some(DbStamp {
            db_len: meta.len(),
            db_mtime,
            wal_len,
            wal_mtime,
        })
    }
}

#[derive(Deserialize, Serialize)]
struct CachedDb {
    stamp: DbStamp,
    summary: DbSummary,
}

/// Read the conversation database for `conv_id` under `agy_cli_dir()/conversations/`.
pub fn read(conv_id: &str, now: i64) -> Option<DbSummary> {
    let safe = super::safe_name(conv_id)?;
    let db_path = crate::paths::agy_cli_dir()
        .join("conversations")
        .join(format!("{safe}.db"));
    read_at(&db_path, &super::dir().join("conv-db-v1"), now)
}

pub fn read_at(db_path: &Path, cache_dir: &Path, now: i64) -> Option<DbSummary> {
    let stamp = DbStamp::at(db_path)?;
    let cache_path = cache_dir.join(format!(
        "{}.json",
        super::file_key(&db_path.to_string_lossy())
    ));
    let saved: Option<CachedDb> = super::read_json(&cache_path);
    if let Some(ref c) = saved
        && c.stamp == stamp
    {
        return Some(c.summary.clone());
    }
    match parse_db(db_path, now) {
        Some(summary) => {
            let _ = super::write_json(
                &cache_path,
                &CachedDb {
                    stamp,
                    summary: summary.clone(),
                },
            );
            super::sometimes(100, || {
                super::sweep(cache_dir, Duration::from_secs(7 * 86_400), 200);
            });
            Some(summary)
        }
        // If SQLite is momentarily locked/busy, reuse last cached summary.
        None => saved.map(|c| c.summary),
    }
}

fn parse_db(db_path: &Path, now: i64) -> Option<DbSummary> {
    let conn = Connection::open_with_flags(
        db_path,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .ok()?;
    let _ = conn.busy_timeout(Duration::from_millis(50));

    let mut step_at: HashMap<i64, i64> = HashMap::new();
    let mut exec_at: HashMap<String, i64> = HashMap::new();

    if let Ok(mut stmt) = conn.prepare("SELECT idx, metadata FROM steps")
        && let Ok(rows) = stmt.query_map([], |row| {
            let idx: i64 = row.get(0)?;
            let meta: Option<Vec<u8>> = row.get(1)?;
            Ok((idx, meta))
        })
    {
        for row in rows {
            let (idx, Some(meta)) = row.ok()? else {
                continue;
            };
            let Some(ts_bytes) = pb_get_bytes(&meta, 1) else {
                continue;
            };
            let Some(sec) = pb_get_varint(ts_bytes, 1).filter(|&s| s > 0) else {
                continue;
            };
            let sec = sec as i64;
            step_at.insert(idx, sec);
            if let Some(exec) = pb_get_bytes(&meta, 12)
                && let Ok(id) = std::str::from_utf8(exec)
                && !id.is_empty()
            {
                exec_at.entry(id.to_string()).or_insert(sec);
            }
        }
    }

    let mut stmt = conn
        .prepare("SELECT idx, data FROM gen_metadata ORDER BY idx ASC")
        .ok()?;
    let rows = stmt
        .query_map([], |row| {
            let data: Option<Vec<u8>> = row.get(1)?;
            Ok(data)
        })
        .ok()?;

    let mut summary = DbSummary::default();
    let mut seen_requests: HashSet<String> = HashSet::new();
    let mut priced_cost = 0.0_f64;
    let mut any_priced = false;

    for row in rows {
        let Some(data) = row.ok()? else {
            continue;
        };
        let Some(cm) = pb_get_bytes(&data, 1) else {
            continue;
        };
        let Some(u) = pb_get_bytes(cm, 4) else {
            continue;
        };
        let inp = pb_get_varint(u, 2).unwrap_or(0);
        let cw = pb_get_varint(u, 4).unwrap_or(0);
        let cr = pb_get_varint(u, 5).unwrap_or(0);
        let think = pb_get_varint(u, 9).unwrap_or(0);
        let resp = pb_get_varint(u, 10).unwrap_or(0);
        let out = pb_get_varint(u, 3).unwrap_or(0).max(think + resp);
        if inp == 0 && cr == 0 && cw == 0 && out == 0 {
            continue;
        }

        let exec_id = pb_get_bytes(data.as_slice(), 4)
            .and_then(|b| std::str::from_utf8(b).ok())
            .unwrap_or("");
        let dedupe_key = pb_get_bytes(u, 11)
            .and_then(|b| std::str::from_utf8(b).ok())
            .filter(|s| !s.is_empty())
            .map(str::to_string)
            .unwrap_or_else(|| format!("{exec_id}:{inp}:{cr}:{out}"));
        if !seen_requests.insert(dedupe_key) {
            continue;
        }

        let step_ts = pb_get_bytes(&data, 2)
            .into_iter()
            .flat_map(pb_varints)
            .find_map(|idx| step_at.get(&(idx as i64)).copied())
            .or_else(|| exec_at.get(exec_id).copied())
            .unwrap_or(now);

        summary.first_call_at = Some(
            summary
                .first_call_at
                .map_or(step_ts, |first| first.min(step_ts)),
        );
        summary.input_tokens += inp;
        summary.cache_read_tokens += cr;
        summary.cache_write_tokens += cw;
        summary.output_tokens += out;
        summary.last_input_tokens = inp;
        summary.last_cache_read_tokens = cr;
        summary.last_cache_write_tokens = cw;

        let api_model = pb_get_bytes(cm, 19)
            .and_then(|b| std::str::from_utf8(b).ok())
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty());
        let display_model = pb_get_bytes(cm, 21)
            .and_then(|b| std::str::from_utf8(b).ok())
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty());

        if let Some(ref model_name) = api_model
            && let Some(p) = builtin::price_at(model_name, false, inp + cr + cw, step_ts)
        {
            any_priced = true;
            priced_cost += (inp as f64 * p.input
                + cr as f64 * p.cache_read
                + cw as f64 * p.cache_write_5m
                + out as f64 * p.output)
                / 1_000_000.0;
        }

        if api_model.is_some() {
            summary.api_model = api_model;
        }
        if display_model.is_some() {
            summary.display_model = display_model;
        }

        let ttft_s = pb_get_bytes(cm, 11).and_then(pb_duration_secs);
        let stream_s = pb_get_bytes(cm, 12).and_then(pb_duration_secs);
        summary.ttft_ms = ttft_s
            .filter(|t| *t > 0.0)
            .map(|t| (t * 1000.0).round() as u64);

        let ttft_v = ttft_s.unwrap_or(0.0).max(0.0);
        let stream_v = stream_s.unwrap_or(0.0).max(0.0);
        let denom = if think > 0 {
            ttft_v + stream_v
        } else if stream_v > 0.05 {
            stream_v
        } else {
            ttft_v + stream_v
        };
        summary.tps = (out > 0 && denom > 0.05).then(|| out as f64 / denom);
    }

    if any_priced {
        summary.total_cost_usd = Some(priced_cost);
    }
    Some(summary)
}

enum WireVal<'a> {
    Varint(u64),
    Bytes(&'a [u8]),
}

fn read_varint(buf: &[u8], pos: &mut usize) -> Option<u64> {
    let mut x = 0_u64;
    let mut shift = 0_u32;
    while *pos < buf.len() {
        let b = buf[*pos];
        *pos += 1;
        if shift < 64 {
            x |= u64::from(b & 0x7f) << shift;
        }
        if b & 0x80 == 0 {
            return Some(x);
        }
        shift += 7;
        if shift > 70 {
            return None;
        }
    }
    None
}

fn pb_get<'a>(buf: &'a [u8], field: u32) -> Option<WireVal<'a>> {
    let mut pos = 0;
    while pos < buf.len() {
        let key = read_varint(buf, &mut pos)?;
        let f = (key >> 3) as u32;
        let wt = (key & 7) as u8;
        if f == 0 {
            return None;
        }
        let val = match wt {
            0 => WireVal::Varint(read_varint(buf, &mut pos)?),
            2 => {
                let len = read_varint(buf, &mut pos)? as usize;
                let end = pos.checked_add(len)?;
                if end > buf.len() {
                    return None;
                }
                let slice = &buf[pos..end];
                pos = end;
                WireVal::Bytes(slice)
            }
            1 => {
                pos = pos.checked_add(8)?;
                if pos > buf.len() {
                    return None;
                }
                continue;
            }
            5 => {
                pos = pos.checked_add(4)?;
                if pos > buf.len() {
                    return None;
                }
                continue;
            }
            _ => return None,
        };
        if f == field {
            return Some(val);
        }
    }
    None
}

fn pb_get_varint(buf: &[u8], field: u32) -> Option<u64> {
    match pb_get(buf, field)? {
        WireVal::Varint(v) => Some(v),
        WireVal::Bytes(_) => None,
    }
}

fn pb_get_bytes(buf: &[u8], field: u32) -> Option<&[u8]> {
    match pb_get(buf, field)? {
        WireVal::Bytes(b) => Some(b),
        WireVal::Varint(_) => None,
    }
}

fn pb_varints(buf: &[u8]) -> Vec<u64> {
    let mut out = Vec::new();
    let mut pos = 0;
    while pos < buf.len() {
        let Some(v) = read_varint(buf, &mut pos) else {
            break;
        };
        out.push(v);
    }
    out
}

fn pb_duration_secs(buf: &[u8]) -> Option<f64> {
    let secs = pb_get_varint(buf, 1).unwrap_or(0) as f64;
    let nanos = pb_get_varint(buf, 2).unwrap_or(0) as f64;
    Some(secs + nanos / 1_000_000_000.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn encode_varint(mut v: u64, out: &mut Vec<u8>) {
        loop {
            let mut b = (v & 0x7f) as u8;
            v >>= 7;
            if v != 0 {
                b |= 0x80;
            }
            out.push(b);
            if v == 0 {
                break;
            }
        }
    }

    fn pb_field_varint(field: u32, v: u64, out: &mut Vec<u8>) {
        encode_varint(u64::from(field) << 3, out);
        encode_varint(v, out);
    }

    fn pb_field_bytes(field: u32, bytes: &[u8], out: &mut Vec<u8>) {
        encode_varint((u64::from(field) << 3) | 2, out);
        encode_varint(bytes.len() as u64, out);
        out.extend_from_slice(bytes);
    }

    fn make_duration(secs: u64, nanos: u64) -> Vec<u8> {
        let mut b = Vec::new();
        pb_field_varint(1, secs, &mut b);
        pb_field_varint(2, nanos, &mut b);
        b
    }

    struct GenRowSpec<'a> {
        exec: &'a str,
        step_idx: u64,
        model: &'a str,
        inp: u64,
        cr: u64,
        out_tok: u64,
        think_tok: u64,
        rid: &'a str,
        ttft_s: u64,
        stream_s: u64,
    }

    fn make_gen_row(spec: GenRowSpec<'_>) -> Vec<u8> {
        let mut usage = Vec::new();
        pb_field_varint(2, spec.inp, &mut usage);
        pb_field_varint(3, spec.out_tok, &mut usage);
        pb_field_varint(5, spec.cr, &mut usage);
        if spec.think_tok > 0 {
            pb_field_varint(9, spec.think_tok, &mut usage);
            pb_field_varint(10, spec.out_tok.saturating_sub(spec.think_tok), &mut usage);
        }
        pb_field_bytes(11, spec.rid.as_bytes(), &mut usage);

        let mut cm = Vec::new();
        pb_field_bytes(4, &usage, &mut cm);
        pb_field_bytes(11, &make_duration(spec.ttft_s, 0), &mut cm);
        pb_field_bytes(12, &make_duration(spec.stream_s, 0), &mut cm);
        pb_field_bytes(19, spec.model.as_bytes(), &mut cm);
        pb_field_bytes(21, b"Gemini 4 Carbon (High)", &mut cm);

        let mut packed_steps = Vec::new();
        encode_varint(spec.step_idx, &mut packed_steps);

        let mut row = Vec::new();
        pb_field_bytes(1, &cm, &mut row);
        pb_field_bytes(2, &packed_steps, &mut row);
        pb_field_bytes(4, spec.exec.as_bytes(), &mut row);
        row
    }

    #[test]
    fn decodes_sqlite_calls_dedupes_and_computes_cost_ttft_tps() {
        let dir = std::env::temp_dir().join(format!("agyline-db-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let db = dir.join("conv.db");
        let cache = dir.join("cache");

        let conn = Connection::open(&db).unwrap();
        conn.execute_batch(
            "CREATE TABLE steps (idx INTEGER PRIMARY KEY, metadata BLOB);
             CREATE TABLE gen_metadata (idx INTEGER PRIMARY KEY, data BLOB);",
        )
        .unwrap();

        let mut ts = Vec::new();
        pb_field_varint(1, 1_795_677_800, &mut ts);
        let mut step_meta = Vec::new();
        pb_field_bytes(1, &ts, &mut step_meta);
        pb_field_bytes(12, b"exec-1", &mut step_meta);
        conn.execute(
            "INSERT INTO steps (idx, metadata) VALUES (1, ?1)",
            [&step_meta],
        )
        .unwrap();

        // Call 1: 1M input ($2) + 1M cache_read ($0.10) + 100k output ($1) = $3.10.
        let r1 = make_gen_row(GenRowSpec {
            exec: "exec-1",
            step_idx: 1,
            model: "gemini-4-carbon",
            inp: 1_000_000,
            cr: 1_000_000,
            out_tok: 100_000,
            think_tok: 20_000,
            rid: "req-1",
            ttft_s: 2,
            stream_s: 8,
        });
        // Duplicate req-1 must be ignored.
        let dup = r1.clone();
        conn.execute("INSERT INTO gen_metadata (idx, data) VALUES (1, ?1)", [&r1])
            .unwrap();
        conn.execute(
            "INSERT INTO gen_metadata (idx, data) VALUES (2, ?1)",
            [&dup],
        )
        .unwrap();
        drop(conn);

        let sum = read_at(&db, &cache, 1_795_677_900).unwrap();
        assert_eq!(sum.api_model.as_deref(), Some("gemini-4-carbon"));
        assert_eq!(sum.ttft_ms, Some(2000));
        // With thinking tokens: 100_000 / (2 + 8) = 10_000 tok/s
        assert!((sum.tps.unwrap() - 10_000.0).abs() < 0.01);
        assert!((sum.total_cost_usd.unwrap() - 3.1).abs() < 1e-6);
        assert_eq!(sum.input_tokens, 1_000_000);
        assert_eq!(sum.cache_read_tokens, 1_000_000);

        std::fs::remove_dir_all(&dir).unwrap();
    }
}
