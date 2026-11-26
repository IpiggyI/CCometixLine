//! Timezone configuration, parsing and resolution (`auto`, IANA names,
//! common aliases, and UTC offsets).

use chrono::{DateTime, Datelike, FixedOffset, NaiveDate};
use serde::{Deserialize, Deserializer, Serialize, Serializer};

/// Common timezones offered for quick cycling and in the TUI timezone picker.
pub const QUICK_TIMEZONES: &[(&str, &str)] = &[
    ("auto", "Follow system local timezone"),
    ("Asia/Shanghai", "Beijing / China (UTC+08:00)"),
    ("Asia/Hong_Kong", "Hong Kong (UTC+08:00)"),
    ("Asia/Taipei", "Taipei (UTC+08:00)"),
    ("Asia/Singapore", "Singapore (UTC+08:00)"),
    ("Asia/Tokyo", "Tokyo (UTC+09:00)"),
    ("Asia/Seoul", "Seoul (UTC+09:00)"),
    ("Europe/London", "London (UTC+00:00 / +01:00)"),
    ("Europe/Berlin", "Berlin / Paris (UTC+01:00 / +02:00)"),
    ("America/New_York", "New York (UTC-05:00 / -04:00)"),
    ("America/Los_Angeles", "Los Angeles (UTC-08:00 / -07:00)"),
    ("UTC", "Coordinated Universal Time (UTC+00:00)"),
];

/// Configured timezone in `config.toml`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum Timezone {
    #[default]
    Auto,
    Custom(String),
}

impl Timezone {
    /// Parse a user-provided timezone specification for interactive editing or
    /// CLI flags. Empty or `"auto"` becomes `Timezone::Auto`.
    pub fn parse(spec: &str) -> Result<Self, &'static str> {
        let trimmed = spec.trim();
        if trimmed.is_empty() || trimmed.eq_ignore_ascii_case("auto") {
            return Ok(Timezone::Auto);
        }
        let now = chrono::Utc::now().timestamp();
        if resolve_spec(trimmed, now).is_some() {
            Ok(Timezone::Custom(trimmed.to_string()))
        } else {
            Err("Use auto, an IANA name like Asia/Shanghai, or an offset like +08:00 or UTC+8.")
        }
    }

    pub fn is_auto(&self) -> bool {
        matches!(self, Timezone::Auto)
    }

    pub fn as_str(&self) -> &str {
        match self {
            Timezone::Auto => "auto",
            Timezone::Custom(s) => s.as_str(),
        }
    }

    /// The invalid raw specification if this setting cannot be resolved.
    pub fn invalid_spec(&self) -> Option<&str> {
        match self {
            Timezone::Auto => None,
            Timezone::Custom(s) => resolve_spec(s, 0).is_none().then_some(s.as_str()),
        }
    }

    /// Resolve to a concrete offset at `now`, falling back to `fallback` for
    /// `Auto` or an invalid specification.
    pub fn resolve(&self, now: i64, fallback: FixedOffset) -> FixedOffset {
        match self {
            Timezone::Auto => fallback,
            Timezone::Custom(s) => resolve_spec(s, now).unwrap_or(fallback),
        }
    }
}

impl Serialize for Timezone {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(self.as_str())
    }
}

/// Keep unknown values as `Custom` so a typo never wipes the rest of
/// `config.toml`; callers still fall back safely and can surface a warning.
impl<'de> Deserialize<'de> for Timezone {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let raw = String::deserialize(d)?;
        let trimmed = raw.trim();
        if trimmed.is_empty() || trimmed.eq_ignore_ascii_case("auto") {
            Ok(Timezone::Auto)
        } else {
            Ok(Timezone::Custom(trimmed.to_string()))
        }
    }
}

/// Format an offset as `UTC+08:00`, `UTC-05:00`, etc.
pub fn format_offset(offset: FixedOffset) -> String {
    let secs = offset.local_minus_utc();
    let sign = if secs >= 0 { '+' } else { '-' };
    let abs = secs.unsigned_abs();
    let hours = abs / 3600;
    let mins = (abs % 3600) / 60;
    format!("UTC{sign}{hours:02}:{mins:02}")
}

/// Detect the ambient offset: `AGYLINE_TZ` (or `CCLINE_TZ`) if valid (and `TZ` on Windows),
/// else the OS local offset.
pub fn detect_system_offset(now: i64) -> FixedOffset {
    from_env(|k| std::env::var(k).ok(), now).unwrap_or_else(|| *chrono::Local::now().offset())
}

fn from_env(var: impl Fn(&str) -> Option<String>, now: i64) -> Option<FixedOffset> {
    if let Some(raw) = var("AGYLINE_TZ")
        .filter(|s| !s.trim().is_empty())
        .or_else(|| var("CCLINE_TZ").filter(|s| !s.trim().is_empty()))
    {
        let trimmed = raw.trim();
        if trimmed.eq_ignore_ascii_case("auto") {
            return None;
        }
        return resolve_spec(trimmed, now);
    }
    #[cfg(windows)]
    if let Some(raw) = var("TZ").filter(|s| !s.trim().is_empty()) {
        return resolve_spec(raw.trim(), now);
    }
    None
}

