use serde_json::Value;
use std::collections::BTreeSet;
use winnow::ascii::space0;
use winnow::combinator::{alt, delimited, separated};
use winnow::prelude::*;
use winnow::token::{one_of, take_till, take_while};

const MAX_DISPLAY_VALUE_BYTES: usize = 240;
const MAX_SEARCH_TEXT_BYTES: usize = 2 * 1024;
const MAX_VALUES_PER_ROW: usize = 512;
const MAX_CANDIDATE_VALUE_CHARS: usize = 120;

#[derive(Debug, Clone, Copy, Eq, PartialEq, Ord, PartialOrd)]
pub(crate) enum CandidateKind {
    KeyValueBag,
    Scalar,
}

impl CandidateKind {
    pub(crate) fn rank(self) -> i64 {
        match self {
            Self::KeyValueBag => 0,
            Self::Scalar => 2,
        }
    }
}

#[derive(Debug, Clone, Eq, PartialEq, Ord, PartialOrd)]
pub(crate) struct CandidateValue {
    pub(crate) column_path: String,
    pub(crate) value: String,
    pub(crate) value_truncated: bool,
    pub(crate) search_text: String,
    pub(crate) value_hash: String,
    pub(crate) kind: CandidateKind,
}

pub(crate) fn collect_row_values(row: &Value) -> BTreeSet<CandidateValue> {
    let mut values = BTreeSet::new();
    collect_value_at_path("", row, &mut values);
    values
}

fn collect_value_at_path(path: &str, value: &Value, values: &mut BTreeSet<CandidateValue>) {
    if values.len() >= MAX_VALUES_PER_ROW {
        return;
    }
    match value {
        Value::Object(object) => {
            for (key, value) in object {
                let child_path = path_join(path, key);
                collect_value_at_path(&child_path, value, values);
                if values.len() >= MAX_VALUES_PER_ROW {
                    break;
                }
            }
        }
        Value::Array(items) => {
            for item in items {
                collect_value_at_path(path, item, values);
                if values.len() >= MAX_VALUES_PER_ROW {
                    break;
                }
            }
        }
        Value::String(text) => {
            collect_string_value(path, text, values);
        }
        Value::Number(number) => {
            push_candidate(path, &number.to_string(), CandidateKind::Scalar, values);
        }
        Value::Bool(value) => {
            push_candidate(
                path,
                if *value { "true" } else { "false" },
                CandidateKind::Scalar,
                values,
            );
        }
        Value::Null => {}
    }
}

fn collect_string_value(path: &str, text: &str, values: &mut BTreeSet<CandidateValue>) {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return;
    }
    if looks_like_json_container(trimmed)
        && let Ok(parsed) = serde_json::from_str::<Value>(trimmed)
    {
        collect_value_at_path(path, &parsed, values);
        return;
    }
    if collect_key_value_bag_values(path, trimmed, values) {
        return;
    }
    push_candidate(path, trimmed, CandidateKind::Scalar, values);
}

fn collect_key_value_bag_values(
    path: &str,
    text: &str,
    values: &mut BTreeSet<CandidateValue>,
) -> bool {
    let pairs = key_value_pairs(text);
    if pairs.len() < 2 && !looks_like_single_key_value_bag(text, &pairs) {
        return false;
    }

    let mut extracted_any = false;
    for pair in pairs {
        let child_path = path_join(path, &pair.key);
        extracted_any |=
            push_candidate(&child_path, &pair.value, CandidateKind::KeyValueBag, values);
        if values.len() >= MAX_VALUES_PER_ROW {
            break;
        }
    }
    extracted_any
}

fn key_value_pairs(text: &str) -> Vec<KeyValuePair> {
    let trimmed = text.trim();
    let parsed: Vec<KeyValuePair> = separated(1.., key_value_pair, pair_separator)
        .parse(trimmed)
        .ok()
        .unwrap_or_default();
    parsed
        .into_iter()
        .filter(|pair: &KeyValuePair| is_filter_value_candidate(&pair.value))
        .collect()
}

#[derive(Debug, Clone, Eq, PartialEq)]
struct KeyValuePair {
    key: String,
    value: String,
}

fn key_value_pair(input: &mut &str) -> ModalResult<KeyValuePair> {
    let _ = space0.parse_next(input)?;
    let key = key_value_key.parse_next(input)?;
    let _ = space0.parse_next(input)?;
    let _ = one_of([':', '=']).parse_next(input)?;
    let _ = space0.parse_next(input)?;
    let value = alt((quoted_value, unquoted_value)).parse_next(input)?;

    Ok(KeyValuePair {
        key: key.to_string(),
        value,
    })
}

