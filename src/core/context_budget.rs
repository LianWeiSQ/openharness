#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct ContextBudgetOptions {
    pub enabled: bool,
    pub strategy: String,
    pub counting: String,
    pub compaction_mode: String,
    pub reserve_output_tokens: u64,
    pub guard_ratio: f64,
    pub input_safety_margin_tokens: u64,
    pub use_safety_margin_tokens: bool,
    pub explicit_input_safety_margin_tokens: bool,
    pub bytes_per_token: u64,
    pub tool_display_max_bytes: u64,
    pub tool_context_preview_bytes: u64,
    pub tool_context_preview_lines: u64,
    pub tool_context_line_max_chars: u64,
    pub prune_old_tool_outputs: bool,
    pub prune_keep_recent_user_turns: u64,
    pub prune_protect_input_tokens: u64,
    pub prune_min_input_tokens: u64,
    pub compact_summary_max_output_tokens: u64,
    pub compact_refresh_min_new_messages: u64,
    pub overflow_keep_recent_user_turns: u64,
    pub overflow_disable_tools_on_final_attempt: bool,
    pub overflow_final_max_output_tokens: u64,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct ContextBudgetResult {
    pub estimated_input_tokens: u64,
    pub input_limit_tokens: u64,
    pub context_window: u64,
    pub reserved_output_tokens: u64,
    pub overflowed: bool,
    pub tool_message_count: u64,
    pub largest_tool_message_tokens: u64,
    pub largest_tool_message_name: String,
    pub counting_method: String,
    pub counting_exact: bool,
    pub fallback_stage: String,
    pub payload_kind: String,
}

#[must_use]
pub fn format_context_budget_error(result: &ContextBudgetResult) -> String {
    let mut message = format!(
        "Context budget exceeded before model call: estimated_input_tokens={}, input_limit_tokens={}, context_window={}, reserved_output_tokens={}, counting_method={}, counting_exact={}, payload_kind={}, fallback_stage={}",
        result.estimated_input_tokens,
        result.input_limit_tokens,
        result.context_window,
        result.reserved_output_tokens,
        result.counting_method,
        result.counting_exact,
        result.payload_kind,
        result.fallback_stage
    );
    if result.tool_message_count > 0 {
        message.push_str(&format!(
            ", tool_message_count={}, largest_tool_message_tokens={}, largest_tool_message_name={}",
            result.tool_message_count,
            result.largest_tool_message_tokens,
            if result.largest_tool_message_name.is_empty() {
                "unknown"
            } else {
                &result.largest_tool_message_name
            }
        ));
    }
    message
}

