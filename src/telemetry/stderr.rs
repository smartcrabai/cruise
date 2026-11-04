//! Bounded, best-effort sanitization for command stderr before it enters telemetry.
//!
//! Redaction is performed over the complete input before the excerpt cap is applied. When
//! sanitized output exceeds [`MAX_STDERR_LOG_BYTES`], the result is the first 4,080 UTF-8
//! bytes, an omission marker, and the last 4,080 UTF-8 bytes. Secret detection is pattern
//! based: arbitrary plaintext secrets and PII that do not match a recognized format cannot
//! be guaranteed to be removed.

use std::collections::VecDeque;

pub(super) const MAX_STDERR_LOG_BYTES: usize = 8192;
const EXCERPT_HEAD_BYTES: usize = 4080;
const EXCERPT_TAIL_BYTES: usize = 4080;
const EXCERPT_MARKER: &str = "\n[... excerpt omitted ...]\n";
const REDACTED: &str = "[REDACTED]";
const REDACTED_PRIVATE_KEY: &str = "[REDACTED PRIVATE KEY]";
const MAX_PEM_MARKER_SCAN_BYTES: usize = 128;

pub(super) struct SanitizedStderr {
    pub(super) body: String,
    pub(super) truncated: bool,
}

/// Remove recognized credentials, terminal controls, and URL secrets without retaining an
/// unbounded copy of the input. Text selection is done only after the full input is scanned.
pub(super) fn sanitize(input: &str) -> SanitizedStderr {
    let mut output = BoundedOutput::new();
    let mut state = ScanState::default();

    for raw_line in input.split_inclusive('\n') {
        let (line, has_newline) = match raw_line.strip_suffix('\n') {
            Some(line) => (line, true),
            None => (raw_line, false),
        };
        if !is_machine_diagnostic_line(line) {
            sanitize_line(line, &mut state, &mut output);
        }
        if has_newline {
            output.push('\n');
        }
    }

    let (body, truncated, meaningful) = output.finish();
    SanitizedStderr {
        body: if meaningful { body } else { String::new() },
        truncated,
    }
}

#[derive(Default)]
struct ScanState {
    in_private_key: bool,
    redact_header_continuation: bool,
    redact_quoted_value: Option<u8>,
}

fn sanitize_line(line: &str, state: &mut ScanState, output: &mut BoundedOutput) {
    let mut cursor = 0;

    if let Some(quote) = state.redact_quoted_value {
        match find_visible_quote_end(line, 0, quote) {
            Some(end) => {
                output.push(quote as char);
                cursor = end;
                state.redact_quoted_value = None;
            }
            None => return,
        }
    }

    if state.redact_header_continuation {
        if line_starts_with_horizontal_space(line) {
            return;
        }
        state.redact_header_continuation = false;
    }

    if state.in_private_key {
        if let Some(end) = find_private_key_end(line, cursor) {
            state.in_private_key = false;
            cursor = end;
        } else {
            return;
        }
    }

    if cursor == 0
        && let Some(colon) = sensitive_header_colon(line)
    {
        emit_clean(&line[..=colon], output);
        let value_start = skip_visible_horizontal_space(line, colon + 1);
        if value_start > colon + 1 {
            emit_clean(&line[colon + 1..value_start], output);
        }
        output.push_str(REDACTED);
        state.redact_header_continuation = true;
        return;
    }

    sanitize_line_content(line, cursor, state, output);
}

fn sanitize_line_content(
    line: &str,
    mut index: usize,
    state: &mut ScanState,
    output: &mut BoundedOutput,
) {
    let bytes = line.as_bytes();
    let mut previous_key_byte = false;
    let mut previous_url_scheme_byte = false;
    let mut shell_quote = None;
    let mut shell_quote_escaped = false;
    while index < bytes.len() {
        if let Some(end) = ansi_escape_end(bytes, index) {
            index = end;
            continue;
        }

        if let Some(end) = private_key_start_end(line, index) {
            output.push_str(REDACTED_PRIVATE_KEY);
            if let Some(end_marker) = find_private_key_end(line, end) {
                index = end_marker;
            } else {
                state.in_private_key = true;
                return;
            }
            previous_key_byte = false;
            previous_url_scheme_byte = false;
            continue;
        }

        if let Some(url) = url_at(line, index, previous_url_scheme_byte) {
            emit_url(line, &url, output);
            index = url.end;
            previous_key_byte = false;
            previous_url_scheme_byte = false;
            continue;
        }

        if let Some((scheme_end, credential_end)) = bearer_or_basic(line, index) {
            emit_clean(&line[index..scheme_end], output);
            output.push(' ');
            output.push_str(REDACTED);
            index = credential_end;
            previous_key_byte = false;
            previous_url_scheme_byte = false;
            continue;
        }

        if !previous_key_byte && let Some(token_end) = known_token_end(line, index) {
            output.push_str(REDACTED);
            index = token_end;
            previous_key_byte = false;
            previous_url_scheme_byte = false;
            continue;
        }

        if !previous_key_byte && let Some(assignment) = secret_assignment(line, index, shell_quote)
        {
            emit_clean(&line[index..assignment.value_start], output);
            if let Some(open) = assignment.open_quote {
                output.push(open as char);
            }
            output.push_str(REDACTED);
            if let Some(close) = assignment.close_quote {
                output.push(close as char);
            }
            if assignment.unclosed_quote.is_some() {
                state.redact_quoted_value = assignment.unclosed_quote;
                return;
            }
            index = assignment.value_end;
            previous_key_byte = false;
            previous_url_scheme_byte = false;
            continue;
        }

        let character = line[index..].chars().next().unwrap_or('\u{fffd}');
        if character.is_control() {
            index += character.len_utf8();
            continue;
        }
        if character == '\t' {
            output.push(' ');
        } else {
            output.push(character);
        }
        update_shell_quote(character, &mut shell_quote, &mut shell_quote_escaped);
        previous_key_byte = character.is_ascii() && is_key_byte(character as u8);
        previous_url_scheme_byte = character.is_ascii() && is_url_scheme_byte(character as u8);
        index += character.len_utf8();
    }
}

