//! Standard five-field cron schedules, evaluated in the server's time zone.
use anyhow::{Context, Result, bail, ensure};

/// Long enough to reach any possible date, including 29 February after a skipped leap year.
const LOOK_AHEAD_SECONDS: u64 = 9 * 366 * 86400;
const MAX_LENGTH: usize = 128;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Cron {
    source: String,
    minutes: u64,
    hours: u32,
    days: u32,
    months: u16,
    weekdays: u8,
    any_day: bool,
    any_weekday: bool,
    /// Neither minute nor hour starts with a star, so the schedule names a time of day.
    /// As in cron, only such schedules are adjusted when the clocks change.
    fixed_time: bool,
}

const MONTHS: [&str; 12] = [
    "jan", "feb", "mar", "apr", "may", "jun", "jul", "aug", "sep", "oct", "nov", "dec",
];
const WEEKDAYS: [&str; 7] = ["sun", "mon", "tue", "wed", "thu", "fri", "sat"];

fn value(text: &str, names: &[&str], first: u32) -> Result<u32> {
    if let Some(index) = names
        .iter()
        .position(|name| name.eq_ignore_ascii_case(text))
    {
        return Ok(first + index as u32);
    }
    text.parse()
        .with_context(|| format!("Invalid value '{text}'"))
}

/// One field as a bit per allowed value.
fn field(text: &str, min: u32, max: u32, names: &[&str]) -> Result<u64> {
    let mut bits = 0_u64;
    for item in text.split(',') {
        let (range, step) = match item.split_once('/') {
            Some((range, step)) => (
                range,
                Some(
                    step.parse::<u32>()
                        .with_context(|| format!("Invalid step '{step}'"))?,
                ),
            ),
            None => (item, None),
        };
        let (low, high) = if range == "*" {
            (min, max)
        } else if let Some((low, high)) = range.split_once('-') {
            (value(low, names, min)?, value(high, names, min)?)
        } else {
            let low = value(range, names, min)?;
            // `5/15` means from 5 to the end of the range.
            (low, if step.is_some() { max } else { low })
        };
        ensure!(
            min <= low && low <= high && high <= max,
            "'{item}' is outside {min}-{max}"
        );
        let step = step.unwrap_or(1);
        ensure!(step > 0, "A step must be at least 1");
        for allowed in (low..=high).step_by(step as usize) {
            bits |= 1 << allowed;
        }
    }
    Ok(bits)
}

struct Local {
    minute: u32,
    hour: u32,
    day: u32,
    month: u32,
    weekday: u32,
    year: i32,
    /// Seconds east of UTC at this moment.
    offset: i64,
}

impl Local {
    fn new(tm: &libc::tm, offset: i64) -> Self {
        Self {
            minute: tm.tm_min as u32,
            hour: tm.tm_hour as u32,
            day: tm.tm_mday as u32,
            month: tm.tm_mon as u32 + 1,
            weekday: tm.tm_wday as u32,
            year: tm.tm_year + 1900,
            offset,
        }
    }
}

fn local(time: u64) -> Option<Local> {
    let time = libc::time_t::try_from(time).ok()?;
    // SAFETY: an all-zero `tm` is a valid value, and `localtime_r` only accesses the two
    // valid, exclusively borrowed values passed here.
    let tm = unsafe {
        let mut tm: libc::tm = std::mem::zeroed();
        if libc::localtime_r(&time, &mut tm).is_null() {
            return None;
        }
        tm
    };
    Some(Local::new(&tm, tm.tm_gmtoff as i64))
}

/// What a clock set to `offset` shows at `time`, including times the zone itself skips.
fn wall_clock(time: u64, offset: i64) -> Option<Local> {
    let shifted = libc::time_t::try_from(i64::try_from(time).ok()?.checked_add(offset)?).ok()?;
    // SAFETY: an all-zero `tm` is a valid value, and `gmtime_r` only accesses the two
    // valid, exclusively borrowed values passed here.
    let tm = unsafe {
        let mut tm: libc::tm = std::mem::zeroed();
        if libc::gmtime_r(&shifted, &mut tm).is_null() {
            return None;
        }
        tm
    };
    Some(Local::new(&tm, offset))
}

