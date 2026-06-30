#[must_use]
pub fn normalize_server_url(value: &str) -> String {
    value.trim_end_matches('/').to_string()
}

#[must_use]
pub fn join_server_url(server_url: &str, path: &str) -> String {
    format!(
        "{}/{}",
        normalize_server_url(server_url),
        path.trim_start_matches('/')
    )
}

#[must_use]
pub fn quote_path(value: &str) -> String {
    let mut output = String::new();
    for byte in value.bytes() {
        let ch = byte as char;
        if ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_' | '.' | '~') {
            output.push(ch);
        } else {
            output.push('%');
            output.push(hex_digit(byte >> 4));
            output.push(hex_digit(byte & 0x0f));
        }
    }
    output
}

#[must_use]
pub fn auth_header(token: Option<&str>) -> Option<String> {
    token
        .filter(|value| !value.is_empty())
        .map(|value| format!("Bearer {value}"))
}

fn hex_digit(value: u8) -> char {
    match value {
        0..=9 => (b'0' + value) as char,
        10..=15 => (b'A' + (value - 10)) as char,
        _ => '0',
    }
}