struct Assignment {
    value_start: usize,
    value_end: usize,
    open_quote: Option<u8>,
    close_quote: Option<u8>,
    unclosed_quote: Option<u8>,
}

const MAX_FIELD_NAME_BYTES: usize = 256;

struct AssignmentKey {
    bytes: [u8; MAX_FIELD_NAME_BYTES],
    len: usize,
    overlong: bool,
    end: usize,
}

fn secret_assignment(
    line: &str,
    index: usize,
    surrounding_quote: Option<char>,
) -> Option<Assignment> {
    let key = assignment_key(line, index)?;
    let key_bytes = &key.bytes[..key.len];
    if key.len == 0 || (!key.overlong && !is_sensitive_key(key_bytes)) {
        return None;
    }
    let redact_remainder = key.overlong || is_authentication_key(key_bytes);
    let command_flag = key_bytes.starts_with(b"--");
    let mut probe = key.end;
    let mut had_space = false;
    let (separator, separator_start) = loop {
        let (character, source_index) = next_visible_char(line, &mut probe)?;
        if matches!(character, ' ' | '\t') {
            had_space = true;
            continue;
        }
        break (character, source_index);
    };

    let (value_start, first_value) = if matches!(separator, '=' | ':') {
        let (value, source_index) = next_non_horizontal_space(line, &mut probe)?;
        if matches!(value, ',' | ';' | '}' | ']') {
            return None;
        }
        (source_index, value)
    } else if command_flag && had_space {
        (separator_start, separator)
    } else {
        return None;
    };
    assignment_value(
        line,
        value_start,
        first_value,
        probe,
        surrounding_quote,
        redact_remainder,
    )
}

fn assignment_key(line: &str, index: usize) -> Option<AssignmentKey> {
    let mut cursor = index;
    let (first, _) = next_visible_char(line, &mut cursor)?;
    let quoted = first == '"';
    let mut bytes = [0u8; MAX_FIELD_NAME_BYTES];
    let mut len = 0;
    let mut overlong = false;

    if quoted {
        loop {
            let (character, _) = next_visible_char(line, &mut cursor)?;
            if character == '"' {
                break;
            }
            if !character.is_ascii() || !is_key_byte(character as u8) {
                return None;
            }
            append_field_byte(&mut bytes, &mut len, &mut overlong, character as u8);
        }
    } else {
        cursor = index;
        while let Some((character, source_index)) = next_visible_char(line, &mut cursor) {
            if character.is_ascii() && is_key_byte(character as u8) {
                append_field_byte(&mut bytes, &mut len, &mut overlong, character as u8);
            } else {
                cursor = source_index;
                break;
            }
        }
    }
    Some(AssignmentKey {
        bytes,
        len,
        overlong,
        end: cursor,
    })
}

fn assignment_value(
    line: &str,
    value_start: usize,
    first_value: char,
    mut probe: usize,
    surrounding_quote: Option<char>,
    redact_remainder: bool,
) -> Option<Assignment> {
    if first_value == '\'' || first_value == '"' {
        return Some(
            match find_visible_quote_end(line, probe, first_value as u8) {
                Some(end) => Assignment {
                    value_start,
                    value_end: end,
                    open_quote: Some(first_value as u8),
                    close_quote: Some(first_value as u8),
                    unclosed_quote: None,
                },
                None => Assignment {
                    value_start,
                    value_end: line.len(),
                    open_quote: Some(first_value as u8),
                    close_quote: None,
                    unclosed_quote: Some(first_value as u8),
                },
            },
        );
    }

    let mut end = probe;
    let mut quote = surrounding_quote;
    let mut escaped = false;
    while let Some((character, source_index)) = next_visible_char(line, &mut probe) {
        if escaped {
            escaped = false;
        } else if character == '\\' && quote != Some('\'') {
            escaped = true;
        } else if let Some(delimiter) = quote {
            if character == delimiter {
                end = source_index;
                break;
            }
        } else if character == '\'' || character == '"' {
            quote = Some(character);
        } else if character.is_whitespace() || character == ';' {
            end = source_index;
            break;
        }
        end = probe;
    }
    (end > value_start).then_some(Assignment {
        value_start,
        value_end: if redact_remainder { line.len() } else { end },
        open_quote: None,
        close_quote: None,
        unclosed_quote: None,
    })
}
fn update_shell_quote(character: char, quote: &mut Option<char>, escaped: &mut bool) {
    if *escaped {
        *escaped = false;
    } else if character == '\\' && *quote != Some('\'') {
        *escaped = true;
    } else if let Some(delimiter) = *quote {
        if character == delimiter {
            *quote = None;
        }
    } else if character == '\'' || character == '"' {
        *quote = Some(character);
    }
}

fn append_field_byte(buffer: &mut [u8], length: &mut usize, overlong: &mut bool, byte: u8) {
    if *length < buffer.len() {
        buffer[*length] = byte;
        *length += 1;
    } else {
        *overlong = true;
    }
}

