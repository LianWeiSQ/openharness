#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn exposes_crate_name() {
        assert_eq!(crate_name(), "openagent-protocol");
    }

    #[test]
    fn tool_call_key_matches_stable_json_format() {
        let call = ToolCall {
            name: "read".to_string(),
            input: json!({"path": "README.md"}),
            call_id: "call_fixture_read".to_string(),
        };
        assert_eq!(call.key(), "read:{\"path\": \"README.md\"}");
    }
}
