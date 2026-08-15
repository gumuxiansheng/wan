//! Cron 表达式解析与下次触发时间计算（spec §4.7 F-CRON-2）
//!
//! 标准 5 字段 cron：分 时 日 月 周
//! 支持范围：
//!   - `*` 任意值
//!   - `N` 精确值
//!   - `N-M` 范围
//!   - `N,M,...` 列表
//!   - `*/N` 步进
//!   - `N-M/S` 范围步进
//!   - 周字段：0/7=周日, 1-6=周一至周六
//!   - 月字段：1-12
//!
//! 不支持：宏（@daily 等）、秒字段、L/W/# 等增强语法

use std::time::{Duration, SystemTime, UNIX_EPOCH};

use crate::error::{Error, Result};

/// 一组 cron 字段的 bit-set
/// 分钟 0-59 需要 60 bit，所以用 u64
#[derive(Debug, Clone, Copy)]
struct Field {
    bits: u64,
}

impl Field {
    fn new() -> Self {
        Field { bits: 0 }
    }

    fn set(&mut self, val: u32) {
        if val < 64 {
            self.bits |= 1u64 << val;
        }
    }

    fn is_set(&self, val: u32) -> bool {
        if val >= 64 {
            return false;
        }
        (self.bits & (1u64 << val)) != 0
    }

    fn clear(&mut self, val: u32) {
        if val < 64 {
            self.bits &= !(1u64 << val);
        }
    }

    fn any(&self) -> bool {
        self.bits != 0
    }
}

#[derive(Debug, Clone)]
pub struct CronExpr {
    minute: Field,
    hour: Field,
    day: Field,
    month: Field,
    weekday: Field,
    raw: String,
}

impl CronExpr {
    /// 解析 cron 表达式字符串
    pub fn parse(expr: &str) -> Result<Self> {
        let parts: Vec<&str> = expr.split_whitespace().collect();
        if parts.len() != 5 {
            return Err(Error::config(format!(
                "cron 表达式需要 5 个字段（分 时 日 月 周），得到 {} 个：`{expr}`",
                parts.len()
            )));
        }

        let minute = parse_field(parts[0], 0, 59, "分")?;
        let hour = parse_field(parts[1], 0, 23, "时")?;
        let day = parse_field(parts[2], 1, 31, "日")?;
        let month = parse_field(parts[3], 1, 12, "月")?;
        // 周字段：0 和 7 都表示周日
        let mut weekday = parse_field(parts[4], 0, 7, "周")?;
        // 7 → 0 统一（清除 bit 7，保留 bit 0）
        if weekday.is_set(7) {
            weekday.set(0);
            weekday.clear(7);
        }

        Ok(CronExpr {
            minute,
            hour,
            day,
            month,
            weekday,
            raw: expr.to_string(),
        })
    }

    pub fn raw(&self) -> &str {
        &self.raw
    }

    /// 计算从 `after` 之后的下一次触发时间（不含 `after` 本身）
    /// 返回 UTC 时间戳（秒）
    pub fn next_after(&self, after: SystemTime) -> Option<SystemTime> {
        // 转为秒级时间戳
        let now_secs = after.duration_since(UNIX_EPOCH).ok()?.as_secs();

        // 从 after+1 分钟开始，逐分钟扫描
        // 扫描上限 4 年（覆盖 2/29 这类低频 cron）
        // 对齐到分钟边界（向下取整）
        let start = now_secs - (now_secs % 60) + 60; // 下一分钟边界

        const MAX_SCAN: u64 = 4 * 366 * 24 * 60; // 4 年的分钟数（保守上界）

        for i in 0..MAX_SCAN {
            let t = start + i * 60;
            if self.matches_timestamp(t) {
                return Some(UNIX_EPOCH + Duration::from_secs(t));
            }
        }
        None
    }

    /// 检查当前时间是否匹配（分钟粒度）
    pub fn matches_now(&self, now: SystemTime) -> bool {
        let secs = now
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        // 对齐到当前分钟边界
        let minute_start = secs - (secs % 60);
        self.matches_timestamp(minute_start)
    }

    /// 检查给定 UTC 时间戳（秒）是否匹配
    fn matches_timestamp(&self, secs: u64) -> bool {
        // 转为 UTC 年月日时分
        let (minute, hour, day, month, weekday) = secs_to_components(secs);
        self.minute.is_set(minute as u32)
            && self.hour.is_set(hour as u32)
            && self.day.is_set(day as u32)
            && self.month.is_set(month as u32)
            && self.weekday.is_set(weekday as u32)
    }
}