/// Resolve a non-`auto` timezone string at Unix epoch seconds `now`.
pub fn resolve_spec(spec: &str, now: i64) -> Option<FixedOffset> {
    let s = spec.trim();
    if s.is_empty() || s.eq_ignore_ascii_case("auto") {
        return None;
    }
    if let Some(offset) = parse_offset(s) {
        return Some(offset);
    }
    #[cfg(unix)]
    if let Some(offset) = resolve_zoneinfo(s, now) {
        return Some(offset);
    }
    resolve_builtin(s, now)
}

/// Parse fixed-offset strings such as `+08:00`, `+8`, `UTC+8`, `GMT-05:30`, `Z`.
pub fn parse_offset(spec: &str) -> Option<FixedOffset> {
    let s = spec.trim();
    if s.eq_ignore_ascii_case("z")
        || s.eq_ignore_ascii_case("utc")
        || s.eq_ignore_ascii_case("gmt")
        || s.eq_ignore_ascii_case("ut")
    {
        return FixedOffset::east_opt(0);
    }
    let rest = if s.len() >= 3
        && (s[..3].eq_ignore_ascii_case("utc") || s[..3].eq_ignore_ascii_case("gmt"))
    {
        let tail = s[3..].trim_start();
        if !tail.starts_with(['+', '-']) {
            return None;
        }
        tail
    } else if s.starts_with(['+', '-']) {
        s
    } else {
        return None;
    };

    let sign = if rest.starts_with('-') { -1 } else { 1 };
    let digits = &rest[1..];
    let (hours, mins) = if let Some((h, m)) = digits.split_once(':') {
        if !(1..=2).contains(&h.len())
            || m.len() != 2
            || !h.bytes().all(|b| b.is_ascii_digit())
            || !m.bytes().all(|b| b.is_ascii_digit())
        {
            return None;
        }
        (h.parse::<i32>().ok()?, m.parse::<i32>().ok()?)
    } else if (1..=2).contains(&digits.len()) && digits.bytes().all(|b| b.is_ascii_digit()) {
        (digits.parse::<i32>().ok()?, 0)
    } else if digits.len() == 4 && digits.bytes().all(|b| b.is_ascii_digit()) {
        (
            digits[..2].parse::<i32>().ok()?,
            digits[2..].parse::<i32>().ok()?,
        )
    } else {
        return None;
    };

    if mins >= 60 {
        return None;
    }
    let total = sign * (hours * 3600 + mins * 60);
    // Real-world UTC offsets span -12:00 through +14:00.
    if !(-12 * 3600..=14 * 3600).contains(&total) {
        return None;
    }
    FixedOffset::east_opt(total)
}

fn resolve_builtin(spec: &str, now: i64) -> Option<FixedOffset> {
    let (_, rule) = BUILTIN_ZONES
        .iter()
        .find(|(name, _)| name.eq_ignore_ascii_case(spec))?;
    eval_posix_tz(rule, now)
}

#[cfg(unix)]
fn is_safe_zone_name(name: &str) -> bool {
    !name.is_empty()
        && !name.starts_with('/')
        && !name.ends_with('/')
        && !name
            .split('/')
            .any(|seg| seg.is_empty() || seg == "." || seg == "..")
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'/' | b'_' | b'-' | b'+'))
}

#[cfg(unix)]
fn resolve_zoneinfo(name: &str, now: i64) -> Option<FixedOffset> {
    if !is_safe_zone_name(name) {
        return None;
    }
    let tzdir = std::env::var("TZDIR")
        .ok()
        .filter(|d| !d.is_empty())
        .unwrap_or_else(|| "/usr/share/zoneinfo".to_string());
    let path = std::path::Path::new(&tzdir).join(name);
    let data = std::fs::read(path).ok()?;
    eval_tzif(&data, now)
}