fn find_visible_quote_end(line: &str, mut cursor: usize, quote: u8) -> Option<usize> {
    let mut escaped = false;
    while let Some((character, _)) = next_visible_char(line, &mut cursor) {
        if escaped {
            escaped = false;
        } else if character == '\\' {
            escaped = true;
        } else if character as u32 == u32::from(quote) {
            return Some(cursor);
        }
    }
    None
}

fn is_key_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-' | b'.')
}

fn is_sensitive_key(key: &[u8]) -> bool {
    let mut segment_start = 0;
    for index in 0..key.len() {
        if matches!(key[index], b'_' | b'-' | b'.') {
            if is_sensitive_segment(&key[segment_start..index]) {
                return true;
            }
            segment_start = index + 1;
        } else if index > segment_start
            && key[index].is_ascii_uppercase()
            && (key[index - 1].is_ascii_lowercase()
                || key[index - 1].is_ascii_digit()
                || (key[index - 1].is_ascii_uppercase()
                    && key.get(index + 1).is_some_and(u8::is_ascii_lowercase)))
        {
            if is_sensitive_segment(&key[segment_start..index]) {
                return true;
            }
            segment_start = index;
        }
    }
    is_sensitive_segment(&key[segment_start..])
}

fn is_authentication_key(key: &[u8]) -> bool {
    let mut segment_start = 0;
    for index in 0..key.len() {
        let separator = matches!(key[index], b'_' | b'-' | b'.');
        let camel_boundary = index > segment_start
            && key[index].is_ascii_uppercase()
            && (key[index - 1].is_ascii_lowercase()
                || key[index - 1].is_ascii_digit()
                || (key[index - 1].is_ascii_uppercase()
                    && key.get(index + 1).is_some_and(u8::is_ascii_lowercase)));
        if separator || camel_boundary {
            if is_authentication_segment(&key[segment_start..index]) {
                return true;
            }
            segment_start = if separator { index + 1 } else { index };
        }
    }
    is_authentication_segment(&key[segment_start..])
}

fn is_authentication_segment(segment: &[u8]) -> bool {
    eq_ascii_case(segment, b"auth")
        || eq_ascii_case(segment, b"authorization")
        || eq_ascii_case(segment, b"authentication")
}

fn is_sensitive_segment(segment: &[u8]) -> bool {
    eq_ascii_case(segment, b"secret")
        || eq_ascii_case(segment, b"token")
        || eq_ascii_case(segment, b"password")
        || eq_ascii_case(segment, b"passwd")
        || eq_ascii_case(segment, b"passphrase")
        || eq_ascii_case(segment, b"credential")
        || eq_ascii_case(segment, b"authorization")
        || eq_ascii_case(segment, b"authentication")
        || eq_ascii_case(segment, b"auth")
        || eq_ascii_case(segment, b"cookie")
        || eq_ascii_case(segment, b"credentials")
        || eq_ascii_case(segment, b"key")
}

fn is_machine_diagnostic_line(line: &str) -> bool {
    [
        b"CRUISE_CI_DIAGNOSTIC_V".as_slice(),
        b"CRUISE_SYNC_BASE_DIAGNOSTIC_V",
    ]
    .iter()
    .any(|marker| {
        let mut cursor = 0;
        while cursor < line.len() {
            if visible_ascii_prefix(line, cursor, marker).is_some() {
                return true;
            }
            if next_visible_char(line, &mut cursor).is_none() {
                break;
            }
        }
        false
    })
}

fn sensitive_header_colon(line: &str) -> Option<usize> {
    const MAX_HEADER_NAME_BYTES: usize = 256;
    let mut cursor = skip_visible_horizontal_space(line, 0);
    let first_position = cursor;
    let (first, _) = next_visible_char(line, &mut cursor)?;
    if matches!(first, '>' | '<' | '*') {
        cursor = skip_visible_horizontal_space(line, cursor);
    } else {
        cursor = first_position;
    }

    let mut name = [0u8; MAX_HEADER_NAME_BYTES];
    let mut name_len = 0;
    let mut overlong_name = false;
    let mut started = false;
    let mut after_name_space = false;
    let colon = loop {
        let (character, source_index) = next_visible_char(line, &mut cursor)?;
        if character == ':' {
            break source_index;
        }
        if matches!(character, ' ' | '\t') {
            if started {
                after_name_space = true;
            }
            continue;
        }
        if after_name_space || !character.is_ascii() || !is_key_byte(character as u8) {
            return None;
        }
        started = true;
        append_field_byte(
            &mut name,
            &mut name_len,
            &mut overlong_name,
            character as u8,
        );
    };

    let header_name = &name[..name_len];
    let explicit = [
        b"authorization".as_slice(),
        b"proxy-authorization",
        b"cookie",
        b"set-cookie",
        b"x-api-key",
        b"api-key",
        b"x-auth-token",
        b"x-access-token",
        b"x-refresh-token",
        b"x-github-token",
        b"x-gitlab-token",
        b"private-token",
        b"x-private-token",
        b"x-api-token",
        b"x-amz-security-token",
        b"x-goog-api-key",
        b"x-csrf-token",
    ];
    let sensitive = overlong_name
        || explicit
            .iter()
            .any(|header| eq_ascii_case(header_name, header))
        || is_sensitive_key(header_name);
    sensitive.then_some(colon)
}

fn line_starts_with_horizontal_space(line: &str) -> bool {
    let mut cursor = 0;
    next_visible_char(line, &mut cursor)
        .is_some_and(|(character, _)| matches!(character, ' ' | '\t'))
}

