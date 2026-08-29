//! Reasoning effort tiers and the `model[:effort]` suffix split.

/// Reasoning effort level forwarded to backends that support it.
///
/// The five tiers are the ones a cruise `model[:effort]` reference has always
/// accepted (`low`, `medium`, `high`, `xhigh`, `max`); each backend maps them to
/// its own reasoning-effort knob (the `claude` CLI spells them as `--effort`
/// values verbatim).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EffortLevel {
    Low,
    Medium,
    High,
    XHigh,
    Max,
}

impl EffortLevel {
    /// The tier's wire name, as the backends spell it on the command line.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Low => "low",
            Self::Medium => "medium",
            Self::High => "high",
            Self::XHigh => "xhigh",
            Self::Max => "max",
        }
    }
}

/// Every `:suffix` recognized on a model reference, paired with the effort tier
/// it selects. Compared case-insensitively after trimming.
///
/// Some recognized suffixes have no tier: they are stripped off the model id but
/// leave the effort unset, so the backend runs at its own default. `off` /
/// `none` / `0` mean "no extended thinking", which is not an effort tier at all;
/// `5` is only an alias spelling that cruise has never mapped to a tier.
const THINKING_SUFFIXES: &[(&str, Option<EffortLevel>)] = &[
    ("off", None),
    ("none", None),
    ("0", None),
    ("minimal", Some(EffortLevel::Low)),
    ("min", Some(EffortLevel::Low)),
    ("low", Some(EffortLevel::Low)),
    ("1", Some(EffortLevel::Low)),
    ("medium", Some(EffortLevel::Medium)),
    ("med", Some(EffortLevel::Medium)),
    ("2", Some(EffortLevel::Medium)),
    ("high", Some(EffortLevel::High)),
    ("3", Some(EffortLevel::High)),
    ("xhigh", Some(EffortLevel::XHigh)),
    ("4", Some(EffortLevel::XHigh)),
    ("max", Some(EffortLevel::Max)),
    ("5", None),
];

/// The [`THINKING_SUFFIXES`] entry for `suffix`, or `None` when it is not a
/// recognized suffix at all.
fn lookup(suffix: &str) -> Option<&'static (&'static str, Option<EffortLevel>)> {
    let suffix = suffix.trim();
    THINKING_SUFFIXES
        .iter()
        .find(|(spelling, _)| suffix.eq_ignore_ascii_case(spelling))
}

/// The effort tier a recognized `:suffix` selects, or `None` when the suffix is
/// unrecognized or carries no tier (see [`THINKING_SUFFIXES`]).
#[must_use]
pub fn effort_from_suffix(suffix: &str) -> Option<EffortLevel> {
    lookup(suffix).and_then(|&(_, effort)| effort)
}

