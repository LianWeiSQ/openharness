use serde_json::Value;

use crate::{
    TimelineLine,
    util::{compact_json, trim_lines},
};

pub(crate) fn diff_detail_lines(payload: &Value) -> Vec<TimelineLine> {
    let undo_count = payload
        .get("undo_count")
        .and_then(Value::as_u64)
        .unwrap_or_default();
    let redo_count = payload
        .get("redo_count")
        .and_then(Value::as_u64)
        .unwrap_or_default();
    let latest = payload.get("latest").cloned().unwrap_or(Value::Null);
    let mut lines = vec![TimelineLine::new(
        "patch",
        format!(
            "diff stack: undo={undo_count} redo={redo_count}{}{}",
            if undo_count > 0 { "  /undo" } else { "" },
            if redo_count > 0 { "  /redo" } else { "" }
        ),
        false,
    )];
    if latest.is_object() {
        lines.extend(patch_lines("latest patch", &latest, false));
    } else {
        lines.push(TimelineLine::new("status", "latest patch: none", false));
    }
    lines
}

pub(crate) fn patch_result_lines(action: &str, payload: &Value) -> Vec<TimelineLine> {
    let patch = payload.get("patch").cloned().unwrap_or(Value::Null);
    if patch.is_object() {
        patch_lines(action, &patch, true)
    } else {
        vec![TimelineLine::new(
            "warning",
            format!("{action}: {}", compact_json(payload)),
            true,
        )]
    }
}

pub(crate) fn patch_lines(label: &str, patch: &Value, highlight: bool) -> Vec<TimelineLine> {
    let path = patch
        .get("path")
        .and_then(Value::as_str)
        .unwrap_or("<unknown>");
    let id = patch.get("id").and_then(Value::as_str).unwrap_or("-");
    let status = patch
        .get("status")
        .and_then(Value::as_str)
        .unwrap_or("applied");
    let mut lines = vec![TimelineLine::new(
        "patch",
        format!("{label}: {path} ({status}, {id})  actions: /undo /redo"),
        highlight,
    )];
    if let Some(diff) = patch.get("diff").and_then(Value::as_str)
        && !diff.trim().is_empty()
    {
        lines.push(TimelineLine::new("diff-meta", "diff:", false));
        lines.extend(trim_lines(diff, 80).into_iter().map(|line| {
            let kind = rendered_diff_line_kind(&line);
            TimelineLine::new(kind, line, false)
        }));
    }
    lines
}

fn rendered_diff_line_kind(line: &str) -> &'static str {
    if line.starts_with("@@") {
        "diff-hunk"
    } else if line.starts_with("+++") || line.starts_with("---") {
        "diff-meta"
    } else if line.starts_with('+') {
        "diff-add"
    } else if line.starts_with('-') {
        "diff-del"
    } else {
        "diff"
    }
}