fn key_value_key<'input>(input: &mut &'input str) -> ModalResult<&'input str> {
    take_while(1..=64, is_key_char)
        .verify(|key: &str| key.chars().any(char::is_alphabetic))
        .parse_next(input)
}

fn quoted_value(input: &mut &str) -> ModalResult<String> {
    alt((
        delimited('"', take_till(0.., '"'), '"'),
        delimited('\'', take_till(0.., '\''), '\''),
    ))
    .map(str::to_string)
    .parse_next(input)
}

fn unquoted_value(input: &mut &str) -> ModalResult<String> {
    take_till(1.., is_unquoted_value_stop)
        .map(str::to_string)
        .parse_next(input)
}

fn pair_separator(input: &mut &str) -> ModalResult<()> {
    alt((
        (space0, one_of([',', ';', '\n']), space0).void(),
        take_while(1.., char::is_whitespace).void(),
    ))
    .parse_next(input)
}

fn looks_like_single_key_value_bag(text: &str, pairs: &[KeyValuePair]) -> bool {
    pairs.len() == 1 && !text.chars().any(char::is_whitespace) && !looks_like_url(text)
}

fn push_candidate(
    path: &str,
    raw_value: &str,
    kind: CandidateKind,
    values: &mut BTreeSet<CandidateValue>,
) -> bool {
    if path.is_empty()
        || values.len() >= MAX_VALUES_PER_ROW
        || !is_filter_value_candidate(raw_value)
    {
        return false;
    }
    let (value, value_truncated) = truncate_for_display(raw_value);
    values.insert(CandidateValue {
        column_path: path.to_string(),
        value,
        value_truncated,
        search_text: search_text_for_value(raw_value),
        value_hash: stable_hash_hex(raw_value),
        kind,
    })
}

fn path_join(parent: &str, child: &str) -> String {
    if parent.is_empty() {
        child.to_string()
    } else {
        format!("{parent}.{child}")
    }
}

fn looks_like_json_container(value: &str) -> bool {
    (value.starts_with('{') && value.ends_with('}'))
        || (value.starts_with('[') && value.ends_with(']'))
}

fn is_filter_value_candidate(value: &str) -> bool {
    let trimmed = value.trim();
    if trimmed.len() <= 1
        || trimmed.chars().count() > MAX_CANDIDATE_VALUE_CHARS
        || !trimmed.chars().any(char::is_alphanumeric)
        || trimmed.chars().any(|char| matches!(char, '\n' | '\r'))
        || looks_like_url(trimmed)
        || looks_like_markup(trimmed)
    {
        return false;
    }
    true
}

fn is_key_char(char: char) -> bool {
    char.is_ascii_alphanumeric() || matches!(char, '_' | '-' | '.')
}

fn is_unquoted_value_stop(char: char) -> bool {
    matches!(char, ',' | ';' | '\n' | '\t') || char.is_whitespace()
}

fn looks_like_url(value: &str) -> bool {
    value.contains("://") || value.starts_with("www.")
}

fn looks_like_markup(value: &str) -> bool {
    value.contains("```")
        || value.contains("<http")
        || value.contains("[http")
        || (value.contains('<') && value.contains('>'))
        || (value.contains("](") && value.contains('['))
}

fn truncate_for_display(value: &str) -> (String, bool) {
    truncate_to_char_boundary(value, MAX_DISPLAY_VALUE_BYTES)
}

fn search_text_for_value(value: &str) -> String {
    let lowered = value.to_lowercase();
    let tokens = value
        .split(|char: char| !char.is_alphanumeric())
        .filter(|token| !token.is_empty())
        .map(str::to_lowercase)
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect::<Vec<_>>()
        .join(" ");
    let combined = if tokens.is_empty() {
        lowered
    } else {
        format!("{lowered} {tokens}")
    };
    truncate_to_char_boundary(&combined, MAX_SEARCH_TEXT_BYTES).0
}

fn truncate_to_char_boundary(value: &str, max_bytes: usize) -> (String, bool) {
    if value.len() <= max_bytes {
        return (value.to_string(), false);
    }
    let marker = " ...";
    let mut cut = max_bytes.saturating_sub(marker.len());
    while cut > 0 && !value.is_char_boundary(cut) {
        cut -= 1;
    }
    let prefix = value
        .get(..cut)
        .expect("cut was adjusted to a UTF-8 character boundary");
    (format!("{prefix}{marker}"), true)
}