#[cfg(unix)]
fn eval_tzif(data: &[u8], now: i64) -> Option<FixedOffset> {
    if data.len() < 44 || &data[..4] != b"TZif" {
        return None;
    }
    let ver = data[4];
    let counts = |at: usize| -> Option<[usize; 6]> {
        let slice = data.get(at..at + 24)?;
        Some([
            u32::from_be_bytes(slice[0..4].try_into().ok()?) as usize, // isutcnt
            u32::from_be_bytes(slice[4..8].try_into().ok()?) as usize, // isstdcnt
            u32::from_be_bytes(slice[8..12].try_into().ok()?) as usize, // leapcnt
            u32::from_be_bytes(slice[12..16].try_into().ok()?) as usize, // timecnt
            u32::from_be_bytes(slice[16..20].try_into().ok()?) as usize, // typecnt
            u32::from_be_bytes(slice[20..24].try_into().ok()?) as usize, // charcnt
        ])
    };

    let [ut1, std1, leap1, time1, type1, char1] = counts(20)?;
    if matches!(ver, b'2' | b'3' | b'4') {
        let v1_body = time1
            .checked_mul(4)?
            .checked_add(time1)?
            .checked_add(type1.checked_mul(6)?)?
            .checked_add(char1)?
            .checked_add(leap1.checked_mul(8)?)?
            .checked_add(std1)?
            .checked_add(ut1)?;
        let v2_head = 44usize.checked_add(v1_body)?;
        if data.get(v2_head..v2_head + 4)? != b"TZif" {
            return None;
        }
        let [ut2, std2, leap2, time2, type2, char2] = counts(v2_head + 20)?;
        let mut pos = v2_head + 44;
        let times_end = pos.checked_add(time2.checked_mul(8)?)?;
        let types_end = times_end.checked_add(time2)?;
        let infos_end = types_end.checked_add(type2.checked_mul(6)?)?;
        let footer_start = infos_end
            .checked_add(char2)?
            .checked_add(leap2.checked_mul(12)?)?
            .checked_add(std2)?
            .checked_add(ut2)?;
        if data.len() < footer_start {
            return None;
        }
        let mut times = Vec::with_capacity(time2);
        while pos < times_end {
            times.push(i64::from_be_bytes(data[pos..pos + 8].try_into().ok()?));
            pos += 8;
        }
        let types = &data[times_end..types_end];
        let infos = &data[types_end..infos_end];

        // Transitions beyond the explicit table follow the trailing POSIX TZ rule.
        let footer = data[footer_start..]
            .strip_prefix(b"\n")
            .and_then(|s| s.strip_suffix(b"\n"))
            .and_then(|s| std::str::from_utf8(s).ok())
            .filter(|s| !s.is_empty());
        if times.last().is_none_or(|&last| now >= last)
            && let Some(rule) = footer
            && let Some(offset) = eval_posix_tz(rule, now)
        {
            return Some(offset);
        }
        return lookup_tzif_offset(&times, types, infos, now);
    }

    // Fallback for TZif v1-only files (32-bit timestamps).
    let pos = 44usize;
    let times_end = pos.checked_add(time1.checked_mul(4)?)?;
    let types_end = times_end.checked_add(time1)?;
    let infos_end = types_end.checked_add(type1.checked_mul(6)?)?;
    if data.len() < infos_end {
        return None;
    }
    let mut times = Vec::with_capacity(time1);
    let mut p = pos;
    while p < times_end {
        times.push(i32::from_be_bytes(data[p..p + 4].try_into().ok()?) as i64);
        p += 4;
    }
    lookup_tzif_offset(
        &times,
        &data[times_end..types_end],
        &data[types_end..infos_end],
        now,
    )
}

#[cfg(unix)]
fn lookup_tzif_offset(times: &[i64], types: &[u8], infos: &[u8], now: i64) -> Option<FixedOffset> {
    let idx = times.partition_point(|&t| t <= now);
    let type_idx = if idx == 0 {
        0
    } else {
        *types.get(idx - 1)? as usize
    };
    let utoff = i32::from_be_bytes(infos.get(type_idx * 6..type_idx * 6 + 4)?.try_into().ok()?);
    FixedOffset::east_opt(utoff)
}

/// Evaluate a POSIX TZ rule string (e.g. `CST-8`, `EST5EDT,M3.2.0,M11.1.0`)
/// at Unix epoch seconds `now`.
pub fn eval_posix_tz(rule: &str, now: i64) -> Option<FixedOffset> {
    let rest = consume_abbr(rule.trim())?;
    let (std_east, rest) = parse_posix_offset(rest, true)?;
    if rest.is_empty() {
        return FixedOffset::east_opt(std_east);
    }
    let rest = consume_abbr(rest)?;
    let (dst_east, rest) = if rest.is_empty() || rest.starts_with(',') {
        (std_east + 3600, rest)
    } else {
        parse_posix_offset(rest, true)?
    };
    let Some(rest) = rest.strip_prefix(',') else {
        return rest.is_empty().then(|| FixedOffset::east_opt(std_east))?;
    };
    let (start_str, end_str) = rest.split_once(',')?;
    let start_rule = parse_transition_rule(start_str)?;
    let end_rule = parse_transition_rule(end_str)?;

    let local_std = DateTime::from_timestamp(now.saturating_add(std_east as i64), 0)?;
    let year = local_std.year();
    let start_utc = start_rule.utc_timestamp(year, std_east)?;
    let end_utc = end_rule.utc_timestamp(year, dst_east)?;

    let in_dst = if start_utc < end_utc {
        now >= start_utc && now < end_utc
    } else {
        now >= start_utc || now < end_utc
    };
    FixedOffset::east_opt(if in_dst { dst_east } else { std_east })
}

