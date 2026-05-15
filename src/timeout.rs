use std::time::Duration;

use crate::error::{CruiseError, Result};

pub fn parse_timeout(s: &str) -> Result<Duration> {
    if s.is_empty() {
        return Err(CruiseError::InvalidStepConfig(
            "timeout must be positive".to_string(),
        ));
    }

    let (num_part, multiplier) = match s.as_bytes()[s.len() - 1] {
        b'h' => (&s[..s.len() - 1], 3600u64),
        b'm' => (&s[..s.len() - 1], 60u64),
        _ => (s, 1u64),
    };

    let value: u64 = num_part
        .parse()
        .map_err(|_| CruiseError::InvalidStepConfig(format!("invalid timeout: '{s}'")))?;

    if value == 0 {
        return Err(CruiseError::InvalidStepConfig(
            "timeout must be positive".to_string(),
        ));
    }

    Ok(Duration::from_secs(value * multiplier))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::err_string;

    #[test]
    fn parse_timeout_plain_digits() {
        let result = parse_timeout("30").unwrap_or_else(|e| panic!("{e:?}"));
        assert_eq!(result, Duration::from_secs(30));
    }

    #[test]
    fn parse_timeout_minutes() {
        let result = parse_timeout("5m").unwrap_or_else(|e| panic!("{e:?}"));
        assert_eq!(result, Duration::from_secs(5 * 60));
    }

    #[test]
    fn parse_timeout_hours() {
        let result = parse_timeout("1h").unwrap_or_else(|e| panic!("{e:?}"));
        assert_eq!(result, Duration::from_secs(3600));
    }

    #[test]
    fn parse_timeout_zero_is_error() {
        let result = parse_timeout("0");
        assert!(result.is_err(), "expected Err for '0'");
        let msg = err_string(result);
        assert!(
            msg.contains("positive"),
            "error should mention positive, got: {msg}"
        );
    }

    #[test]
    fn parse_timeout_empty_is_error() {
        let result = parse_timeout("");
        assert!(result.is_err(), "expected Err for empty string");
        let msg = err_string(result);
        assert!(
            msg.contains("positive"),
            "error should mention positive, got: {msg}"
        );
    }

    #[test]
    fn parse_timeout_rejects_5min() {
        let result = parse_timeout("5min");
        assert!(result.is_err(), "expected Err for '5min'");
    }

    #[test]
    fn parse_timeout_rejects_negative() {
        let result = parse_timeout("-5");
        assert!(result.is_err(), "expected Err for '-5'");
    }

    #[test]
    fn parse_timeout_rejects_abc() {
        let result = parse_timeout("abc");
        assert!(result.is_err(), "expected Err for 'abc'");
    }

    #[test]
    fn parse_timeout_large_value() {
        let result = parse_timeout("24h").unwrap_or_else(|e| panic!("{e:?}"));
        assert_eq!(result, Duration::from_secs(24 * 3600));
    }

    #[test]
    fn parse_timeout_single_minute() {
        let result = parse_timeout("1m").unwrap_or_else(|e| panic!("{e:?}"));
        assert_eq!(result, Duration::from_secs(60));
    }
}