impl Cron {
    pub fn parse(text: &str) -> Result<Self> {
        let text = text.split_whitespace().collect::<Vec<_>>().join(" ");
        ensure!(
            text.len() <= MAX_LENGTH,
            "A schedule is at most {MAX_LENGTH} characters"
        );
        let expanded = match text.to_ascii_lowercase().as_str() {
            "@hourly" => "0 * * * *".to_owned(),
            "@daily" | "@midnight" => "0 0 * * *".to_owned(),
            "@weekly" => "0 0 * * 0".to_owned(),
            "@monthly" => "0 0 1 * *".to_owned(),
            "@yearly" | "@annually" => "0 0 1 1 *".to_owned(),
            _ => text.clone(),
        };
        let parts: Vec<_> = expanded.split(' ').collect();
        let [minute, hour, day, month, weekday] = parts[..] else {
            bail!("A schedule has five fields: minute hour day month weekday");
        };
        let named = |name: &str, result: Result<u64>| {
            result.with_context(|| format!("Invalid {name} field"))
        };
        let weekdays = named("weekday", field(weekday, 0, 7, &WEEKDAYS))?;
        let cron = Self {
            minutes: named("minute", field(minute, 0, 59, &[]))?,
            hours: named("hour", field(hour, 0, 23, &[]))? as u32,
            days: named("day", field(day, 1, 31, &[]))? as u32,
            months: named("month", field(month, 1, 12, &MONTHS))? as u16,
            // 0 and 7 are both Sunday.
            weekdays: ((weekdays & 0x7f) | u64::from(weekdays & 0x80 != 0)) as u8,
            any_day: day.starts_with('*'),
            any_weekday: weekday.starts_with('*'),
            fixed_time: !minute.starts_with('*') && !hour.starts_with('*'),
            source: text,
        };
        // 31 February is valid field by field but would silently never scan.
        ensure!(
            cron.next(super::updates::now()).is_some(),
            "This schedule never runs"
        );
        Ok(cron)
    }

    pub fn source(&self) -> &str {
        &self.source
    }

    fn day_matches(&self, time: &Local) -> bool {
        let day = self.days & (1 << time.day) != 0;
        let weekday = self.weekdays & (1 << time.weekday) != 0;
        // As in cron: when both are restricted, either one is enough.
        if self.any_day || self.any_weekday {
            day && weekday
        } else {
            day || weekday
        }
    }

    fn hour_matches(&self, time: &Local) -> bool {
        self.months & (1 << time.month) != 0
            && self.day_matches(time)
            && self.hours & (1 << time.hour) != 0
    }

    fn minute_matches(&self, time: &Local) -> bool {
        self.hour_matches(time) && self.minutes & (1 << time.minute) != 0
    }

    /// The clocks jumped forward at `time`, over a start this schedule names.
    fn skipped_a_start(
        &self,
        time: u64,
        now: &Local,
        zone: &impl Fn(u64) -> Option<Local>,
    ) -> bool {
        let Some(before) = time.checked_sub(60).and_then(zone) else {
            return false;
        };
        let jump = (now.offset - before.offset) / 60;
        jump > 0
            && (0..jump as u64).any(|minute| {
                wall_clock(time + minute * 60, before.offset)
                    .is_some_and(|skipped| self.minute_matches(&skipped))
            })
    }

    /// The first start strictly after `after`, on a whole minute. A time of day the clocks
    /// skip runs right after the change, and one they repeat runs only the first time.
    pub fn next(&self, after: u64) -> Option<u64> {
        self.next_in(after, &local)
    }

    fn next_in(&self, after: u64, zone: &impl Fn(u64) -> Option<Local>) -> Option<u64> {
        let mut time = (after / 60 + 1) * 60;
        let limit = after.saturating_add(LOOK_AHEAD_SECONDS);
        while time <= limit {
            let now = zone(time)?;
            if self.fixed_time && self.skipped_a_start(time, &now, zone) {
                return Some(time);
            }
            if !self.hour_matches(&now) {
                time += 3600 - u64::from(now.minute) * 60;
            } else if !self.minute_matches(&now) || (self.fixed_time && repeated(time, &now, zone))
            {
                time += 60;
            } else {
                return Some(time);
            }
        }
        None
    }
}

/// The clocks went back, and already showed this time of day once.
fn repeated(time: u64, now: &Local, zone: &impl Fn(u64) -> Option<Local>) -> bool {
    let Some(earlier) = time.checked_sub(86400).and_then(zone) else {
        return false;
    };
    let back = earlier.offset - now.offset;
    back > 0
        && time
            .checked_sub(back as u64)
            .and_then(zone)
            .is_some_and(|first| first.offset == earlier.offset)
}