/// Splits a trailing `:effort` suffix off a model reference, returning
/// `(model, effort)`.
///
/// The suffix is recognized only when it is one of [`THINKING_SUFFIXES`] --
/// including the spellings that carry no tier, which must still be stripped so
/// they never reach the provider as part of the model id. Anything else stays
/// part of the model name, so model ids with a legitimate `:` -- e.g.
/// `OpenRouter` variants like `meta-llama/llama-3.1-8b-instruct:free` -- pass
/// through untouched.
///
/// The suffix is returned verbatim; use [`effort_from_suffix`] to normalize it
/// into a tier.
#[must_use]
pub fn split_thinking_suffix(model: &str) -> (&str, Option<&str>) {
    match model.rsplit_once(':') {
        Some((m, t)) if lookup(t).is_some() => (m, Some(t)),
        _ => (model, None),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tier_wire_names_match_the_suffix_spellings_that_select_them() {
        for tier in [
            EffortLevel::Low,
            EffortLevel::Medium,
            EffortLevel::High,
            EffortLevel::XHigh,
            EffortLevel::Max,
        ] {
            assert_eq!(
                effort_from_suffix(tier.as_str()),
                Some(tier),
                "tier {tier:?} does not round-trip through its wire name"
            );
        }
    }

    #[test]
    fn effort_from_suffix_maps_every_alias() {
        let cases = [
            ("minimal", Some(EffortLevel::Low)),
            ("min", Some(EffortLevel::Low)),
            ("low", Some(EffortLevel::Low)),
            ("1", Some(EffortLevel::Low)),
            ("medium", Some(EffortLevel::Medium)),
            ("med", Some(EffortLevel::Medium)),
            ("2", Some(EffortLevel::Medium)),
            ("high", Some(EffortLevel::High)),
            ("3", Some(EffortLevel::High)),
            ("xhigh", Some(EffortLevel::XHigh)),
            ("4", Some(EffortLevel::XHigh)),
            ("max", Some(EffortLevel::Max)),
            // Recognized suffixes without a tier.
            ("off", None),
            ("none", None),
            ("0", None),
            ("5", None),
            // Not a suffix at all.
            ("free", None),
            ("", None),
        ];
        for (input, expected) in cases {
            assert_eq!(effort_from_suffix(input), expected, "input: {input:?}");
        }
    }

    #[test]
    fn effort_from_suffix_ignores_case_and_surrounding_space() {
        assert_eq!(effort_from_suffix("  XHigh "), Some(EffortLevel::XHigh));
        assert_eq!(effort_from_suffix("MED"), Some(EffortLevel::Medium));
    }

    #[test]
    fn splits_every_recognized_suffix_including_the_tierless_ones() {
        for (spelling, _) in THINKING_SUFFIXES {
            let model = format!("anthropic/claude-sonnet-4-6:{spelling}");
            assert_eq!(
                split_thinking_suffix(&model),
                ("anthropic/claude-sonnet-4-6", Some(*spelling)),
                "suffix {spelling:?} was not split off"
            );
        }
    }

    #[test]
    fn keeps_unrecognized_suffixes_in_the_model_name() {
        // An OpenRouter free-tier variant, not an effort suffix.
        assert_eq!(
            split_thinking_suffix("meta-llama/llama-3.1-8b-instruct:free"),
            ("meta-llama/llama-3.1-8b-instruct:free", None)
        );
        assert_eq!(split_thinking_suffix("opus-4.7"), ("opus-4.7", None));
        // A trailing empty suffix is not a level; left untouched.
        assert_eq!(split_thinking_suffix("opus-4.7:"), ("opus-4.7:", None));
    }

    #[test]
    fn splits_only_the_last_colon_suffix() {
        assert_eq!(
            split_thinking_suffix("llama-3.1:free:low"),
            ("llama-3.1:free", Some("low"))
        );
    }

    #[test]
    fn splits_a_suffix_only_reference_into_an_empty_model() {
        assert_eq!(split_thinking_suffix(":high"), ("", Some("high")));
    }

    /// The port must accept exactly what seher's `split_thinking_suffix` did, or
    /// existing `model:` values silently turn into unknown model ids. Deleted
    /// together with the `seher-sdk` dependency.
    #[test]
    fn matches_seher_split_thinking_suffix() {
        let inputs = [
            "opus-4.7:off",
            "opus-4.7:none",
            "opus-4.7:0",
            "opus-4.7:minimal",
            "opus-4.7:min",
            "opus-4.7:low",
            "opus-4.7:1",
            "opus-4.7:medium",
            "opus-4.7:med",
            "opus-4.7:2",
            "opus-4.7:high",
            "opus-4.7:3",
            "opus-4.7:xhigh",
            "opus-4.7:4",
            "opus-4.7:max",
            "opus-4.7:5",
            "opus-4.7:HIGH",
            "opus-4.7: high",
            "opus-4.7:free",
            "opus-4.7:",
            "opus-4.7",
            ":high",
            "llama-3.1:free:low",
            "meta-llama/llama-3.1-8b-instruct:free",
        ];
        for input in inputs {
            assert_eq!(
                split_thinking_suffix(input),
                seher::sdk::split_thinking_suffix(input),
                "input: {input:?}"
            );
        }
    }
}