fn consume_abbr(s: &str) -> Option<&str> {
    if let Some(after) = s.strip_prefix('<') {
        let end = after.find('>')?;
        (end > 0).then(|| &after[end + 1..])
    } else {
        let len = s.bytes().take_while(|b| b.is_ascii_alphabetic()).count();
        (len > 0).then(|| &s[len..])
    }
}

/// Parse `H[:M[:S]]` with optional sign. When `invert_sign` is true (POSIX
/// zone offsets), `-8` means +8 hours east of UTC; when false (transition
/// wall-clock times), `/-1` means -1 hour.
fn parse_posix_offset(s: &str, invert_sign: bool) -> Option<(i32, &str)> {
    let (neg, rest) = if let Some(r) = s.strip_prefix('-') {
        (true, r)
    } else if let Some(r) = s.strip_prefix('+') {
        (false, r)
    } else {
        (false, s)
    };
    let (h, rest) = take_uint(rest)?;
    let (m, rest) = if let Some(r) = rest.strip_prefix(':') {
        take_uint(r)?
    } else {
        (0, rest)
    };
    let (sec, rest) = if let Some(r) = rest.strip_prefix(':') {
        take_uint(r)?
    } else {
        (0, rest)
    };
    if m >= 60 || sec >= 60 || h > 167 {
        return None;
    }
    let total = h * 3600 + m * 60 + sec;
    let signed = if invert_sign {
        if neg { total } else { -total }
    } else if neg {
        -total
    } else {
        total
    };
    Some((signed, rest))
}

fn take_uint(s: &str) -> Option<(i32, &str)> {
    let len = s.bytes().take_while(|b| b.is_ascii_digit()).count();
    if len == 0 {
        return None;
    }
    let val = s[..len].parse::<i32>().ok()?;
    Some((val, &s[len..]))
}

#[derive(Clone, Copy)]
enum RuleDate {
    /// `Mm.n.d`: month (1..=12), week (1..=5, 5=last), weekday (0=Sun..=6=Sat).
    MonthWeekDay { month: u32, week: u32, weekday: u32 },
    /// `Jn`: 1-based day ignoring Feb 29 (1..=365).
    JulianNoLeap(u32),
    /// `n`: 0-based day including Feb 29 (0..=365).
    ZeroBasedDay(u32),
}

#[derive(Clone, Copy)]
struct TransitionRule {
    date: RuleDate,
    /// Seconds after local 00:00 (may be negative or >= 86400).
    time_secs: i32,
}

impl TransitionRule {
    fn utc_timestamp(self, year: i32, offset_before: i32) -> Option<i64> {
        let date = match self.date {
            RuleDate::MonthWeekDay {
                month,
                week,
                weekday,
            } => {
                let first = NaiveDate::from_ymd_opt(year, month, 1)?;
                let first_wd = first.weekday().num_days_from_sunday();
                let first_match = 1 + (weekday + 7 - first_wd) % 7;
                let mut day = first_match + (week - 1) * 7;
                if week == 5 {
                    while NaiveDate::from_ymd_opt(year, month, day).is_none() {
                        day -= 7;
                    }
                }
                NaiveDate::from_ymd_opt(year, month, day)?
            }
            RuleDate::JulianNoLeap(j) => {
                let leap = NaiveDate::from_ymd_opt(year, 2, 29).is_some();
                let ordinal = if leap && j >= 60 { j + 1 } else { j };
                NaiveDate::from_yo_opt(year, ordinal)?
            }
            RuleDate::ZeroBasedDay(n) => NaiveDate::from_yo_opt(year, n + 1)?,
        };
        let midnight = date.and_hms_opt(0, 0, 0)?.and_utc().timestamp();
        Some(midnight + self.time_secs as i64 - offset_before as i64)
    }
}

fn parse_transition_rule(s: &str) -> Option<TransitionRule> {
    let (date_part, time_part) = match s.split_once('/') {
        Some((d, t)) => (d, Some(t)),
        None => (s, None),
    };
    let date = if let Some(rest) = date_part.strip_prefix('M') {
        let mut parts = rest.split('.');
        let month: u32 = parts.next()?.parse().ok()?;
        let week: u32 = parts.next()?.parse().ok()?;
        let weekday: u32 = parts.next()?.parse().ok()?;
        if parts.next().is_some()
            || !(1..=12).contains(&month)
            || !(1..=5).contains(&week)
            || weekday > 6
        {
            return None;
        }
        RuleDate::MonthWeekDay {
            month,
            week,
            weekday,
        }
    } else if let Some(rest) = date_part.strip_prefix('J') {
        let j: u32 = rest.parse().ok()?;
        if !(1..=365).contains(&j) {
            return None;
        }
        RuleDate::JulianNoLeap(j)
    } else {
        let n: u32 = date_part.parse().ok()?;
        if n > 365 {
            return None;
        }
        RuleDate::ZeroBasedDay(n)
    };
    let time_secs = match time_part {
        Some(t) => {
            let (secs, rem) = parse_posix_offset(t, false)?;
            if !rem.is_empty() {
                return None;
            }
            secs
        }
        None => 2 * 3600,
    };
    Some(TransitionRule { date, time_secs })
}

