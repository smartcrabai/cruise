//! User-declared model availability windows, evaluated in a fixed UTC offset.

#[derive(Debug, Clone)]
pub struct TimeWindow {
    days: u8,
    start: u16,
    end: u16,
    offset_secs: i32,
}

/// Parse `[days ]HH:MM-HH:MM[ UTC|Z|+HH:MM|-HH:MM]`.
///
/// # Errors
/// Returns a configuration error when the window has invalid days, times, or offset.
pub fn parse_time_window(input: &str) -> crate::error::Result<TimeWindow> {
    parse(input).ok_or_else(|| crate::error::CruiseError::InvalidStepConfig(format!(
        "invalid disabled_during window {input:?}: expected [days ]HH:MM-HH:MM[ offset], e.g. mon-fri 01:00-04:00 UTC; use weekday names mon..sun, an end time up to 24:00, and UTC, Z, or a fixed offset within +/-14:00"
    )))
}

fn parse(input: &str) -> Option<TimeWindow> {
    let mut tokens = input.split_whitespace();
    let first = tokens.next()?;
    let (days, time) = if first.contains(':') {
        (0x7f, first)
    } else {
        (parse_days(first)?, tokens.next()?)
    };
    let offset = tokens.next();
    if tokens.next().is_some() {
        return None;
    }
    let (start, end) = time.split_once('-')?;
    let start = parse_clock(start, false)?;
    let end = parse_clock(end, true)?;
    if start == end || end == 0 {
        return None;
    }
    let offset_secs = match offset {
        None | Some("UTC" | "Z") => 0,
        Some(offset) => {
            let sign = match offset.as_bytes().first()? {
                b'+' => 1,
                b'-' => -1,
                _ => return None,
            };
            let minutes = parse_clock(&offset[1..], false)?;
            if minutes > 14 * 60 {
                return None;
            }
            sign * i32::from(minutes) * 60
        }
    };
    Some(TimeWindow {
        days,
        start,
        end,
        offset_secs,
    })
}

fn parse_clock(raw: &str, allow_end: bool) -> Option<u16> {
    let bytes = raw.as_bytes();
    if bytes.len() != 5
        || bytes[2] != b':'
        || ![bytes[0], bytes[1], bytes[3], bytes[4]]
            .iter()
            .all(u8::is_ascii_digit)
    {
        return None;
    }
    let hour = u16::from(bytes[0] - b'0') * 10 + u16::from(bytes[1] - b'0');
    let minute = u16::from(bytes[3] - b'0') * 10 + u16::from(bytes[4] - b'0');
    if minute >= 60 || (hour >= 24 && !(allow_end && hour == 24 && minute == 0)) {
        return None;
    }
    Some(hour * 60 + minute)
}

fn weekday(raw: &str) -> Option<u8> {
    (0u8..)
        .zip(["sun", "mon", "tue", "wed", "thu", "fri", "sat"])
        .find_map(|(day, name)| raw.eq_ignore_ascii_case(name).then_some(day))
}

fn parse_days(raw: &str) -> Option<u8> {
    let mut days = 0;
    for part in raw.split(',') {
        if let Some((start, end)) = part.split_once('-') {
            let mut day = weekday(start)?;
            let end = weekday(end)?;
            loop {
                days |= 1 << day;
                if day == end {
                    break;
                }
                day = (day + 1) % 7;
            }
        } else {
            days |= 1 << weekday(part)?;
        }
    }
    Some(days)
}

impl TimeWindow {
    /// Whether the instant is within the half-open interval, including overnight windows.
    #[must_use]
    pub fn contains(&self, unix_secs: i64) -> bool {
        // Wider arithmetic also handles timestamps at either i64 boundary.
        let local = i128::from(unix_secs) + i128::from(self.offset_secs);
        let day = (local.div_euclid(86_400) + 4).rem_euclid(7);
        let minute = local.rem_euclid(86_400) / 60;
        let start = i128::from(self.start);
        let end = i128::from(self.end);
        let selected = |day: i128| self.days & (1 << day) != 0;
        if self.end > self.start {
            selected(day) && minute >= start && minute < end
        } else {
            (selected(day) && minute >= start) || (selected((day + 6) % 7) && minute < end)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::parse_time_window;

    #[test]
    fn window_end_starts_at_one_minute_after_midnight() {
        // Plan grammar: end := 00:01..24:00, with 24:00 reserved for ends.
        for raw in ["23:00-00:00", "mon 22:00-00:00 +09:00"] {
            assert!(parse_time_window(raw).is_err(), "accepted {raw}");
        }
        assert!(parse_time_window("23:00-00:01").is_ok());
        assert!(parse_time_window("23:00-24:00").is_ok());
    }
}