fn skip_visible_horizontal_space(line: &str, mut cursor: usize) -> usize {
    loop {
        let Some((character, source_index)) = next_visible_char(line, &mut cursor) else {
            return cursor;
        };
        if !matches!(character, ' ' | '\t') {
            return source_index;
        }
    }
}

fn next_non_horizontal_space(line: &str, cursor: &mut usize) -> Option<(char, usize)> {
    loop {
        let (character, source_index) = next_visible_char(line, cursor)?;
        if !matches!(character, ' ' | '\t') {
            return Some((character, source_index));
        }
    }
}

fn bearer_or_basic(line: &str, index: usize) -> Option<(usize, usize)> {
    let scheme_end = if let Some(end) = visible_ascii_prefix(line, index, b"bearer") {
        end
    } else {
        visible_ascii_prefix(line, index, b"basic")?
    };
    let mut cursor = scheme_end;
    let (separator, _) = next_visible_char(line, &mut cursor)?;
    if !matches!(separator, ' ' | '\t') {
        return None;
    }
    let (first, first_start) = next_non_horizontal_space(line, &mut cursor)?;
    if matches!(first, ',' | ';' | ')' | ']' | '}' | '"' | '\'') {
        return None;
    }
    let mut end = cursor;
    while let Some((character, source_index)) = next_visible_char(line, &mut cursor) {
        if character.is_whitespace()
            || matches!(character, ',' | ';' | ')' | ']' | '}' | '"' | '\'')
        {
            end = source_index;
            break;
        }
        end = cursor;
    }
    (end > first_start).then_some((scheme_end, end))
}

fn visible_ascii_prefix(line: &str, index: usize, prefix: &[u8]) -> Option<usize> {
    let mut cursor = index;
    for expected in prefix {
        let (character, _) = next_visible_char(line, &mut cursor)?;
        if !character.is_ascii() || !(character as u8).eq_ignore_ascii_case(expected) {
            return None;
        }
    }
    Some(cursor)
}

fn is_url_scheme_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || matches!(byte, b'+' | b'.' | b'-')
}

fn known_token_end(line: &str, index: usize) -> Option<usize> {
    let prefixes: [&[u8]; 30] = [
        b"github_pat_",
        b"ghp_",
        b"gho_",
        b"ghu_",
        b"ghs_",
        b"ghr_",
        b"glpat-",
        b"gldt-",
        b"glrt-",
        b"glcbt-",
        b"xoxb-",
        b"xoxp-",
        b"xoxa-",
        b"xoxr-",
        b"xoxs-",
        b"xoxc-",
        b"sk_live_",
        b"rk_live_",
        b"sk_test_",
        b"sk-ant-api03-",
        b"sk-ant-",
        b"sk-proj-",
        b"sk-svcacct-",
        b"sk-",
        b"hf_",
        b"sb_secret_",
        b"vercel_",
        b"lin_api_",
        b"npm_",
        b"pypi-",
    ];
    if prefixes
        .iter()
        .any(|prefix| visible_ascii_prefix(line, index, prefix).is_some())
        || visible_ascii_prefix(line, index, b"AIza").is_some()
    {
        return Some(visible_token_word_end(line, index));
    }

    let aws_prefix_end = visible_ascii_prefix(line, index, b"AKIA")
        .or_else(|| visible_ascii_prefix(line, index, b"ASIA"));
    if let Some(mut cursor) = aws_prefix_end {
        let mut valid = true;
        for _ in 0..16 {
            let (character, _) = next_visible_char(line, &mut cursor)?;
            if !character.is_ascii_uppercase() && !character.is_ascii_digit() {
                valid = false;
                break;
            }
        }
        if valid {
            return Some(visible_token_word_end(line, index));
        }
    }

    if visible_ascii_prefix(line, index, b"eyJ").is_some() {
        let end = visible_token_word_end(line, index);
        let mut cursor = index;
        let mut segment_len = 0;
        let mut dots = 0;
        let mut valid = true;
        while cursor < end {
            let (character, _) = next_visible_char(line, &mut cursor)?;
            if character == '.' {
                if segment_len == 0 {
                    valid = false;
                    break;
                }
                dots += 1;
                segment_len = 0;
            } else {
                segment_len += 1;
            }
        }
        if valid && dots == 2 && segment_len > 0 {
            return Some(end);
        }
    }
    None
}

fn visible_token_word_end(line: &str, index: usize) -> usize {
    let mut cursor = index;
    let mut end = index;
    while let Some((character, _)) = next_visible_char(line, &mut cursor) {
        if character.is_ascii() && is_key_byte(character as u8) {
            end = cursor;
        } else {
            break;
        }
    }
    end
}