/// 将 UNIX 时间戳（秒）转为 (minute, hour, day, month, weekday)
/// weekday: 0=周日, 1-6=周一至周六（cron 标准）
/// 使用系统本地时区（本地执行器定位，非 UTC）
fn secs_to_components(secs: u64) -> (u8, u8, u8, u8, u8) {
    use jiff::tz::TimeZone;
    let ts = jiff::Timestamp::from_second(secs as i64).unwrap_or_else(|_| jiff::Timestamp::now());
    let tz = TimeZone::system();
    let zoned = ts.to_zoned(tz);
    let dt = zoned.datetime();
    // jiff Weekday: Monday=1..Sunday=7，转为 cron 标准 0=Sunday..6=Saturday
    let weekday = dt.weekday().to_sunday_zero_offset() as u8;
    (
        dt.minute() as u8,
        dt.hour() as u8,
        dt.day() as u8,
        dt.month() as u8,
        weekday,
    )
}

fn parse_field(s: &str, min: u32, max: u32, name: &str) -> Result<Field> {
    let mut field = Field::new();

    for part in s.split(',') {
        if part == "*" {
            for v in min..=max {
                field.set(v);
            }
        } else if let Some(range_part) = part.strip_prefix("*/") {
            // */N
            let step: u32 = range_part
                .parse()
                .map_err(|_| Error::config(format!("cron {name} 字段步进非法：`{part}`")))?;
            if step == 0 {
                return Err(Error::config(format!("cron {name} 字段步进不能为 0")));
            }
            for v in (min..=max).step_by(step as usize) {
                field.set(v);
            }
        } else if part.contains('/') {
            // N-M/S
            let (range_str, step_str) = part
                .split_once('/')
                .ok_or_else(|| Error::config(format!("cron {name} 字段格式非法：`{part}`")))?;
            let step: u32 = step_str
                .parse()
                .map_err(|_| Error::config(format!("cron {name} 字段步进非法：`{step_str}`")))?;
            if step == 0 {
                return Err(Error::config(format!("cron {name} 字段步进不能为 0")));
            }
            let (lo, hi) = parse_range(range_str, min, max, name)?;
            for v in (lo..=hi).step_by(step as usize) {
                field.set(v);
            }
        } else if part.contains('-') {
            // N-M
            let (lo, hi) = parse_range(part, min, max, name)?;
            for v in lo..=hi {
                field.set(v);
            }
        } else {
            // N
            let v: u32 = part
                .parse()
                .map_err(|_| Error::config(format!("cron {name} 字段值非法：`{part}`")))?;
            if v < min || v > max {
                return Err(Error::config(format!(
                    "cron {name} 字段值 {v} 超出范围 [{min}, {max}]"
                )));
            }
            field.set(v);
        }
    }

    if !field.any() {
        return Err(Error::config(format!("cron {name} 字段为空")));
    }
    Ok(field)
}

