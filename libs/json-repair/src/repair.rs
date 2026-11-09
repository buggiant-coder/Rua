use serde_json::Value;

use crate::config::RepairConfig;
use crate::diagnostics::{RepairDiagnostic, RepairStrategyKind};
use crate::error::RepairError;
use crate::outcome::{RepairConfidence, RepairOutcome, RepairStringOutcome};
use crate::redact::redact_for_log;
use crate::scanner::check_depth;
use crate::strategies;

pub fn repair_json(input: &str) -> Result<RepairOutcome, RepairError> {
    repair_json_with_config(input, &RepairConfig::default())
}

pub fn repair_json_with_config(
    input: &str,
    config: &RepairConfig,
) -> Result<RepairOutcome, RepairError> {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return Err(RepairError::EmptyInput);
    }

    if input.len() > config.max_input_bytes {
        return Err(RepairError::InputTooLarge {
            bytes: input.len(),
            limit: config.max_input_bytes,
        });
    }

    if let Err(depth) = check_depth(input, config.max_depth) {
        return Err(RepairError::MaxDepthExceeded {
            depth,
            limit: config.max_depth,
        });
    }

    if let Ok(value) = serde_json::from_str::<Value>(input) {
        let repaired_json = value.to_string();
        return Ok(RepairOutcome {
            value,
            repaired_json,
            changed: false,
            strategies_applied: vec![],
            confidence: RepairConfidence::Exact,
            diagnostics: vec![],
        });
    }

    let mut current = input.to_string();
    let mut strategies_applied: Vec<RepairStrategyKind> = Vec::new();
    let mut diagnostics: Vec<RepairDiagnostic> = Vec::new();
    let mut confidence = RepairConfidence::High;

    if config.strip_fences {
        if let Some((result, diag)) = strategies::strip_fences::strip_fences(&current) {
            current = result;
            strategies_applied.push(RepairStrategyKind::StripFences);
            diagnostics.push(diag);
        }
    }

    if let Ok(value) = serde_json::from_str::<Value>(&current) {
        return finalize(
            value,
            &current,
            true,
            strategies_applied,
            confidence,
            diagnostics,
            config,
        );
    }

    if config.extract_first_json {
        if let Some((result, diag)) = strategies::extract_first_json::extract_first_json(&current) {
            current = result;
            strategies_applied.push(RepairStrategyKind::ExtractFirstJson);
            diagnostics.push(diag);

            if confidence > RepairConfidence::Medium {
                confidence = RepairConfidence::Medium;
            }
        }
    }

    if let Ok(value) = serde_json::from_str::<Value>(&current) {
        return finalize(
            value,
            &current,
            true,
            strategies_applied,
            confidence,
            diagnostics,
            config,
        );
    }

    if config.remove_comments {
        if let Some((result, diag)) = strategies::comments::remove_comments(&current) {
            current = result;
            strategies_applied.push(RepairStrategyKind::RemoveComments);
            diagnostics.push(diag);
        }
    }

    if config.fix_python_literals {
        if let Some((result, diag)) = strategies::python_literals::fix_python_literals(&current) {
            current = result;
            strategies_applied.push(RepairStrategyKind::FixPythonLiterals);
            diagnostics.push(diag);
        }
    }

    if config.fix_quotes {
        if let Some((result, diag)) = strategies::quotes::fix_quotes(&current) {
            current = result;
            strategies_applied.push(RepairStrategyKind::FixQuotes);
            diagnostics.push(diag);
        }
    }

    if config.remove_trailing_commas {
        if let Some((result, diag)) = strategies::trailing_commas::remove_trailing_commas(&current)
        {
            current = result;
            strategies_applied.push(RepairStrategyKind::RemoveTrailingCommas);
            diagnostics.push(diag);
        }
    }

    if config.quote_unquoted_keys {
        if let Some((result, diag)) = strategies::quote_keys::quote_unquoted_keys(&current) {
            current = result;
            strategies_applied.push(RepairStrategyKind::QuoteUnquotedKeys);
            diagnostics.push(diag);
        }
    }

    if config.fix_control_chars {
        if let Some((result, diag)) = strategies::controls::fix_control_chars(&current) {
            current = result;
            strategies_applied.push(RepairStrategyKind::FixControlChars);
            diagnostics.push(diag);
        }
    }

    if let Ok(value) = serde_json::from_str::<Value>(&current) {
        return finalize(
            value,
            &current,
            true,
            strategies_applied,
            confidence,
            diagnostics,
            config,
        );
    }

    if config.close_brackets {
        if let Some((result, diag)) = strategies::close_brackets::close_brackets(&current) {
            current = result;
            strategies_applied.push(RepairStrategyKind::CloseBrackets);
            diagnostics.push(diag);
        }
    }

    if let Ok(value) = serde_json::from_str::<Value>(&current) {
        return finalize(
            value,
            &current,
            true,
            strategies_applied,
            confidence,
            diagnostics,
            config,
        );
    }

    if config.handle_truncated {
        if let Some((result, diag)) = strategies::truncation::safe_truncate(&current) {
            current = result;
            strategies_applied.push(RepairStrategyKind::SafeTruncation);
            diagnostics.push(diag);

            if confidence > RepairConfidence::Medium {
                confidence = RepairConfidence::Medium;
            }
        }
    }

    match serde_json::from_str::<Value>(&current) {
        Ok(value) => finalize(
            value,
            &current,
            true,
            strategies_applied,
            confidence,
            diagnostics,
            config,
        ),
        Err(_) => Err(RepairError::Unrecoverable {
            reason: format!(
                "could not repair JSON after applying {} strategies: {}",
                strategies_applied.len(),
                redact_for_log(input, 200)
            ),
            diagnostics,
        }),
    }
}