fn url_at(line: &str, index: usize, preceded_by_scheme_byte: bool) -> Option<ParsedUrl> {
    if preceded_by_scheme_byte {
        return None;
    }
    let mut cursor = index;
    let (first, _) = next_visible_char(line, &mut cursor)?;
    if !first.is_ascii_alphabetic() {
        return None;
    }
    loop {
        let (character, _) = next_visible_char(line, &mut cursor)?;
        if character == ':' {
            break;
        }
        if !character.is_ascii() || !is_url_scheme_byte(character as u8) {
            return None;
        }
    }
    for _ in 0..2 {
        let (character, _) = next_visible_char(line, &mut cursor)?;
        if character != '/' {
            return None;
        }
    }

    let scheme_end = cursor;
    let authority_start = scheme_end;
    let mut end = line.len();
    let mut authority_end = line.len();
    let mut in_authority = true;
    let mut userinfo_end = None;
    let mut query_or_fragment = None;
    while let Some((character, source_index)) = next_visible_char(line, &mut cursor) {
        if character.is_whitespace() || matches!(character, '<' | '>' | '"' | '\'') {
            end = source_index;
            break;
        }
        if in_authority {
            match character {
                '/' | '?' | '#' => {
                    authority_end = source_index;
                    in_authority = false;
                    if matches!(character, '?' | '#') {
                        query_or_fragment = Some(source_index);
                    }
                }
                '@' => userinfo_end = Some(source_index),
                _ => {}
            }
        } else if query_or_fragment.is_none() && matches!(character, '?' | '#') {
            query_or_fragment = Some(source_index);
        }
    }
    if in_authority {
        authority_end = end;
    }

    Some(ParsedUrl {
        start: index,
        end,
        scheme_end,
        authority_start,
        authority_end,
        userinfo_end,
        query_or_fragment,
    })
}

struct ParsedUrl {
    start: usize,
    end: usize,
    scheme_end: usize,
    authority_start: usize,
    authority_end: usize,
    userinfo_end: Option<usize>,
    query_or_fragment: Option<usize>,
}

fn emit_url(line: &str, url: &ParsedUrl, output: &mut BoundedOutput) {
    let bytes = line.as_bytes();
    emit_clean(&line[url.start..url.scheme_end], output);
    if let Some(at) = url.userinfo_end {
        output.push_str(REDACTED);
        output.push('@');
        emit_url_path(line, at + 1, url.authority_end, output);
    } else {
        emit_url_path(line, url.authority_start, url.authority_end, output);
    }

    let path_start = url.authority_end;
    match url.query_or_fragment {
        Some(secret_start) => {
            emit_url_path(line, path_start, secret_start, output);
            if bytes[secret_start] == b'?' {
                output.push_str("?[REDACTED]");
            } else {
                output.push_str("#[REDACTED]");
            }
        }
        None => emit_url_path(line, path_start, url.end, output),
    }
}

fn emit_url_path(line: &str, start: usize, end: usize, output: &mut BoundedOutput) {
    let bytes = line.as_bytes();
    let mut segment_start = start;
    let mut index = start;
    let mut previous_key_byte = false;
    while index < end {
        if let Some(escape_end) = ansi_escape_end(bytes, index) {
            index = escape_end;
            continue;
        }
        if !previous_key_byte && let Some(token_end) = known_token_end(line, index) {
            emit_clean(&line[segment_start..index], output);
            output.push_str(REDACTED);
            index = token_end.min(end);
            segment_start = index;
            previous_key_byte = false;
            continue;
        }
        let character = line[index..].chars().next().unwrap_or('\u{fffd}');
        if character.is_control() {
            index += character.len_utf8();
            continue;
        }
        previous_key_byte = character.is_ascii() && is_key_byte(character as u8);
        index += character.len_utf8();
    }
    emit_clean(&line[segment_start..end], output);
}

fn private_key_start_end(line: &str, index: usize) -> Option<usize> {
    let mut cursor = visible_ascii_prefix(line, index, b"-----BEGIN ")?;
    let key_type_start = cursor;
    while cursor.saturating_sub(key_type_start) <= MAX_PEM_MARKER_SCAN_BYTES {
        if let Some(end) = visible_ascii_prefix(line, cursor, b"PRIVATE KEY-----") {
            return Some(end);
        }
        let (character, _) = next_visible_char(line, &mut cursor)?;
        if !character.is_ascii_uppercase() && !matches!(character, ' ' | '-') {
            return None;
        }
    }
    None
}

fn find_private_key_end(line: &str, from: usize) -> Option<usize> {
    let mut cursor = from;
    while cursor < line.len() {
        if let Some(mut candidate) = visible_ascii_prefix(line, cursor, b"-----END ") {
            let key_type_start = candidate;
            while candidate.saturating_sub(key_type_start) <= MAX_PEM_MARKER_SCAN_BYTES {
                if let Some(end) = visible_ascii_prefix(line, candidate, b"PRIVATE KEY-----") {
                    return Some(end);
                }
                let Some((character, _)) = next_visible_char(line, &mut candidate) else {
                    break;
                };
                if !character.is_ascii_uppercase() && !matches!(character, ' ' | '-') {
                    break;
                }
            }
            cursor = candidate;
        }
        if next_visible_char(line, &mut cursor).is_none() {
            break;
        }
    }
    None
}

fn eq_ascii_case(left: &[u8], right: &[u8]) -> bool {
    left.eq_ignore_ascii_case(right)
}

fn ansi_escape_end(bytes: &[u8], index: usize) -> Option<usize> {
    if bytes.get(index) == Some(&0x1b) {
        let next = *bytes.get(index + 1)?;
        return match next {
            b'[' => Some(csi_end(bytes, index + 2)),
            b']' | b'P' | b'_' | b'^' | b'X' => Some(string_control_end(bytes, index + 2)),
            _ if next.is_ascii() => Some((index + 2).min(bytes.len())),
            _ => Some(index + 1),
        };
    }

    let introducer = bytes.get(index..index.saturating_add(2))?;
    if introducer[0] != 0xc2 {
        return None;
    }
    match introducer[1] {
        0x9b => Some(csi_end(bytes, index + 2)),
        0x90 | 0x98 | 0x9d | 0x9e | 0x9f => Some(string_control_end(bytes, index + 2)),
        _ => None,
    }
}

