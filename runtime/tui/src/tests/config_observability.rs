#[test]
fn tui_config_loads_jsonc_and_theme_command_updates_state() {
    let root = std::env::temp_dir().join(format!("openagent-tui-config-{}", std::process::id()));
    let config_dir = root.join(".openagent");
    fs::create_dir_all(&config_dir).expect("create config dir");
    fs::write(
        config_dir.join("tui.jsonc"),
        r#"{
                // user theme
                "theme": "midnight",
                "color_scheme": "dark",
                "keybinds": {"stash": "ctrl+g"},
                "leader_key": ",",
                "mouse": false,
                "scroll": 9,
                "diff_style": "split",
                "attention_notifications": false,
                "sounds": true
            }"#,
    )
    .expect("write config");

    let config = TuiConfig::load_from_workspace(&root);
    assert_eq!(config.theme, "midnight");
    assert_eq!(config.color_scheme, "dark");
    assert_eq!(config.keybinds["stash"], "ctrl+g");
    assert_eq!(config.leader_key, ",");
    assert!(!config.mouse);
    assert_eq!(config.scroll, 9);
    assert_eq!(config.diff_style, "split");
    assert!(!config.attention_notifications);
    assert!(config.sounds);

    let mut state = TuiState::with_config(config);
    state.input_buffer = "/themes high-contrast".to_string();
    assert!(!state.submit());
    assert_eq!(state.config.theme, "high-contrast");

    state.input_buffer = "/theme-scheme light".to_string();
    assert!(!state.submit());
    assert_eq!(state.config.color_scheme, "light");

    state.input_buffer = "/theme-scheme cycle".to_string();
    assert!(!state.submit());
    assert_eq!(state.config.color_scheme, "dark");

    state.input_buffer = "/config".to_string();
    assert!(!state.submit());
    assert!(
        state
            .timeline
            .iter()
            .any(|line| line.text.contains("diff_style"))
    );

    let _ = fs::remove_dir_all(root);
}

#[test]
fn observability_usage_warnings_and_tool_details_render() {
    let mut state = TuiState::new();
    state.apply_app_event(&json!({
        "method": "turn/completed",
        "params": {
            "status": "completed",
            "usage": {"input_tokens": 10, "output_tokens": 5, "total_tokens": 15, "cost": 0.01},
            "trace": {"run_id": "turn_1", "model": "server-local"}
        }
    }));
    state.apply_app_event(&json!({
        "method": "turn/completed",
        "params": {
            "status": "completed",
            "usage": {"input_tokens": 2, "output_tokens": 3, "total_tokens": 5, "cost": 0.02}
        }
    }));
    assert_eq!(state.usage_totals["input_tokens"], json!(12));
    assert_eq!(state.usage_totals["output_tokens"], json!(8));
    assert_eq!(state.usage_totals["total_tokens"], json!(20));

    state.apply_app_event(&json!({
        "method": "runtime/warning",
        "params": {"message": "provider throttled"}
    }));
    assert_eq!(
        state.runtime_warnings,
        vec!["provider throttled".to_string()]
    );

    state.input_buffer = "/tool-details on".to_string();
    assert!(!state.submit());
    assert!(state.show_tool_details);
    state.apply_app_event(&json!({
        "method": "item/toolCall/completed",
        "params": {
            "name": "bash",
            "output": "ok",
            "metadata": {"returncode": 0}
        }
    }));
    assert!(
        state
            .timeline
            .iter()
            .any(|line| line.text.contains("tool details"))
    );

    state.input_buffer = "/warnings".to_string();
    assert!(!state.submit());
    assert!(
        state
            .timeline
            .iter()
            .any(|line| line.text.contains("provider throttled"))
    );
}