fn stable_hash_hex(value: &str) -> String {
    let mut hash = 0xcbf29ce484222325_u64;
    for byte in value.as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    format!("{hash:016x}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn long_prose_shape_is_not_a_candidate_regardless_of_field_name() {
        let values = collect_string_pairs(
            "service",
            "This is a long description about a benchmark result, not a filter value. It keeps going for long enough that it should be treated as prose rather than a candidate value.",
        );

        assert!(values.is_empty());
    }

    #[test]
    fn short_sentence_like_values_are_candidates_regardless_of_field_name() {
        let values = collect_string_pairs(
            "description",
            "No-Coral baseline benchmark - same vakra questions, raw data access",
        );

        assert_eq!(
            values,
            vec![(
                "description".to_string(),
                "No-Coral baseline benchmark - same vakra questions, raw data access".to_string(),
                CandidateKind::Scalar,
            )]
        );
    }

    #[test]
    fn short_scalar_values_are_candidates_regardless_of_field_name() {
        let values = collect_string_pairs("description", "prod");

        assert_eq!(
            values,
            vec![(
                "description".to_string(),
                "prod".to_string(),
                CandidateKind::Scalar,
            )]
        );
    }

    #[test]
    fn colon_key_value_bags_are_parsed_without_path_name_hints() {
        let values = collect_string_pairs(
            "metadata",
            "env:prod,kube_deployment:titaness-worker,service:titaness-worker",
        );

        assert_eq!(
            values,
            vec![
                (
                    "metadata.env".to_string(),
                    "prod".to_string(),
                    CandidateKind::KeyValueBag,
                ),
                (
                    "metadata.kube_deployment".to_string(),
                    "titaness-worker".to_string(),
                    CandidateKind::KeyValueBag,
                ),
                (
                    "metadata.service".to_string(),
                    "titaness-worker".to_string(),
                    CandidateKind::KeyValueBag,
                ),
            ]
        );
    }

    #[test]
    fn equals_key_value_bags_are_parsed() {
        let values =
            collect_string_pairs("metadata", "env=prod service=titaness-worker status=error");

        assert_eq!(
            values,
            vec![
                (
                    "metadata.env".to_string(),
                    "prod".to_string(),
                    CandidateKind::KeyValueBag,
                ),
                (
                    "metadata.service".to_string(),
                    "titaness-worker".to_string(),
                    CandidateKind::KeyValueBag,
                ),
                (
                    "metadata.status".to_string(),
                    "error".to_string(),
                    CandidateKind::KeyValueBag,
                ),
            ]
        );
    }

    #[test]
    fn single_equals_key_value_bag_is_parsed() {
        let values = collect_string_pairs("metadata", "service=titaness-worker");

        assert_eq!(
            values,
            vec![(
                "metadata.service".to_string(),
                "titaness-worker".to_string(),
                CandidateKind::KeyValueBag,
            )]
        );
    }

    #[test]
    fn quoted_key_value_values_can_contain_spaces_and_delimiters() {
        let values = collect_string_pairs(
            "metadata",
            r#"service="titaness worker",error='timed out, retrying'"#,
        );

        assert_eq!(
            values,
            vec![
                (
                    "metadata.error".to_string(),
                    "timed out, retrying".to_string(),
                    CandidateKind::KeyValueBag,
                ),
                (
                    "metadata.service".to_string(),
                    "titaness worker".to_string(),
                    CandidateKind::KeyValueBag,
                ),
            ]
        );
    }

    #[test]
    fn key_value_bag_parsing_suppresses_the_raw_blob() {
        let values = collect_string_pairs("metadata", "env:prod,service:titaness-worker");

        assert!(!values.iter().any(|(path, _, _)| path == "metadata"));
    }

    #[test]
    fn prose_with_colon_is_not_parsed_as_a_key_value_bag() {
        let values = collect_string_pairs("message", "This is prose: with a colon");

        assert_eq!(
            values,
            vec![(
                "message".to_string(),
                "This is prose: with a colon".to_string(),
                CandidateKind::Scalar,
            )]
        );
    }

    #[test]
    fn json_strings_are_flattened_before_candidate_extraction() {
        let values = collect_string_pairs(
            "payload",
            r#"{"project":{"name":"Coral Feedback"},"team_key":"BENCH"}"#,
        );

        assert_eq!(
            values,
            vec![
                (
                    "payload.project.name".to_string(),
                    "Coral Feedback".to_string(),
                    CandidateKind::Scalar,
                ),
                (
                    "payload.team_key".to_string(),
                    "BENCH".to_string(),
                    CandidateKind::Scalar,
                ),
            ]
        );
    }

    fn collect_string_pairs(path: &str, text: &str) -> Vec<(String, String, CandidateKind)> {
        let mut values = BTreeSet::new();
        collect_string_value(path, text, &mut values);
        candidate_pairs(&values)
    }

    fn candidate_pairs(values: &BTreeSet<CandidateValue>) -> Vec<(String, String, CandidateKind)> {
        values
            .iter()
            .map(|value| (value.column_path.clone(), value.value.clone(), value.kind))
            .collect()
    }
}