pub fn load_context_budget_options(
    options: Option<&Value>,
    model: Option<&Model>,
) -> Result<ContextBudgetOptions, String> {
    let merged = merge_compaction_facade_options(options)?;
    let enabled = expect_bool(
        merged.get("enabled").unwrap_or(&Value::Bool(true)),
        "enabled",
        "context_budget",
    )?;
    let strategy = expect_non_empty_string(
        merged
            .get("strategy")
            .unwrap_or(&Value::String("auto".to_string())),
        "context_budget.strategy",
    )?;
    let counting = expect_non_empty_string(
        merged
            .get("counting")
            .unwrap_or(&Value::String("auto".to_string())),
        "context_budget.counting",
    )?;
    let compaction_mode = expect_non_empty_string(
        merged
            .get("compaction_mode")
            .unwrap_or(&Value::String(DEFAULT_COMPACTION_MODE.to_string())),
        "context_budget.compaction_mode",
    )?;
    if !SUPPORTED_COMPACTION_MODES.contains(&compaction_mode.as_str()) {
        return Err(format!(
            "Unsupported context_budget.compaction_mode: {compaction_mode}. Supported modes: structured_work_state."
        ));
    }

    let model_max_output = model.map(|item| item.max_output).unwrap_or(0);
    let reserve_output_tokens = expect_int(
        merged
            .get("reserve_output_tokens")
            .unwrap_or(&json!(model_max_output)),
        "reserve_output_tokens",
        0,
        "context_budget",
    )?;
    let guard_ratio = expect_float(
        merged
            .get("guard_ratio")
            .unwrap_or(&json!(DEFAULT_GUARD_RATIO)),
        "guard_ratio",
        0.0,
        1.0,
        false,
    )?;
    let explicit_input_safety_margin_tokens = merged.contains_key("input_safety_margin_tokens");
    let use_safety_margin_tokens =
        explicit_input_safety_margin_tokens || !merged.contains_key("guard_ratio");
    let safety_margin_default = if use_safety_margin_tokens {
        DEFAULT_INPUT_SAFETY_MARGIN_TOKENS
    } else {
        0
    };
    let input_safety_margin_tokens = expect_int(
        merged
            .get("input_safety_margin_tokens")
            .unwrap_or(&json!(safety_margin_default)),
        "input_safety_margin_tokens",
        0,
        "context_budget",
    )?;
    let bytes_per_token = expect_int(
        merged
            .get("bytes_per_token")
            .unwrap_or(&json!(DEFAULT_BYTES_PER_TOKEN)),
        "bytes_per_token",
        1,
        "context_budget",
    )?;
    let tool_display_max_bytes = expect_int(
        merged
            .get("tool_display_max_bytes")
            .unwrap_or(&json!(DEFAULT_TOOL_DISPLAY_MAX_BYTES)),
        "tool_display_max_bytes",
        1,
        "context_budget",
    )?;
    let tool_context_preview_bytes = expect_int(
        merged
            .get("tool_context_preview_bytes")
            .unwrap_or(&json!(DEFAULT_TOOL_CONTEXT_PREVIEW_BYTES)),
        "tool_context_preview_bytes",
        1,
        "context_budget",
    )?;
    let tool_context_preview_lines = expect_int(
        merged
            .get("tool_context_preview_lines")
            .unwrap_or(&json!(DEFAULT_TOOL_CONTEXT_PREVIEW_LINES)),
        "tool_context_preview_lines",
        1,
        "context_budget",
    )?;
    let tool_context_line_max_chars = expect_int(
        merged
            .get("tool_context_line_max_chars")
            .unwrap_or(&json!(DEFAULT_TOOL_CONTEXT_LINE_MAX_CHARS)),
        "tool_context_line_max_chars",
        1,
        "context_budget",
    )?;
    let prune_old_tool_outputs = expect_bool(
        merged
            .get("prune_old_tool_outputs")
            .unwrap_or(&Value::Bool(DEFAULT_PRUNE_OLD_TOOL_OUTPUTS)),
        "prune_old_tool_outputs",
        "context_budget",
    )?;
    let prune_keep_recent_user_turns = expect_int(
        merged
            .get("prune_keep_recent_user_turns")
            .unwrap_or(&json!(DEFAULT_PRUNE_KEEP_RECENT_USER_TURNS)),
        "prune_keep_recent_user_turns",
        1,
        "context_budget",
    )?;
    let prune_protect_input_tokens = expect_int(
        merged
            .get("prune_protect_input_tokens")
            .unwrap_or(&json!(DEFAULT_PRUNE_PROTECT_INPUT_TOKENS)),
        "prune_protect_input_tokens",
        0,
        "context_budget",
    )?;
    let prune_min_input_tokens = expect_int(
        merged
            .get("prune_min_input_tokens")
            .unwrap_or(&json!(DEFAULT_PRUNE_MIN_INPUT_TOKENS)),
        "prune_min_input_tokens",
        0,
        "context_budget",
    )?;
    let compact_summary_max_output_tokens = expect_int(
        merged
            .get("compact_summary_max_output_tokens")
            .unwrap_or(&json!(DEFAULT_COMPACT_SUMMARY_MAX_OUTPUT_TOKENS)),
        "compact_summary_max_output_tokens",
        1,
        "context_budget",
    )?;
    let compact_refresh_min_new_messages = expect_int(
        merged
            .get("compact_refresh_min_new_messages")
            .unwrap_or(&json!(DEFAULT_COMPACT_REFRESH_MIN_NEW_MESSAGES)),
        "compact_refresh_min_new_messages",
        1,
        "context_budget",
    )?;
    let overflow_keep_recent_user_turns = expect_int(
        merged
            .get("overflow_keep_recent_user_turns")
            .unwrap_or(&json!(DEFAULT_OVERFLOW_KEEP_RECENT_USER_TURNS)),
        "overflow_keep_recent_user_turns",
        1,
        "context_budget",
    )?;
    let overflow_disable_tools_on_final_attempt = expect_bool(
        merged
            .get("overflow_disable_tools_on_final_attempt")
            .unwrap_or(&Value::Bool(true)),
        "overflow_disable_tools_on_final_attempt",
        "context_budget",
    )?;
    let overflow_final_max_output_default =
        model.map(|item| item.max_output.min(512)).unwrap_or(512);
    let overflow_final_max_output_tokens = expect_int(
        merged
            .get("overflow_final_max_output_tokens")
            .unwrap_or(&json!(overflow_final_max_output_default)),
        "overflow_final_max_output_tokens",
        1,
        "context_budget",
    )?;

    Ok(ContextBudgetOptions {
        enabled,
        strategy,
        counting,
        compaction_mode,
        reserve_output_tokens,
        guard_ratio,
        input_safety_margin_tokens,
        use_safety_margin_tokens,
        explicit_input_safety_margin_tokens,
        bytes_per_token,
        tool_display_max_bytes,
        tool_context_preview_bytes,
        tool_context_preview_lines,
        tool_context_line_max_chars,
        prune_old_tool_outputs,
        prune_keep_recent_user_turns,
        prune_protect_input_tokens,
        prune_min_input_tokens,
        compact_summary_max_output_tokens,
        compact_refresh_min_new_messages,
        overflow_keep_recent_user_turns,
        overflow_disable_tools_on_final_attempt,
        overflow_final_max_output_tokens,
    })
}