fn csi_end(bytes: &[u8], mut cursor: usize) -> usize {
    while let Some(byte) = bytes.get(cursor) {
        cursor += 1;
        if (0x40..=0x7e).contains(byte) {
            return cursor;
        }
    }
    bytes.len()
}

fn string_control_end(bytes: &[u8], mut cursor: usize) -> usize {
    while cursor < bytes.len() {
        if bytes[cursor] == 0x07 {
            return cursor + 1;
        }
        if bytes.get(cursor) == Some(&0xc2) && bytes.get(cursor + 1) == Some(&0x9c) {
            return cursor + 2;
        }
        if bytes[cursor] == 0x1b && bytes.get(cursor + 1) == Some(&b'\\') {
            return cursor + 2;
        }
        cursor += 1;
    }
    bytes.len()
}

fn next_visible_char(line: &str, cursor: &mut usize) -> Option<(char, usize)> {
    let bytes = line.as_bytes();
    while *cursor < bytes.len() {
        if let Some(end) = ansi_escape_end(bytes, *cursor) {
            *cursor = end;
            continue;
        }
        let source_index = *cursor;
        let character = line.get(source_index..)?.chars().next()?;
        *cursor += character.len_utf8();
        if character.is_control() && character != '\t' {
            continue;
        }
        return Some((character, source_index));
    }
    None
}

fn emit_clean(text: &str, output: &mut BoundedOutput) {
    let bytes = text.as_bytes();
    let mut index = 0;
    while index < bytes.len() {
        if let Some(end) = ansi_escape_end(bytes, index) {
            index = end;
            continue;
        }
        let character = text[index..].chars().next().unwrap_or('\u{fffd}');
        if character.is_control() {
            index += character.len_utf8();
            continue;
        }
        if character == '\t' {
            output.push(' ');
        } else {
            output.push(character);
        }
        index += character.len_utf8();
    }
}

struct BoundedOutput {
    full: Option<String>,
    head: String,
    tail: VecDeque<char>,
    tail_bytes: usize,
    truncated: bool,
    meaningful: bool,
}

impl BoundedOutput {
    fn new() -> Self {
        Self {
            full: Some(String::with_capacity(MAX_STDERR_LOG_BYTES)),
            head: String::with_capacity(EXCERPT_HEAD_BYTES),
            tail: VecDeque::with_capacity(EXCERPT_TAIL_BYTES),
            tail_bytes: 0,
            truncated: false,
            meaningful: false,
        }
    }

    fn push_str(&mut self, text: &str) {
        for character in text.chars() {
            self.push(character);
        }
    }

    fn push(&mut self, character: char) {
        self.meaningful |= !character.is_whitespace();
        if let Some(full) = self.full.as_mut()
            && full.len() + character.len_utf8() <= MAX_STDERR_LOG_BYTES
        {
            full.push(character);
            return;
        }

        if let Some(full) = self.full.take() {
            self.head
                .push_str(prefix_by_bytes(&full, EXCERPT_HEAD_BYTES));
            for character in suffix_by_bytes(&full, EXCERPT_TAIL_BYTES).chars() {
                self.tail.push_back(character);
                self.tail_bytes += character.len_utf8();
            }
            self.truncated = true;
        }

        self.tail.push_back(character);
        self.tail_bytes += character.len_utf8();
        while self.tail_bytes > EXCERPT_TAIL_BYTES {
            if let Some(removed) = self.tail.pop_front() {
                self.tail_bytes -= removed.len_utf8();
            }
        }
    }

    fn finish(self) -> (String, bool, bool) {
        let meaningful = self.meaningful;
        let truncated = self.truncated;
        if let Some(full) = self.full {
            return (full, false, meaningful);
        }
        let mut body =
            String::with_capacity(EXCERPT_HEAD_BYTES + EXCERPT_MARKER.len() + EXCERPT_TAIL_BYTES);
        body.push_str(&self.head);
        body.push_str(EXCERPT_MARKER);
        body.extend(self.tail);
        debug_assert!(body.len() <= MAX_STDERR_LOG_BYTES);
        (body, truncated, meaningful)
    }
}

fn prefix_by_bytes(text: &str, max_bytes: usize) -> &str {
    let mut end = text.len().min(max_bytes);
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    &text[..end]
}

fn suffix_by_bytes(text: &str, max_bytes: usize) -> &str {
    let mut start = text.len().saturating_sub(max_bytes);
    while !text.is_char_boundary(start) {
        start += 1;
    }
    &text[start..]
}

#[cfg(test)]
mod tests {
    use super::{MAX_STDERR_LOG_BYTES, REDACTED, sanitize};

    #[test]
    fn redacts_sensitive_headers_and_folded_values() {
        let result = sanitize(
            "Authorization: Bearer header-secret\n\tcontinued-cookie-secret\nProxy-Authorization: Basic proxy-secret\nCookie: session=cookie-secret\nSet-Cookie: sid=set-cookie-secret\nX-Api-Key: api-header-secret\nX-GitHub-Token: github-header-secret\nDone\n",
        );
        for secret in [
            "header-secret",
            "continued-cookie-secret",
            "proxy-secret",
            "cookie-secret",
            "set-cookie-secret",
            "api-header-secret",
            "github-header-secret",
        ] {
            assert!(!result.body.contains(secret));
        }
        assert!(result.body.contains("Authorization: [REDACTED]"));
        assert!(result.body.contains("Done"));
    }

