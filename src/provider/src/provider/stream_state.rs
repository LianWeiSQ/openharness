#[derive(Clone, Debug, Default)]
struct OpenAiToolCallState {
    id: Option<String>,
    name: Option<String>,
    arguments: String,
    arguments_emitted: String,
}

#[derive(Clone, Debug)]
struct ParsedToolCall {
    call_id: String,
    name: String,
    input: Value,
}

#[derive(Clone, Debug)]
struct ToolUseState {
    call_id: String,
    name: String,
    input_value: Value,
    partial_json: String,
    emitted: bool,
}

#[derive(Clone, Copy, Debug)]
struct ProviderMetadata {
    label: Option<&'static str>,
    default_base_url: Option<&'static str>,
    default_model: Option<&'static str>,
    requires_api_key: bool,
    auth_notes: Option<&'static str>,
}

trait TitleCase {
    fn to_title_case(&self) -> String;
}

impl TitleCase for str {
    fn to_title_case(&self) -> String {
        self.split('-')
            .map(|part| {
                let mut chars = part.chars();
                let Some(first) = chars.next() else {
                    return String::new();
                };
                format!(
                    "{}{}",
                    first.to_ascii_uppercase(),
                    chars.collect::<String>()
                )
            })
            .collect::<Vec<_>>()
            .join("-")
    }
}