pub fn repair_json_string_with_config(
    input: &str,
    config: &RepairConfig,
) -> Result<RepairStringOutcome, RepairError> {
    let wrapped = format!("\"{}\"", input.replace('\\', "\\\\").replace('"', "\\\""));

    match repair_json_with_config(&wrapped, config) {
        Ok(outcome) => {
            let string_value = outcome.value.as_str().unwrap_or(input).to_string();
            Ok(RepairStringOutcome {
                value: string_value,
                repaired_json: outcome.repaired_json,
                changed: outcome.changed,
                strategies_applied: outcome.strategies_applied,
                confidence: outcome.confidence,
                diagnostics: outcome.diagnostics,
            })
        }
        Err(e) => Err(e),
    }
}

fn finalize(
    value: Value,
    _current: &str,
    changed: bool,
    strategies_applied: Vec<RepairStrategyKind>,
    mut confidence: RepairConfidence,
    diagnostics: Vec<RepairDiagnostic>,
    config: &RepairConfig,
) -> Result<RepairOutcome, RepairError> {
    let repaired_json = value.to_string();

    if repaired_json.len() > config.max_output_bytes {
        return Err(RepairError::OutputTooLarge {
            bytes: repaired_json.len(),
            limit: config.max_output_bytes,
        });
    }

    let depth = measure_value_depth(&value);
    if depth > config.max_depth {
        return Err(RepairError::MaxDepthExceeded {
            depth,
            limit: config.max_depth,
        });
    }

    if !strategies_applied.is_empty() && confidence == RepairConfidence::Exact {
        confidence = RepairConfidence::High;
    }

    if !config.allow_low_confidence && confidence == RepairConfidence::Low {
        return Err(RepairError::Unrecoverable {
            reason: "repair confidence too low".to_string(),
            diagnostics,
        });
    }

    Ok(RepairOutcome {
        value,
        repaired_json,
        changed,
        strategies_applied,
        confidence,
        diagnostics,
    })
}

pub fn measure_value_depth(value: &Value) -> usize {
    match value {
        Value::Array(arr) => 1 + arr.iter().map(measure_value_depth).max().unwrap_or(0),
        Value::Object(obj) => 1 + obj.values().map(measure_value_depth).max().unwrap_or(0),
        _ => 0,
    }
}
