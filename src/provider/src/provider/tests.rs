#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn links_to_protocol_crate() {
        assert_eq!(crate_name(), "openagent-provider");
        assert_eq!(protocol_crate_name(), "openagent-protocol");
    }

    #[test]
    fn provider_manager_reports_unknown_provider() {
        let mut manager = ProviderManager::new();
        manager.register_provider("fixture", "Fixture Provider");

        assert_eq!(
            manager
                .get_provider("fixture")
                .map(|item| item.name.as_str()),
            Ok("Fixture Provider")
        );
        assert_eq!(
            manager.get_provider("missing"),
            Err("Unknown provider: missing".to_string())
        );
    }

    #[test]
    fn provider_metadata_normalizes_like_legacy() {
        assert_eq!(
            default_env_mapping("custom.gateway").expect("env mapping"),
            BTreeMap::from([
                ("api_key".to_string(), "CUSTOM_GATEWAY_API_KEY".to_string()),
                (
                    "base_url".to_string(),
                    "CUSTOM_GATEWAY_BASE_URL".to_string()
                ),
                ("model".to_string(), "CUSTOM_GATEWAY_MODEL".to_string()),
                (
                    "wire_api".to_string(),
                    "CUSTOM_GATEWAY_WIRE_API".to_string()
                ),
            ])
        );
        assert!(normalize_provider(Some("bad provider")).is_err());
        assert_eq!(
            provider_label("custom.gateway").expect("label"),
            "Custom-Gateway"
        );
        assert!(!provider_requires_api_key("ollama").expect("requires key"));
    }

    #[test]
    fn openai_argument_parser_recovers_cumulative_snapshot() {
        let parsed = parse_tool_arguments(&Value::String(
            "{\"query\":\"climate tipping points\",\"num_results\":8,\"timeout\":60\
             {\"query\":\"climate tipping points\",\"num_results\":8,\"timeout\":60}"
                .to_string(),
        ));

        assert_eq!(
            parsed,
            json!({"query": "climate tipping points", "num_results": 8, "timeout": 60})
        );
    }
}