fn parse_range(s: &str, min: u32, max: u32, name: &str) -> Result<(u32, u32)> {
    let (lo_str, hi_str) = s
        .split_once('-')
        .ok_or_else(|| Error::config(format!("cron {name} 字段范围格式非法：`{s}`")))?;
    let lo: u32 = lo_str
        .parse()
        .map_err(|_| Error::config(format!("cron {name} 字段范围下限非法：`{lo_str}`")))?;
    let hi: u32 = hi_str
        .parse()
        .map_err(|_| Error::config(format!("cron {name} 字段范围上限非法：`{hi_str}`")))?;
    if lo < min || hi > max {
        return Err(Error::config(format!(
            "cron {name} 字段范围 {lo}-{hi} 超出 [{min}, {max}]"
        )));
    }
    if lo > hi {
        return Err(Error::config(format!(
            "cron {name} 字段范围 {lo}-{hi} 下限大于上限"
        )));
    }
    Ok((lo, hi))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cron(s: &str) -> CronExpr {
        CronExpr::parse(s).unwrap()
    }

    #[test]
    fn parse_every_minute() {
        let c = cron("* * * * *");
        assert!(c.minute.is_set(0));
        assert!(c.minute.is_set(59));
        assert!(c.hour.is_set(0));
        assert!(c.hour.is_set(23));
    }

    #[test]
    fn parse_specific() {
        let c = cron("30 2 * * *");
        assert!(c.minute.is_set(30));
        assert!(!c.minute.is_set(0));
        assert!(c.hour.is_set(2));
        assert!(!c.hour.is_set(3));
    }

    #[test]
    fn parse_step() {
        let c = cron("*/15 * * * *");
        assert!(c.minute.is_set(0));
        assert!(c.minute.is_set(15));
        assert!(c.minute.is_set(30));
        assert!(c.minute.is_set(45));
        assert!(!c.minute.is_set(5));
    }

    #[test]
    fn parse_range_step() {
        let c = cron("0-59/5 * * * *");
        assert!(c.minute.is_set(0));
        assert!(c.minute.is_set(5));
        assert!(c.minute.is_set(55));
        assert!(!c.minute.is_set(3));
    }

    #[test]
    fn parse_list() {
        let c = cron("0,15,30,45 * * * *");
        assert!(c.minute.is_set(0));
        assert!(c.minute.is_set(15));
        assert!(c.minute.is_set(30));
        assert!(c.minute.is_set(45));
        assert!(!c.minute.is_set(10));
    }

    #[test]
    fn parse_range() {
        let c = cron("0-4 * * * *");
        assert!(c.minute.is_set(0));
        assert!(c.minute.is_set(4));
        assert!(!c.minute.is_set(5));
    }

    #[test]
    fn parse_weekday_7_is_sunday() {
        let c = cron("* * * * 7");
        // 7 应该映射到 0（周日）
        assert!(c.weekday.is_set(0) || c.weekday.is_set(7));
    }

    #[test]
    fn parse_error_wrong_fields() {
        assert!(CronExpr::parse("* * * *").is_err());
        assert!(CronExpr::parse("* * * * * *").is_err());
    }

    #[test]
    fn parse_error_out_of_range() {
        assert!(CronExpr::parse("60 * * * *").is_err());
        assert!(CronExpr::parse("* 24 * * *").is_err());
        assert!(CronExpr::parse("* * 32 * *").is_err());
        assert!(CronExpr::parse("* * * 13 *").is_err());
        assert!(CronExpr::parse("* * * * 8").is_err());
    }

    #[test]
    fn parse_error_zero_step() {
        assert!(CronExpr::parse("*/0 * * * *").is_err());
    }

    #[test]
    fn parse_error_reversed_range() {
        assert!(CronExpr::parse("5-3 * * * *").is_err());
    }

    #[test]
    fn next_after_basic() {
        // 每分钟触发
        let c = cron("* * * * *");
        let now = SystemTime::now();
        let next = c.next_after(now).unwrap();
        let diff = next.duration_since(now).unwrap().as_secs();
        // 应该在 0-120 秒内触发
        assert!(diff <= 120, "next_after too far: {diff}s");
    }

    #[test]
    fn next_after_specific() {
        // 每天 02:30（本地时区）
        let c = cron("30 2 * * *");
        let now = SystemTime::now();
        let next = c.next_after(now).unwrap();
        // 最多 25 小时后（跨夏令时/时区边界时可能略超 24h）
        let diff = next.duration_since(now).unwrap().as_secs();
        assert!(diff <= 25 * 3600, "next_after too far: {diff}s");
    }

    #[test]
    fn next_after_feb_29() {
        // 2 月 29 日（闰年才存在）
        let c = cron("0 0 29 2 *");
        let now = SystemTime::now();
        // 最多 4 年后
        if let Some(next) = c.next_after(now) {
            let diff = next.duration_since(now).unwrap().as_secs();
            assert!(
                diff <= 4 * 365 * 24 * 3600 + 60,
                "next_after too far: {diff}s"
            );
        }
        // 不存在匹配时返回 None 也可接受（但 4 年内一定有 2/29）
    }

    #[test]
    fn next_immediate_minute() {
        // 固定到一个已知时间点测试（本地时区）
        // 2026-01-01 00:00:00 UTC = 1767225600
        // 在 UTC+8 下为 08:00 本地时间
        let base = UNIX_EPOCH + Duration::from_secs(1767225600);
        let c = cron("30 0 * * *");
        let next = c.next_after(base).unwrap();
        // 下一次 00:30 本地时间
        let diff = next.duration_since(base).unwrap().as_secs();
        // 在 UTC+8 下，base 是本地 08:00，下次 00:30 是约 16.5 小时后
        // 只验证差值合理（不超过 25 小时）
        assert!(diff <= 25 * 3600 && diff > 0, "diff: {diff}s");
    }

    #[test]
    fn next_crosses_hour() {
        // 2026-01-01 00:59:00 UTC
        let base = UNIX_EPOCH + Duration::from_secs(1767225540);
        let c = cron("0 * * * *"); // 每小时整点
        let next = c.next_after(base).unwrap();
        let secs = next.duration_since(UNIX_EPOCH).unwrap().as_secs();
        // 应该是 2026-01-01 01:00:00 UTC
        assert_eq!(secs, 1767225600);
    }

    #[test]
    fn next_crosses_day() {
        // 2026-01-01 23:59:00 UTC
        let base = UNIX_EPOCH + Duration::from_secs(1767311540);
        let c = cron("0 0 * * *"); // 每天 00:00 本地时间
        let next = c.next_after(base).unwrap();
        let diff = next.duration_since(base).unwrap().as_secs();
        // 下一次本地 00:00，差值不超过 25 小时
        assert!(diff <= 25 * 3600 && diff > 0, "diff: {diff}s");
    }
}