/// Cross-platform IANA zone and friendly alias table (case-insensitive).
/// Unix resolves `/usr/share/zoneinfo` first; this table serves Windows,
/// minimal containers without tzdata, case-insensitive input, and aliases
/// such as `Beijing` / `Asia/Beijing`.
const BUILTIN_ZONES: &[(&str, &str)] = &[
    // Friendly aliases & abbreviations
    ("Beijing", "CST-8"),
    ("Asia/Beijing", "CST-8"),
    ("BJT", "CST-8"),
    ("PRC", "CST-8"),
    ("ROC", "CST-8"),
    ("Hongkong", "HKT-8"),
    ("HKT", "HKT-8"),
    ("Singapore", "<+08>-8"),
    ("SGT", "<+08>-8"),
    ("Japan", "JST-9"),
    ("JST", "JST-9"),
    ("ROK", "KST-9"),
    ("KST", "KST-9"),
    ("GB", "GMT0BST,M3.5.0/1,M10.5.0"),
    ("NZ", "NZST-12NZDT,M9.5.0,M4.1.0/3"),
    ("US/Eastern", "EST5EDT,M3.2.0,M11.1.0"),
    ("US/Central", "CST6CDT,M3.2.0,M11.1.0"),
    ("US/Mountain", "MST7MDT,M3.2.0,M11.1.0"),
    ("US/Pacific", "PST8PDT,M3.2.0,M11.1.0"),
    ("US/Alaska", "AKST9AKDT,M3.2.0,M11.1.0"),
    ("US/Hawaii", "HST10"),
    // Etc
    ("Etc/UTC", "UTC0"),
    ("Etc/GMT", "GMT0"),
    ("Etc/GMT+0", "GMT0"),
    ("Etc/GMT-0", "GMT0"),
    ("Etc/GMT-1", "<+01>-1"),
    ("Etc/GMT-2", "<+02>-2"),
    ("Etc/GMT-3", "<+03>-3"),
    ("Etc/GMT-4", "<+04>-4"),
    ("Etc/GMT-5", "<+05>-5"),
    ("Etc/GMT-6", "<+06>-6"),
    ("Etc/GMT-7", "<+07>-7"),
    ("Etc/GMT-8", "<+08>-8"),
    ("Etc/GMT-9", "<+09>-9"),
    ("Etc/GMT-10", "<+10>-10"),
    ("Etc/GMT-11", "<+11>-11"),
    ("Etc/GMT-12", "<+12>-12"),
    ("Etc/GMT-13", "<+13>-13"),
    ("Etc/GMT-14", "<+14>-14"),
    ("Etc/GMT+1", "<-01>1"),
    ("Etc/GMT+2", "<-02>2"),
    ("Etc/GMT+3", "<-03>3"),
    ("Etc/GMT+4", "<-04>4"),
    ("Etc/GMT+5", "<-05>5"),
    ("Etc/GMT+6", "<-06>6"),
    ("Etc/GMT+7", "<-07>7"),
    ("Etc/GMT+8", "<-08>8"),
    ("Etc/GMT+9", "<-09>9"),
    ("Etc/GMT+10", "<-10>10"),
    ("Etc/GMT+11", "<-11>11"),
    ("Etc/GMT+12", "<-12>12"),
    // Asia
    ("Asia/Shanghai", "CST-8"),
    ("Asia/Chongqing", "CST-8"),
    ("Asia/Harbin", "CST-8"),
    ("Asia/Urumqi", "<+06>-6"),
    ("Asia/Hong_Kong", "HKT-8"),
    ("Asia/Macau", "CST-8"),
    ("Asia/Macao", "CST-8"),
    ("Asia/Taipei", "CST-8"),
    ("Asia/Singapore", "<+08>-8"),
    ("Asia/Kuala_Lumpur", "<+08>-8"),
    ("Asia/Tokyo", "JST-9"),
    ("Asia/Seoul", "KST-9"),
    ("Asia/Pyongyang", "KST-9"),
    ("Asia/Bangkok", "<+07>-7"),
    ("Asia/Ho_Chi_Minh", "<+07>-7"),
    ("Asia/Saigon", "<+07>-7"),
    ("Asia/Jakarta", "WIB-7"),
    ("Asia/Makassar", "WITA-8"),
    ("Asia/Jayapura", "WIT-9"),
    ("Asia/Manila", "PST-8"),
    ("Asia/Yangon", "<+0630>-6:30"),
    ("Asia/Dhaka", "<+06>-6"),
    ("Asia/Kathmandu", "<+0545>-5:45"),
    ("Asia/Kolkata", "IST-5:30"),
    ("Asia/Calcutta", "IST-5:30"),
    ("Asia/Colombo", "<+0530>-5:30"),
    ("Asia/Karachi", "PKT-5"),
    ("Asia/Tashkent", "<+05>-5"),
    ("Asia/Almaty", "<+05>-5"),
    ("Asia/Dubai", "<+04>-4"),
    ("Asia/Tehran", "<+0330>-3:30"),
    ("Asia/Riyadh", "<+03>-3"),
    ("Asia/Baghdad", "<+03>-3"),
    ("Asia/Jerusalem", "IST-2IDT,M3.4.4/26,M10.5.0"),
    ("Asia/Beirut", "EET-2EEST,M3.5.0/0,M10.5.0/0"),
    ("Asia/Amman", "<+03>-3"),
    ("Asia/Vladivostok", "<+10>-10"),
    ("Asia/Novosibirsk", "<+07>-7"),
    ("Asia/Yekaterinburg", "<+05>-5"),
    // Europe
    ("Europe/London", "GMT0BST,M3.5.0/1,M10.5.0"),
    ("Europe/Dublin", "GMT0IST,M3.5.0/1,M10.5.0"),
    ("Europe/Lisbon", "WET0WEST,M3.5.0/1,M10.5.0"),
    ("Europe/Madrid", "CET-1CEST,M3.5.0,M10.5.0/3"),
    ("Europe/Paris", "CET-1CEST,M3.5.0,M10.5.0/3"),
    ("Europe/Brussels", "CET-1CEST,M3.5.0,M10.5.0/3"),
    ("Europe/Amsterdam", "CET-1CEST,M3.5.0,M10.5.0/3"),
    ("Europe/Luxembourg", "CET-1CEST,M3.5.0,M10.5.0/3"),
    ("Europe/Zurich", "CET-1CEST,M3.5.0,M10.5.0/3"),
    ("Europe/Berlin", "CET-1CEST,M3.5.0,M10.5.0/3"),
    ("Europe/Vienna", "CET-1CEST,M3.5.0,M10.5.0/3"),
    ("Europe/Rome", "CET-1CEST,M3.5.0,M10.5.0/3"),
    ("Europe/Prague", "CET-1CEST,M3.5.0,M10.5.0/3"),
    ("Europe/Warsaw", "CET-1CEST,M3.5.0,M10.5.0/3"),
    ("Europe/Budapest", "CET-1CEST,M3.5.0,M10.5.0/3"),
    ("Europe/Copenhagen", "CET-1CEST,M3.5.0,M10.5.0/3"),
    ("Europe/Oslo", "CET-1CEST,M3.5.0,M10.5.0/3"),
    ("Europe/Stockholm", "CET-1CEST,M3.5.0,M10.5.0/3"),
    ("Europe/Helsinki", "EET-2EEST,M3.5.0/3,M10.5.0/4"),
    ("Europe/Tallinn", "EET-2EEST,M3.5.0/3,M10.5.0/4"),
    ("Europe/Riga", "EET-2EEST,M3.5.0/3,M10.5.0/4"),
    ("Europe/Vilnius", "EET-2EEST,M3.5.0/3,M10.5.0/4"),
    ("Europe/Athens", "EET-2EEST,M3.5.0/3,M10.5.0/4"),
    ("Europe/Bucharest", "EET-2EEST,M3.5.0/3,M10.5.0/4"),
    ("Europe/Sofia", "EET-2EEST,M3.5.0/3,M10.5.0/4"),
    ("Europe/Kyiv", "EET-2EEST,M3.5.0/3,M10.5.0/4"),
    ("Europe/Kiev", "EET-2EEST,M3.5.0/3,M10.5.0/4"),
    ("Europe/Istanbul", "<+03>-3"),
    ("Europe/Moscow", "MSK-3"),
    ("Europe/Minsk", "<+03>-3"),
    // Americas
    ("America/St_Johns", "NST3:30NDT,M3.2.0,M11.1.0"),
    ("America/Halifax", "AST4ADT,M3.2.0,M11.1.0"),
    ("America/New_York", "EST5EDT,M3.2.0,M11.1.0"),
    ("America/Detroit", "EST5EDT,M3.2.0,M11.1.0"),
    ("America/Toronto", "EST5EDT,M3.2.0,M11.1.0"),
    ("America/Chicago", "CST6CDT,M3.2.0,M11.1.0"),
    ("America/Winnipeg", "CST6CDT,M3.2.0,M11.1.0"),
    ("America/Mexico_City", "CST6"),
    ("America/Denver", "MST7MDT,M3.2.0,M11.1.0"),
    ("America/Edmonton", "MST7MDT,M3.2.0,M11.1.0"),
    ("America/Phoenix", "MST7"),
    ("America/Los_Angeles", "PST8PDT,M3.2.0,M11.1.0"),
    ("America/Vancouver", "PST8PDT,M3.2.0,M11.1.0"),
    ("America/Anchorage", "AKST9AKDT,M3.2.0,M11.1.0"),
    ("America/Bogota", "<-05>5"),
    ("America/Lima", "<-05>5"),
    ("America/Santiago", "<-04>4<-03>,M9.1.6/24,M4.1.6/24"),
    ("America/Caracas", "<-04>4"),
    ("America/Sao_Paulo", "<-03>3"),
    ("America/Argentina/Buenos_Aires", "<-03>3"),
    // Pacific & Australia
    ("Pacific/Honolulu", "HST10"),
    ("Pacific/Auckland", "NZST-12NZDT,M9.5.0,M4.1.0/3"),
    ("Pacific/Fiji", "<+12>-12"),
    ("Pacific/Guam", "ChST-10"),
    ("Australia/Sydney", "AEST-10AEDT,M10.1.0,M4.1.0/3"),
    ("Australia/Melbourne", "AEST-10AEDT,M10.1.0,M4.1.0/3"),
    ("Australia/Brisbane", "AEST-10"),
    ("Australia/Adelaide", "ACST-9:30ACDT,M10.1.0,M4.1.0/3"),
    ("Australia/Perth", "AWST-8"),
    ("Australia/Darwin", "ACST-9:30"),
    ("Australia/Hobart", "AEST-10AEDT,M10.1.0,M4.1.0/3"),
    // Africa
    ("Africa/Cairo", "EET-2EEST,M4.5.5/0,M10.5.4/24"),
    ("Africa/Johannesburg", "SAST-2"),
    ("Africa/Lagos", "WAT-1"),
    ("Africa/Nairobi", "EAT-3"),
    ("Africa/Casablanca", "<+00>0"),
];

