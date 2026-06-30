pub(super) fn parse_sse_json_values(raw: &str) -> Result<Vec<Value>, String> {
    let mut values = Vec::new();
    let mut data_lines = Vec::new();
    for line in raw.lines() {
        let line = line.trim_end_matches('\r');
        if line.is_empty() {
            flush_sse_json_value(&mut data_lines, &mut values)?;
            continue;
        }
        if line.starts_with(':') {
            continue;
        }
        if let Some(data) = line.strip_prefix("data:") {
            data_lines.push(data.trim_start().to_string());
        }
    }
    flush_sse_json_value(&mut data_lines, &mut values)?;
    Ok(values)
}

fn read_sse_json_values_stream<R, F>(mut reader: R, mut on_value: F) -> Result<(), String>
where
    R: Read,
    F: FnMut(Value) -> Result<(), String>,
{
    let mut raw = String::new();
    let mut buffer = [0_u8; 4096];
    let mut saw_done = false;
    loop {
        let read = match reader.read(&mut buffer) {
            Ok(read) => read,
            Err(_error) if saw_done => break,
            Err(error) => return Err(format!("provider SSE read failed: {error}")),
        };
        if read == 0 {
            break;
        }
        raw.push_str(&String::from_utf8_lossy(&buffer[..read]));
        while let Some(index) = sse_frame_end(&raw) {
            let frame = raw[..index].to_string();
            let drain_to = if raw[index..].starts_with("\r\n\r\n") {
                index + 4
            } else {
                index + 2
            };
            raw.drain(..drain_to);
            if sse_frame_is_done(&frame) {
                saw_done = true;
            }
            if let Some(value) = parse_sse_frame_json(&frame)? {
                on_value(value)?;
            }
        }
    }
    if !raw.trim().is_empty()
        && let Some(value) = parse_sse_frame_json(&raw)?
    {
        on_value(value)?;
    }
    Ok(())
}

fn sse_frame_is_done(frame: &str) -> bool {
    frame.lines().any(|line| {
        let line = line.trim_end_matches('\r');
        line.strip_prefix("data:")
            .map(str::trim)
            .is_some_and(|data| data == "[DONE]")
    })
}

fn sse_frame_end(raw: &str) -> Option<usize> {
    match (raw.find("\r\n\r\n"), raw.find("\n\n")) {
        (Some(left), Some(right)) => Some(left.min(right)),
        (Some(index), None) | (None, Some(index)) => Some(index),
        (None, None) => None,
    }
}

fn parse_sse_frame_json(frame: &str) -> Result<Option<Value>, String> {
    let mut data_lines = Vec::new();
    for line in frame.lines() {
        let line = line.trim_end_matches('\r');
        if line.starts_with(':') {
            continue;
        }
        if let Some(data) = line.strip_prefix("data:") {
            data_lines.push(data.trim_start().to_string());
        }
    }
    if data_lines.is_empty() {
        return Ok(None);
    }
    let data = data_lines.join("\n");
    let trimmed = data.trim();
    if trimmed.is_empty() || trimmed == "[DONE]" {
        return Ok(None);
    }
    serde_json::from_str(trimmed)
        .map(Some)
        .map_err(|error| format!("provider SSE data was not JSON: {error}"))
}

fn flush_sse_json_value(
    data_lines: &mut Vec<String>,
    values: &mut Vec<Value>,
) -> Result<(), String> {
    if data_lines.is_empty() {
        return Ok(());
    }
    let data = data_lines.join("\n");
    data_lines.clear();
    let trimmed = data.trim();
    if trimmed.is_empty() || trimmed == "[DONE]" {
        return Ok(());
    }
    let value: Value = serde_json::from_str(trimmed)
        .map_err(|error| format!("provider SSE data was not JSON: {error}"))?;
    values.push(value);
    Ok(())
}