/// `2026-11-02 00:00` in the server's time zone.
pub fn format_local(time: u64) -> Option<String> {
    let time = local(time)?;
    Some(format!(
        "{:04}-{:02}-{:02} {:02}:{:02}",
        time.year, time.month, time.day, time.hour, time.minute
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The fields of `next`, so these hold whatever zone the tests run in.
    fn next_fields(schedule: &str, after: u64) -> (u32, u32, u32, u32) {
        let next = Cron::parse(schedule).unwrap().next(after).unwrap();
        let time = local(next).unwrap();
        (time.month, time.day, time.hour, time.minute)
    }

    const NOON: u64 = 1_781_524_800; // 2026-06-15, a Monday, around midday in most zones.

    #[test]
    fn standard_schedules_land_on_their_fields() {
        assert_eq!(next_fields("0 0 * * *", NOON).2, 0);
        assert_eq!(next_fields("@daily", NOON).3, 0);
        let (_, _, hour, minute) = next_fields("30 4 * * *", NOON);
        assert_eq!((hour, minute), (4, 30));
        assert_eq!(next_fields("*/15 * * * *", NOON).3 % 15, 0);
        assert_eq!(next_fields("0 3 1 * *", NOON).1, 1);
        assert_eq!(next_fields("0 0 1 jan *", NOON).0, 1);
    }

    #[test]
    fn next_is_strictly_later_and_on_a_whole_minute() {
        let cron = Cron::parse("* * * * *").unwrap();
        assert_eq!(cron.next(NOON), Some(NOON + 60));
        assert_eq!(cron.next(NOON + 1), Some(NOON + 60));
        assert_eq!(cron.next(NOON + 59), Some(NOON + 60));
    }

    #[test]
    fn sunday_is_zero_and_seven_and_names_work() {
        for schedule in ["0 0 * * 0", "0 0 * * 7", "0 0 * * sun", "@weekly"] {
            let next = Cron::parse(schedule).unwrap().next(NOON).unwrap();
            assert_eq!(local(next).unwrap().weekday, 0, "{schedule}");
        }
    }

    #[test]
    fn restricted_day_and_weekday_match_either_one() {
        let cron = Cron::parse("0 0 13 * fri").unwrap();
        let next = local(cron.next(NOON).unwrap()).unwrap();
        assert!(next.day == 13 || next.weekday == 5);
        // A star in either field means only the other restricts the day.
        let next = local(Cron::parse("0 0 13 * *").unwrap().next(NOON).unwrap()).unwrap();
        assert_eq!(next.day, 13);
    }

    #[test]
    fn leap_days_are_reachable_but_impossible_dates_are_refused() {
        assert_eq!(next_fields("0 0 29 2 *", NOON).1, 29);
        assert!(Cron::parse("0 0 31 2 *").is_err());
        assert!(Cron::parse("0 0 31 4 *").is_err());
    }

    #[test]
    fn malformed_schedules_are_refused() {
        for schedule in [
            "",
            "* * * *",
            "* * * * * *",
            "60 * * * *",
            "* 24 * * *",
            "* * 0 * *",
            "* * * 13 *",
            "* * * * 8",
            "*/0 * * * *",
            "5-1 * * * *",
            "a * * * *",
            "1,,2 * * * *",
            "@sometimes",
            "; * * * *",
        ] {
            assert!(Cron::parse(schedule).is_err(), "{schedule}");
        }
        assert!(Cron::parse(&format!("0 0 * * {}", "0,".repeat(80) + "0")).is_err());
    }

    /// Berlin in 2026: 01:00 UTC is 02:00 turning into 03:00, then 03:00 turning into 02:00.
    const FORWARD: u64 = 1_774_746_000;
    const BACK: u64 = 1_792_890_000;
    const HOUR: u64 = 3600;
    fn berlin(time: u64) -> Option<Local> {
        let summer = (FORWARD..BACK).contains(&time);
        wall_clock(time, if summer { 7200 } else { 3600 })
    }
    fn next_in_berlin(schedule: &str, after: u64) -> u64 {
        Cron::parse(schedule)
            .unwrap()
            .next_in(after, &berlin)
            .unwrap()
    }

    #[test]
    fn a_time_the_clocks_skip_runs_right_after_the_change_and_only_on_its_day() {
        assert_eq!(next_in_berlin("30 2 * * *", FORWARD - HOUR), FORWARD);
        assert_eq!(
            next_in_berlin("30 2 * * *", FORWARD),
            FORWARD + 23 * HOUR + 1800,
            "Back to 02:30 the next day, once"
        );
        assert_eq!(next_in_berlin("0 3 * * *", FORWARD - HOUR), FORWARD);
        assert_ne!(
            next_in_berlin("30 2 1 * *", FORWARD - HOUR),
            FORWARD,
            "Not its day of the month"
        );
    }

    #[test]
    fn a_time_the_clocks_repeat_runs_only_the_first_time() {
        let first = BACK - 1800;
        assert_eq!(next_in_berlin("30 2 * * *", first - 60), first);
        assert_eq!(next_in_berlin("30 2 * * *", first), BACK + 1800 + 24 * HOUR);
        // 01:30 and 03:30 happen once, on either side of the repeated hour.
        assert_eq!(
            next_in_berlin("30 1 * * *", first - HOUR - 60),
            first - HOUR
        );
        assert_eq!(next_in_berlin("30 3 * * *", first - 60), BACK + HOUR + 1800);
    }

    #[test]
    fn schedules_with_a_star_follow_real_time_through_clock_changes() {
        assert_eq!(next_in_berlin("0 * * * *", FORWARD - HOUR), FORWARD);
        assert_eq!(next_in_berlin("0 * * * *", FORWARD), FORWARD + HOUR);
        assert_eq!(next_in_berlin("30 * * * *", BACK - 1800), BACK + 1800);
        assert_eq!(next_in_berlin("* 2 * * *", BACK), BACK + 60);
    }

    #[test]
    fn spacing_is_normalized_for_the_stored_schedule() {
        assert_eq!(Cron::parse(" 0   0 * * * ").unwrap().source(), "0 0 * * *");
    }
}