#[cfg(test)]
mod tests {
    use super::*;

    fn east(hours: i32, mins: i32) -> FixedOffset {
        let sign = if hours < 0 || mins < 0 { -1 } else { 1 };
        FixedOffset::east_opt(sign * (hours.abs() * 3600 + mins.abs() * 60)).unwrap()
    }

    #[test]
    fn parses_fixed_offsets() {
        for (input, expected) in [
            ("Z", east(0, 0)),
            ("UTC", east(0, 0)),
            ("gmt", east(0, 0)),
            ("+08:00", east(8, 0)),
            ("+8:00", east(8, 0)),
            ("+0800", east(8, 0)),
            ("+08", east(8, 0)),
            ("+8", east(8, 0)),
            ("UTC+8", east(8, 0)),
            ("utc+08:00", east(8, 0)),
            ("GMT+8", east(8, 0)),
            ("UTC +08:00", east(8, 0)),
            ("-05:00", east(-5, 0)),
            ("-5", east(-5, 0)),
            ("UTC-5", east(-5, 0)),
            ("+05:30", east(5, 30)),
            ("UTC+5:30", east(5, 30)),
            ("+14:00", east(14, 0)),
            ("-12:00", east(-12, 0)),
        ] {
            assert_eq!(parse_offset(input), Some(expected), "{input}");
        }
        for bad in [
            "", "auto", "8", "+25:00", "+15:00", "-13:00", "+08:60", "+800", "UTC+foo", "UTC0",
        ] {
            assert_eq!(parse_offset(bad), None, "{bad}");
        }
    }