    #[test]
    fn redacts_shell_and_json_secret_assignments() {
        let result = sanitize(
            "ok: retained\nPASSWORD=plain-secret TOKEN='quoted secret' --api-key \"flag secret\" {\"client_secret\":\"json secret\", \"refresh_token\": \"refresh secret\", \"accessToken\":\"camel secret\"}\n",
        );
        for secret in [
            "plain-secret",
            "quoted secret",
            "flag secret",
            "json secret",
            "refresh secret",
            "camel secret",
        ] {
            assert!(!result.body.contains(secret));
        }
        assert!(result.body.contains("ok: retained"));
    }

    #[test]
    fn redacts_shell_xtrace_assignments_with_quoted_or_escaped_spaces() {
        let result = sanitize(
            "+ export 'PASSWORD=quoted-first-secret quoted-second-secret'\n+ 'PASSPHRASE=quoted-passphrase-secret quoted-passphrase-suffix'\n+ PASSWORD=escaped-first-secret\\ escaped-second-secret\n",
        );
        for secret in [
            "quoted-first-secret",
            "quoted-second-secret",
            "quoted-passphrase-secret",
            "quoted-passphrase-suffix",
            "escaped-first-secret",
            "escaped-second-secret",
        ] {
            assert!(!result.body.contains(secret));
        }
        assert_eq!(
            result.body,
            "+ export 'PASSWORD=[REDACTED]'\n+ 'PASSPHRASE=[REDACTED]'\n+ PASSWORD=[REDACTED]\n"
        );
    }

    #[test]
    fn redacts_auth_schemes_and_known_token_shapes() {
        let result = sanitize(
            "Bearer bearer-secret Basic basic-secret ghp_GitHubToken123 glpat-GitLabToken123 xoxb-SlackToken123 AKIA1234567890ABCDEF eyJhbGciOiJIUzI1NiJ9.eyJzdWIiOiJ1c2VyIn0.signature sk-ant-api03-AnthropicSecret sk-proj-OpenAISecret hf_HuggingFaceSecret sb_secret_SupabaseSecret\n",
        );
        for secret in [
            "bearer-secret",
            "basic-secret",
            "ghp_GitHubToken123",
            "glpat-GitLabToken123",
            "xoxb-SlackToken123",
            "AKIA1234567890ABCDEF",
            "eyJhbGciOiJIUzI1NiJ9",
            "sk-ant-api03-AnthropicSecret",
            "sk-proj-OpenAISecret",
            "hf_HuggingFaceSecret",
            "sb_secret_SupabaseSecret",
        ] {
            assert!(!result.body.contains(secret));
        }
        assert!(result.body.contains("Bearer [REDACTED]"));
    }

    #[test]
    fn redacts_prefixed_authorization_without_losing_diagnosis() {
        let result = sanitize("rate limit: Authorization: Bearer retry-error-secret\n");
        assert!(!result.body.contains("retry-error-secret"));
        assert!(
            result
                .body
                .contains("rate limit: Authorization: [REDACTED]")
        );
    }

    #[test]
    fn removes_url_userinfo_query_and_fragment_secrets() {
        let result = sanitize(
            "connect https://user:password@example.test/path?access_token=query-secret&x=1#fragment-secret now; http://example.test/safe#private-fragment\n",
        );
        for secret in [
            "user:password",
            "query-secret",
            "fragment-secret",
            "private-fragment",
        ] {
            assert!(!result.body.contains(secret));
        }
        assert!(
            result
                .body
                .contains("https://[REDACTED]@example.test/path?[REDACTED]")
        );
        assert!(result.body.contains("connect "));
        assert!(result.body.contains(" now"));
    }

    #[test]
    fn recognizes_urls_with_ansi_and_control_split_syntax() {
        let result = sanitize(
            "connect htt\u{1b}[31mps://alice:opaque@example.test/path\u{0001}?\u{1b}[32mdebug=private-value#private-fragment now; split https:\u{0001}/\u{009b}31m/alice:second-password@example.test/safe#other-private-fragment; clean https://example.test/safe\n",
        );
        for secret in [
            "alice:opaque",
            "private-value",
            "private-fragment",
            "alice:second-password",
            "other-private-fragment",
        ] {
            assert!(!result.body.contains(secret));
        }
        assert!(
            result
                .body
                .contains("connect https://[REDACTED]@example.test/path?[REDACTED]")
        );
        assert!(
            result
                .body
                .contains("split https://[REDACTED]@example.test/safe#[REDACTED]")
        );
        assert!(result.body.contains("clean https://example.test/safe"));
    }

    #[test]
    fn later_url_does_not_replay_prior_redacted_text() {
        let result = sanitize(
            "TOKEN=synthetic-secret ghp_prior-secret -----BEGIN PRIVATE KEY-----pem-secret-----END PRIVATE KEY----- https://example.test/\n",
        );
        for secret in ["synthetic-secret", "prior-secret", "pem-secret"] {
            assert!(!result.body.contains(secret));
        }
        assert!(!result.body.contains("TOKEN=synthetic-secret"));
        assert!(result.body.contains("https://example.test/"));
    }

    #[test]
    fn redacts_multiline_private_key_blocks() {
        let result = sanitize(
            "before\n-----BEGIN RSA \u{1b}[31mPRIVATE KEY-----\nMIIE_secret_private_key_material\n-----END RSA \u{1b}[0mPRIVATE KEY-----\nafter\n",
        );
        assert!(!result.body.contains("MIIE_secret_private_key_material"));
        assert!(result.body.contains("[REDACTED PRIVATE KEY]"));
        assert!(result.body.contains("before"));
        assert!(result.body.contains("after"));
    }

