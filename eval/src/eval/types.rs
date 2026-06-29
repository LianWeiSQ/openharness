#[must_use]
pub const fn crate_name() -> &'static str {
    CRATE_NAME
}

#[must_use]
pub fn core_crate_name() -> &'static str {
    openagent_core::crate_name()
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct EvalResult {
    pub case_id: String,
    pub status: String,
    pub score: f64,
    pub duration_ms: i64,
    pub steps: i64,
    pub tool_calls: i64,
    pub input_tokens: i64,
    pub output_tokens: i64,
    pub cost: f64,
    pub error_kind: Option<String>,
    pub failure_reasons: Vec<String>,
    pub trace_path: Option<String>,
    pub session_id: Option<String>,
    pub run_id: Option<String>,
    pub ledger_path: Option<String>,
    pub session_state_path: Option<String>,
    pub trace_summary_path: Option<String>,
    pub trace_check_ok: bool,
    pub trace_check_errors: Vec<String>,
    pub trace_event_count: i64,
    pub model_calls: i64,
    pub mcp_calls: i64,
    pub skill_calls: i64,
    pub local_tool_calls: i64,
    pub artifact_count: i64,
    pub error_count: i64,
    pub runtime_warning_count: i64,
    pub runtime_warning_codes: Vec<String>,
    pub total_latency_ms: i64,
    pub langfuse_trace_id: Option<String>,
    pub langfuse_scores_sent: bool,
    pub langfuse_error: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct EvalCiGateResult {
    pub ok: bool,
    pub status: String,
    pub reasons: Vec<String>,
    pub metrics: Value,
    pub report_path: Option<String>,
    pub regression_path: Option<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct EvalCiGateOptions {
    pub min_success_rate: f64,
    pub max_runtime_warnings: Option<i64>,
    pub require_trace_check: bool,
    pub fail_on_budget_regressions: bool,
    pub fail_on_status_regressions: bool,
}

impl Default for EvalCiGateOptions {
    fn default() -> Self {
        Self {
            min_success_rate: 1.0,
            max_runtime_warnings: None,
            require_trace_check: true,
            fail_on_budget_regressions: true,
            fail_on_status_regressions: true,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CommandResult {
    pub cwd: String,
    pub returncode: i64,
    pub stderr: String,
    pub stdout: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct HarborCommandRecord {
    pub command: String,
    pub cwd: String,
    pub timeout_sec: i64,
}

#[derive(Clone, Debug, PartialEq)]
pub struct HarborSuccessSpec<'a> {
    pub command: &'a str,
    pub cwd: Option<&'a str>,
    pub timeout_ms: i64,
    pub workspace_root: &'a str,
    pub returncode: i64,
    pub stdout: &'a str,
    pub stderr: &'a str,
    pub elapsed_ms: i64,
}