    #[test]
    fn resolves_beijing_and_iana_aliases() {
        let now = 1_790_812_800;
        for spec in [
            "Asia/Shanghai",
            "asia/shanghai",
            "Beijing",
            "Asia/Beijing",
            "PRC",
            "BJT",
            "Asia/Hong_Kong",
            "Asia/Taipei",
            "Asia/Singapore",
            "Etc/GMT-8",
            "UTC+8",
            "+08:00",
        ] {
            assert_eq!(resolve_spec(spec, now), Some(east(8, 0)), "{spec}");
        }
        assert_eq!(resolve_spec("Etc/GMT+8", now), Some(east(-8, 0)));
        assert_eq!(resolve_spec("Asia/Tokyo", now), Some(east(9, 0)));
        assert_eq!(resolve_spec("Asia/Kolkata", now), Some(east(5, 30)));
    }

    #[test]
    fn evaluates_posix_dst_across_hemispheres_and_extended_hours() {
        // 2026-03-08 06:59:59Z -> EST (-05:00); 07:00:00Z -> EDT (-04:00).
        let before_ny = DateTime::parse_from_rfc3339("2026-03-08T06:59:59Z")
            .unwrap()
            .timestamp();
        let after_ny = before_ny + 1;
        assert_eq!(
            eval_posix_tz("EST5EDT,M3.2.0,M11.1.0", before_ny),
            Some(east(-5, 0))
        );
        assert_eq!(
            eval_posix_tz("EST5EDT,M3.2.0,M11.1.0", after_ny),
            Some(east(-4, 0))
        );

        // 2026-11-01 05:59:59Z -> EDT (-04:00); 06:00:00Z -> EST (-05:00).
        let before_fall = DateTime::parse_from_rfc3339("2026-11-01T05:59:59Z")
            .unwrap()
            .timestamp();
        assert_eq!(
            eval_posix_tz("EST5EDT,M3.2.0,M11.1.0", before_fall),
            Some(east(-4, 0))
        );
        assert_eq!(
            eval_posix_tz("EST5EDT,M3.2.0,M11.1.0", before_fall + 1),
            Some(east(-5, 0))
        );

        // Southern hemisphere: Sydney DST in Nov (+11), standard in Jul (+10).
        let jul = DateTime::parse_from_rfc3339("2026-07-01T12:00:00Z")
            .unwrap()
            .timestamp();
        let nov = DateTime::parse_from_rfc3339("2026-11-18T12:00:00Z")
            .unwrap()
            .timestamp();
        assert_eq!(
            eval_posix_tz("AEST-10AEDT,M10.1.0,M4.1.0/3", jul),
            Some(east(10, 0))
        );
        assert_eq!(
            eval_posix_tz("AEST-10AEDT,M10.1.0,M4.1.0/3", nov),
            Some(east(11, 0))
        );

        // Extended hour >24 (Jerusalem M3.4.4/26 -> Friday/Saturday 02:00) and negative hour (Scoresbysund /-1).
        let israel_after = DateTime::parse_from_rfc3339("2026-03-28T00:00:00Z")
            .unwrap()
            .timestamp();
        assert_eq!(
            eval_posix_tz("IST-2IDT,M3.4.4/26,M10.5.0", israel_after),
            Some(east(3, 0))
        );
        let greenland_after = DateTime::parse_from_rfc3339("2026-03-29T01:00:00Z")
            .unwrap()
            .timestamp();
        assert_eq!(
            eval_posix_tz("<-02>2<-01>,M3.5.0/-1,M10.5.0/0", greenland_after),
            Some(east(-1, 0))
        );
    }