pub fn check_context_budget(
    system: Option<&str>,
    messages: &[ChatMessage],
    tools: &[ToolSchema],
    model: Option<&Model>,
    options: Option<&Value>,
    fallback_stage: &str,
) -> Result<Option<ContextBudgetResult>, String> {
    let Some(model) = model else {
        return Ok(None);
    };
    if model.context_window == 0 {
        return Ok(None);
    }
    let config = load_context_budget_options(options, Some(model))?;
    if !config.enabled {
        return Ok(None);
    }
    if !SUPPORTED_STRATEGIES.contains(&config.strategy.as_str()) {
        return Err(format!(
            "Unsupported context budget strategy: {}. Supported strategies: auto, error, compact.",
            config.strategy
        ));
    }
    if !SUPPORTED_COUNTING.contains(&config.counting.as_str()) {
        return Err(format!(
            "Unsupported context budget counting mode: {}. Supported modes: auto, tiktoken, heuristic.",
            config.counting
        ));
    }

    let provider_options = options_to_btree(options);
    let payload = materialize_openai_compatible_payload(
        system,
        messages,
        tools,
        Some(model),
        Some(&provider_options),
    );
    let payload_kind = if is_openai_compatible_model(model) {
        "openai_compatible"
    } else {
        "generic"
    };
    let count = estimate_payload_tokens(&payload, config.bytes_per_token);
    let diagnostics = tool_message_diagnostics(messages, model, options, config.bytes_per_token);
    let input_limit_tokens = compute_input_limit_tokens(model, &config);
    Ok(Some(ContextBudgetResult {
        estimated_input_tokens: count,
        input_limit_tokens,
        context_window: model.context_window,
        reserved_output_tokens: config.reserve_output_tokens,
        overflowed: count > input_limit_tokens,
        tool_message_count: diagnostics.tool_message_count,
        largest_tool_message_tokens: diagnostics.largest_tool_message_tokens,
        largest_tool_message_name: diagnostics.largest_tool_message_name,
        counting_method: "heuristic".to_string(),
        counting_exact: false,
        fallback_stage: fallback_stage.to_string(),
        payload_kind: payload_kind.to_string(),
    }))
}
