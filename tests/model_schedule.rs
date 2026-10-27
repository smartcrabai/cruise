//! Pure schedule contracts. The planned module is included directly until the
//! next production step registers it in the library and runtime.
use cruise::error;
#[path = "../src/model_schedule.rs"]
mod model_schedule;
use model_schedule::{TimeWindow, parse_time_window};

const DAY: i64 = 86_400;
// 1970-01-05 was Monday. Fixtures include pre-epoch dates below.
const MONDAY: i64 = 4 * DAY;

fn window(raw: &str) -> TimeWindow {
    parse_time_window(raw).unwrap_or_else(|error| panic!("valid window {raw:?}: {error}"))
}

#[test]
fn documented_schedule_grammar_is_accepted() {
    for raw in [
        "mon-fri 01:00-04:00",
        "01:00-04:00",
        "Mon-Fri 01:00-04:00",
        "sat,sun 01:00-04:00",
        "mon,wed-fri 01:00-04:00",
        "fri-mon 01:00-04:00",
        "mon-fri 10:00-13:00 +09:00",
        "01:00-04:00 -05:30",
        "01:00-04:00 UTC",
        "01:00-04:00 Z",
        "00:00-24:00",
        "00:00-24:00 +14:00",
        "00:00-24:00 -14:00",
    ] {
        let _ = window(raw);
    }
}

#[test]
fn malformed_windows_return_structured_configuration_errors() {
    for raw in [
        "",
        "mon-fri",
        "mon 01:00",
        "24:00-24:00",
        "24:00-01:00",
        "01:00-01:00",
        "01:00-25:00",
        "25:00-02:00",
        "01:60-02:00",
        "mon-xyz 01:00-04:00",
        "monday 01:00-04:00",
        "mön 01:00-04:00",
        "ſun 01:00-04:00",
        "01:00-04:00 +15:00",
        "01:00-04:00 +14:01",
        "01:00-04:00 -14:01",
        "01:00-04:00 +09:60",
        "01:00-04:00 UTC extra",
        "01:00-04:00 +09:00 extra",
        "mon-fri 01:00-04:00 UTC extra",
        "mon-fri 01:00-04:00 +09:00 extra",
        "01:00-04:00 America/New_York",
        "01:00-04:00 local",
    ] {
        match parse_time_window(raw) {
            Err(error::CruiseError::InvalidStepConfig(message)) => {
                assert!(!message.is_empty(), "missing diagnostic for {raw:?}");
            }
            Err(error) => panic!("wrong error for {raw:?}: {error}"),
            Ok(_) => panic!("accepted malformed schedule {raw:?}"),
        }
    }
}

#[test]
fn weekday_peak_intervals_are_half_open_to_the_second() {
    let peak = window("mon-fri 01:00-04:00");
    for (seconds, expected) in [
        (MONDAY + 59 * 60, false),
        (MONDAY + 3600 - 1, false),
        (MONDAY + 3600, true),
        (MONDAY + 3 * 3600 + 59 * 60, true),
        (MONDAY + 4 * 3600 - 1, true),
        (MONDAY + 4 * 3600, false),
        (MONDAY + 5 * DAY + 2 * 3600, false),
    ] {
        assert_eq!(peak.contains(seconds), expected, "unix_secs={seconds}");
    }
}

#[test]
fn omitted_days_and_full_day_cover_every_weekday() {
    let full = window("00:00-24:00");
    for day in -8..8 {
        for second in [0, 1, DAY - 1] {
            assert!(full.contains(day * DAY + second));
        }
    }
}

#[test]
fn day_lists_ranges_and_case_select_the_declared_weekdays() {
    for (raw, days) in [
        ("Mon-Fri 01:00-04:00", [0, 1, 2, 3, 4].as_slice()),
        ("sat,sun 01:00-04:00", [5, 6].as_slice()),
        ("fri-mon 01:00-04:00", [0, 4, 5, 6].as_slice()),
        ("mon,wed-fri 01:00-04:00", [0, 2, 3, 4].as_slice()),
    ] {
        let schedule = window(raw);
        for day in 0..7 {
            assert_eq!(
                schedule.contains(MONDAY + day * DAY + 2 * 3600),
                days.contains(&day),
                "{raw}, day={day}"
            );
        }
    }
}

#[test]
fn fixed_positive_offset_is_equivalent_to_utc_peak_intervals() {
    let utc = window("mon-fri 01:00-04:00");
    let shifted = window("mon-fri 10:00-13:00 +09:00");
    for minute in 0..7 * 24 * 60 {
        let seconds = MONDAY + minute * 60;
        assert_eq!(
            utc.contains(seconds),
            shifted.contains(seconds),
            "minute={minute}"
        );
    }
}

#[test]
fn utc_and_z_offsets_have_the_same_membership_as_omitted_offset() {
    for suffix in ["UTC", "Z"] {
        let explicit = window(&format!("mon 01:00-04:00 {suffix}"));
        let implicit = window("mon 01:00-04:00");
        for second in [MONDAY, MONDAY + 3600, MONDAY + 4 * 3600] {
            assert_eq!(explicit.contains(second), implicit.contains(second));
        }
    }
}

#[test]
fn overnight_days_refer_to_the_start_day() {
    let overnight = window("fri 22:00-02:00");
    for (seconds, expected) in [
        (MONDAY + 4 * DAY + 22 * 3600 - 1, false),
        (MONDAY + 4 * DAY + 22 * 3600, true),
        (MONDAY + 5 * DAY + 3600, true),
        (MONDAY + 5 * DAY + 2 * 3600, false),
        (MONDAY + 6 * DAY + 3600, false),
    ] {
        assert_eq!(overnight.contains(seconds), expected, "unix_secs={seconds}");
    }
}

#[test]
fn negative_offset_uses_local_start_day_across_utc_midnight() {
    let friday = window("fri 22:00-24:00 -05:30");
    let saturday = MONDAY + 5 * DAY;
    assert!(!friday.contains(saturday + 3 * 3600 + 30 * 60 - 1));
    assert!(friday.contains(saturday + 3 * 3600 + 30 * 60));
    assert!(friday.contains(saturday + 5 * 3600 + 30 * 60 - 1));
    assert!(!friday.contains(saturday + 5 * 3600 + 30 * 60));
}

#[test]
fn epoch_and_negative_unix_seconds_have_correct_weekdays() {
    assert!(window("thu 00:00-24:00").contains(0));
    assert!(!window("wed 00:00-24:00").contains(0));
    assert!(window("wed 00:00-24:00").contains(-1));
    assert!(window("wed 23:00-24:00 -01:00").contains(0));
    assert!(window("fri 00:00-01:00 +01:00").contains(DAY - 3600));
}
