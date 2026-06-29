#[must_use]
pub fn eval_integrations_fixture() -> Value {
    let results = fixture_results();
    let aggregate = aggregate_results(&results);
    let summary = render_summary(&results);
    let baseline_report = baseline_report_fixture();
    let baseline_path = fixture_path("baseline.json");
    let current_report_path = fixture_path("report.json");
    let regression = compare_with_baseline(
        &baseline_path,
        &baseline_report,
        &results,
        &current_report_path,
        None,
    );
    let regression_summary = render_regression_summary(&regression);
    let report_payload = json!({
        "schema_version": "openagent.eval.report.v1",
        "aggregate": aggregate,
        "results": results,
        "regression": regression,
    });
    let clean_results = vec![fixture_pass_result()];
    let clean_report = json!({
        "schema_version": "openagent.eval.report.v1",
        "aggregate": aggregate_results(&clean_results),
        "results": clean_results,
    });
    let regression_path = fixture_path("regression.json");
    let ci_gate = json!({
        "pass": check_eval_ci_gate(
            &fixture_path("clean-report.json"),
            &clean_report,
            None,
            EvalCiGateOptions {
                max_runtime_warnings: Some(0),
                ..EvalCiGateOptions::default()
            },
        ),
        "fail": check_eval_ci_gate(
            &current_report_path,
            &report_payload,
            Some((&regression_path, &regression)),
            EvalCiGateOptions {
                min_success_rate: 0.75,
                max_runtime_warnings: Some(1),
                ..EvalCiGateOptions::default()
            },
        ),
    });

    let mut langfuse_success = fixture_fail_result();
    langfuse_success.langfuse_trace_id = Some("trace_fixture_123".to_string());
    langfuse_success.langfuse_scores_sent = true;
    let mut langfuse_failure = fixture_pass_result();
    langfuse_failure.langfuse_trace_id = Some("trace_fixture_123".to_string());
    langfuse_failure.langfuse_scores_sent = false;
    langfuse_failure.langfuse_error = Some("fixture score export failed".to_string());

    let (harbor_success_command, harbor_success_result) =
        harbor_success_command(HarborSuccessSpec {
            command: "echo hello",
            cwd: Some("/app/project"),
            timeout_ms: 5200,
            workspace_root: "/app",
            returncode: 7,
            stdout: "hello from harbor\n",
            stderr: "warn",
            elapsed_ms: 320,
        });
    let (harbor_timeout_command, harbor_timeout_result) =
        harbor_timeout_command("sleep 10", None, 1, "/app", 1234, "fixture timeout");
    let (terminal_code, terminal_cleaned) = terminal_bench_extract_returncode(
        "$ command\nhello\n__OPENAGENT_TBENCH_EXIT_fixture__7\ntrailing",
        "__OPENAGENT_TBENCH_EXIT_fixture__",
    );

    json!({
        "schema_version": 1,
        "eval": {
            "results": fixture_results(),
            "aggregate": aggregate,
            "summary": summary,
            "regression": regression,
            "regression_summary": regression_summary,
            "ci_gate": ci_gate,
            "langfuse": {
                "success_result": langfuse_success,
                "success_scores": langfuse_score_payloads(
                    &fixture_fail_result(),
                    "langfuse_case",
                    "run_fixture",
                    "trace_fixture_123",
                ),
                "success_flush_count": 1,
                "failure_result": langfuse_failure,
            },
        },
        "terminal_bench": {
            "defaults": {
                "max_steps": DEFAULT_MAX_STEPS,
                "context_window": DEFAULT_CONTEXT_WINDOW,
                "max_output": DEFAULT_MAX_OUTPUT,
                "workdir": DEFAULT_WORKDIR,
            },
            "metadata": execution_metadata("terminal_bench", "/app", "terminal_bench"),
            "display_paths": {
                "root": display_path("/app", "/app"),
                "nested": display_path("/app", "/app/project/file.txt"),
                "external": display_path("/app", "/tmp/file.txt"),
            },
            "wrapped_command": terminal_bench_wrap_command(
                "printf 'hello world'",
                Some("/app/project"),
                "__OPENAGENT_TBENCH_EXIT_fixture__",
            ),
            "extract_returncode": [json!(terminal_code), json!(terminal_cleaned)],
            "format_observation": {
                "with_body": terminal_bench_format_observation("hello", 7, 321),
                "empty": terminal_bench_format_observation("", 0, 1),
            },
            "failure_modes": {
                "timeout": terminal_bench_failure_mode("agent timeout"),
                "context": terminal_bench_failure_mode("context length exceeded"),
                "output": terminal_bench_failure_mode("output length exceeded"),
                "unknown": terminal_bench_failure_mode("boom"),
            },
            "system_prompt": terminal_bench_system_prompt("/workspace"),
        },
        "harbor": {
            "defaults": {
                "max_steps": DEFAULT_MAX_STEPS,
                "context_window": DEFAULT_CONTEXT_WINDOW,
                "max_output": DEFAULT_MAX_OUTPUT,
                "workdir": DEFAULT_WORKDIR,
            },
            "metadata": execution_metadata("harbor", "/app", "harbor"),
            "display_paths": {
                "root": display_path("/app", "/app"),
                "nested": display_path("/app", "/app/project/file.txt"),
                "external": display_path("/app", "/tmp/file.txt"),
            },
            "success_command": harbor_success_command,
            "success_result": harbor_success_result,
            "timeout_command": harbor_timeout_command,
            "timeout_result": harbor_timeout_result,
            "normalized_models": {
                "openai": harbor_normalized_model_name(Some("OpenAI/gpt-test")),
                "openai_compatible": harbor_normalized_model_name(Some("openai-compatible/gpt-test")),
                "other_provider": harbor_normalized_model_name(Some("vendor/model")),
                "plain": harbor_normalized_model_name(Some("plain-model")),
                "empty": harbor_normalized_model_name(Some("")),
            },
            "system_prompt": harbor_system_prompt("/workspace"),
        },
    })
}