    #[cfg(unix)]
    #[test]
    fn zoneinfo_matches_builtin_for_common_zones() {
        let winter = DateTime::parse_from_rfc3339("2026-01-15T12:00:00Z")
            .unwrap()
            .timestamp();
        let summer = DateTime::parse_from_rfc3339("2026-07-15T12:00:00Z")
            .unwrap()
            .timestamp();
        for name in [
            "Asia/Shanghai",
            "Asia/Tokyo",
            "Asia/Kolkata",
            "Europe/London",
            "Europe/Berlin",
            "America/New_York",
            "America/Los_Angeles",
            "Australia/Sydney",
        ] {
            if let Some(zi) = resolve_zoneinfo(name, winter) {
                assert_eq!(Some(zi), resolve_builtin(name, winter), "{name} winter");
            }
            if let Some(zi) = resolve_zoneinfo(name, summer) {
                assert_eq!(Some(zi), resolve_builtin(name, summer), "{name} summer");
            }
        }
        assert!(resolve_zoneinfo("../etc/passwd", winter).is_none());
        assert!(resolve_zoneinfo("/etc/passwd", winter).is_none());
    }

    #[test]
    fn reads_env_override_and_formats_offsets() {
        let env = |pairs: &'static [(&'static str, &'static str)]| {
            move |k: &str| {
                pairs
                    .iter()
                    .find(|(name, _)| *name == k)
                    .map(|(_, v)| v.to_string())
            }
        };
        assert_eq!(
            from_env(env(&[("CCLINE_TZ", "Asia/Shanghai")]), 0),
            Some(east(8, 0))
        );
        assert_eq!(from_env(env(&[("CCLINE_TZ", "auto")]), 0), None);
        assert_eq!(from_env(env(&[("CCLINE_TZ", "Mars/Phobos")]), 0), None);
        assert_eq!(format_offset(east(8, 0)), "UTC+08:00");
        assert_eq!(format_offset(east(-5, 0)), "UTC-05:00");
        assert_eq!(format_offset(east(5, 30)), "UTC+05:30");
    }

    #[test]
    fn timezone_round_trips_and_flags_unknown_values() {
        #[derive(Serialize, Deserialize, PartialEq, Debug)]
        struct Boxed {
            #[serde(default)]
            timezone: Timezone,
        }
        let unset: Boxed = toml::from_str("").unwrap();
        assert_eq!(unset.timezone, Timezone::Auto);

        let bj: Boxed = toml::from_str("timezone = \"Asia/Shanghai\"").unwrap();
        assert_eq!(bj.timezone, Timezone::Custom("Asia/Shanghai".into()));
        assert_eq!(bj.timezone.invalid_spec(), None);
        assert_eq!(
            toml::to_string(&bj).unwrap().trim(),
            "timezone = \"Asia/Shanghai\""
        );

        let bad: Boxed = toml::from_str("timezone = \"Mars/Phobos\"").unwrap();
        assert_eq!(bad.timezone.invalid_spec(), Some("Mars/Phobos"));
        assert_eq!(bad.timezone.resolve(0, east(0, 0)), east(0, 0));

        assert_eq!(Timezone::parse("").unwrap(), Timezone::Auto);
        assert_eq!(Timezone::parse("AUTO").unwrap(), Timezone::Auto);
        assert!(Timezone::parse("Asia/Shanghai").is_ok());
        assert!(Timezone::parse("Mars/Phobos").is_err());
    }
}