    #[test]
    fn strips_ansi_and_control_injection_but_keeps_diagnostics() {
        let result =
            sanitize("\u{1b}[31merror\u{1b}[0m: failed\u{0000}\u{0007} to build\u{009b}31m\n");
        assert!(result.body.contains("error: failed to build"));
        assert!(!result.body.contains('\u{1b}'));
        assert!(!result.body.contains('\u{0000}'));
        assert!(!result.body.contains('\u{009b}'));
        assert_eq!(sanitize("\u{1b}é").body, "é");
    }

    #[test]
    fn recognizes_ansi_and_control_split_headers_assignments_and_auth() {
        let result = sanitize(
            "\u{1b}[31mCookie: session=ansi-cookie-secret\u{1b}[0m\n> Cookie: curl-cookie-secret\n\u{009b}31mCookie: c1-cookie-secret\nTO\u{1b}[31mKEN=ansi-assignment-secret\nTO\u{0001}KEN=control-assignment-secret\nBe\u{1b}[31marer ansi-auth-secret\nAuth\u{0001}orization: Bearer control-header-secret\n",
        );
        for secret in [
            "ansi-cookie-secret",
            "curl-cookie-secret",
            "ansi-assignment-secret",
            "c1-cookie-secret",
            "ansi-auth-secret",
            "control-header-secret",
        ] {
            assert!(!result.body.contains(secret));
        }
        assert!(result.body.contains("Cookie: [REDACTED]"));
        assert!(result.body.contains("> Cookie: [REDACTED]"));
        assert!(result.body.contains("TOKEN=[REDACTED]"));
        assert!(result.body.contains("Bearer [REDACTED]"));
    }

    #[test]
    fn detects_tokens_that_cross_the_excerpt_boundary_before_selecting_excerpt() {
        let token = format!("ghp_{}", "sensitive-token-part-".repeat(80));
        let input = format!(
            "{}{}{}",
            "diagnostic ".repeat(700),
            token,
            " tail detail".repeat(700)
        );
        let result = sanitize(&input);
        assert!(result.truncated);
        assert!(result.body.len() <= MAX_STDERR_LOG_BYTES);
        assert!(!result.body.contains("ghp_"));
        assert!(result.body.contains("excerpt omitted"));
        assert!(result.body.starts_with("diagnostic "));
        assert!(result.body.ends_with(" tail detail"));
    }

    #[test]
    fn caps_unicode_output_at_utf8_boundaries_with_head_and_tail_excerpt() {
        let input = format!(
            "{}{}{}",
            "前🧪".repeat(1500),
            " 中間の診断 ".repeat(300),
            "末尾🧪".repeat(1500)
        );
        let result = sanitize(&input);
        assert!(result.truncated);
        assert!(result.body.len() <= MAX_STDERR_LOG_BYTES);
        assert!(result.body.is_char_boundary(result.body.len()));
        assert!(result.body.starts_with("前🧪"));
        assert!(result.body.contains("excerpt omitted"));
        assert!(result.body.ends_with("末尾🧪"));
    }

    #[test]
    fn handles_an_overlong_assignment_line_without_leaking_its_value() {
        let input = format!("prefix TOKEN={} safe-tail", "secret-value".repeat(50_000));
        let result = sanitize(&input);
        assert!(!result.truncated);
        assert!(result.body.len() <= MAX_STDERR_LOG_BYTES);
        assert!(!result.body.contains("secret-value"));
        assert!(result.body.starts_with("prefix TOKEN=[REDACTED]"));
        assert!(result.body.ends_with("safe-tail"));
    }

    #[test]
    fn leaves_unrecognized_plaintext_and_safe_diagnostics_intact() {
        let result = sanitize("compile failed: expected 3, got 4\nuser=alice says hello\n");
        assert!(result.body.contains("compile failed: expected 3, got 4"));
        assert!(result.body.contains("user=alice says hello"));
        assert!(!result.truncated);
        assert!(!result.body.contains(REDACTED));
    }

    #[test]
    fn empty_and_machine_marker_only_input_produces_empty_body() {
        assert_eq!(sanitize("").body, "");
        assert_eq!(
            sanitize(" \t\nCRUISE_CI_DIAGNOSTIC_V4 {\"private\":\"value\"}\n").body,
            ""
        );
        assert_eq!(sanitize(&" ".repeat(MAX_STDERR_LOG_BYTES * 2)).body, "");
    }
    #[test]
    fn omits_machine_diagnostic_markers_with_untrusted_fields() {
        let result = sanitize(
            "safe diagnostic\nCRUISE_CI_DIAGNOSTIC_V4 {\"checks\":[],\"unexpected\":\"leaked-field-secret\"}\nCRUISE_SYNC_BASE_DIAGNOSTIC_V9 {\"secret\":\"also-not-for-logs\"}\n\u{1b}[31mCRUISE_CI_DIAGNOSTIC_V5 {\"unexpected\":\"colored-marker-secret\"}\nretained tail\n",
        );
        assert!(!result.body.contains("CRUISE_CI_DIAGNOSTIC"));
        assert!(!result.body.contains("CRUISE_SYNC_BASE_DIAGNOSTIC"));
        assert!(!result.body.contains("leaked-field-secret"));
        assert!(!result.body.contains("also-not-for-logs"));
        assert!(result.body.contains("safe diagnostic"));
        assert!(result.body.contains("retained tail"));
        assert!(!result.body.contains("colored-marker-secret"));
    }
}
