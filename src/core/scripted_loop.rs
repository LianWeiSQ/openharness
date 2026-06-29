#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct ScriptedLoopInput {
    pub user_text: String,
    pub script: Vec<ScriptedLoopCall>,
    pub tools: Vec<String>,
    #[serde(default)]
    pub options: BTreeMap<String, Value>,
    pub max_steps: u64,
    pub doom_loop_threshold: u64,
    #[serde(default)]
    pub reply_questions: bool,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct ScriptedLoopCall {
    #[serde(default)]
    pub events: Vec<Value>,
    #[serde(default)]
    pub error: Option<String>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct ScriptedLoopOutput {
    pub events: Vec<Value>,
    pub event_types: Vec<String>,
    pub model_call_count: u64,
    pub seen_tools_by_call: Vec<Vec<String>>,
    pub seen_max_output_tokens_by_call: Vec<Option<u64>>,
    pub pause_statuses: Vec<String>,
    pub final_session_status: String,
}

#[must_use]
pub fn run_scripted_agent_loop(input: &ScriptedLoopInput) -> ScriptedLoopOutput {
    let mut runner = ScriptedAgentLoopRunner::new(input);
    runner.run();
    runner.finish()
}

struct ScriptedAgentLoopRunner<'a> {
    input: &'a ScriptedLoopInput,
    script_index: usize,
    events: Vec<Value>,
    seen_tools_by_call: Vec<Vec<String>>,
    seen_max_output_tokens_by_call: Vec<Option<u64>>,
    pause_statuses: Vec<String>,
    doom_history: Vec<String>,
    snapshot_count: u64,
    text_count: u64,
    final_session_status: String,
}

impl<'a> ScriptedAgentLoopRunner<'a> {
    fn new(input: &'a ScriptedLoopInput) -> Self {
        Self {
            input,
            script_index: 0,
            events: Vec::new(),
            seen_tools_by_call: Vec::new(),
            seen_max_output_tokens_by_call: Vec::new(),
            pause_statuses: Vec::new(),
            doom_history: Vec::new(),
            snapshot_count: 0,
            text_count: 0,
            final_session_status: "running".to_string(),
        }
    }

    fn run(&mut self) {
        let max_retry = 1_u64;
        for step_index in 1..=self.input.max_steps {
            self.snapshot_count += 1;
            self.events.push(json!({
                "type": "step-start",
                "snapshot_id": format!("snapshot_{}", self.snapshot_count),
            }));

            let mut attempt = 0_u64;
            let step = loop {
                attempt += 1;
                self.seen_tools_by_call.push(self.input.tools.clone());
                self.seen_max_output_tokens_by_call.push(Some(256));
                let Some(call) = self.next_script_call() else {
                    break ModelStep::default();
                };
                if let Some(error) = &call.error {
                    if attempt <= max_retry {
                        continue;
                    }
                    self.events.push(json!({"type": "error", "error": error}));
                    self.final_session_status = "stop".to_string();
                    return;
                }
                break self.process_model_events(&call.events);
            };

            for call in &step.tool_calls {
                if self.record_doom_loop(call) {
                    let name = call.get("name").and_then(Value::as_str).unwrap_or_default();
                    let input_value = call.get("input").cloned().unwrap_or_else(|| json!({}));
                    self.events.push(json!({
                        "type": "error",
                        "error": format!(
                            "Detected repeated tool-call loop (threshold={}): {} {}",
                            self.input.doom_loop_threshold,
                            name,
                            stable_json_dumps(&input_value)
                        ),
                    }));
                    self.final_session_status = "stop".to_string();
                    return;
                }
                let name = call.get("name").and_then(Value::as_str).unwrap_or_default();
                if name == "question" {
                    self.emit_question_result(call);
                } else {
                    self.emit_fixture_echo_result(call);
                }
            }

            for warning in
                step_usage_warnings_from_options(&self.input.options, &step.usage, step_index)
            {
                self.events.push(warning);
            }

            let finish_reason = if !step.tool_calls.is_empty() && step.finish_reason == "unknown" {
                "tool_call".to_string()
            } else {
                step.finish_reason.clone()
            };
            self.events.push(json!({
                "type": "step-finish",
                "tokens": {
                    "input": step.usage.input_tokens,
                    "output": step.usage.output_tokens,
                },
                "cost": step.usage.cost,
                "finish_reason": finish_reason,
            }));

            if !step.tool_calls.is_empty() {
                continue;
            }
            if finish_reason == "stop" || step_index >= self.input.max_steps {
                self.final_session_status = "stop".to_string();
                return;
            }
        }
        self.events
            .push(json!({"type": "error", "error": "max_steps exceeded"}));
        self.final_session_status = "stop".to_string();
    }

    fn finish(self) -> ScriptedLoopOutput {
        let event_types = self
            .events
            .iter()
            .filter_map(|event| event.get("type").and_then(Value::as_str))
            .map(str::to_string)
            .collect();
        ScriptedLoopOutput {
            events: self.events,
            event_types,
            model_call_count: self.seen_tools_by_call.len() as u64,
            seen_tools_by_call: self.seen_tools_by_call,
            seen_max_output_tokens_by_call: self.seen_max_output_tokens_by_call,
            pause_statuses: self.pause_statuses,
            final_session_status: self.final_session_status,
        }
    }

    fn next_script_call(&mut self) -> Option<ScriptedLoopCall> {
        let call = self.input.script.get(self.script_index).cloned();
        self.script_index += usize::from(call.is_some());
        call
    }

    fn process_model_events(&mut self, events: &[Value]) -> ModelStep {
        let mut step = ModelStep::default();
        let mut text_started = false;
        let mut text_id = String::new();
        for event in events {
            match event
                .get("type")
                .and_then(Value::as_str)
                .unwrap_or_default()
            {
                "text-delta" => {
                    if !text_started {
                        text_started = true;
                        self.text_count += 1;
                        text_id = format!("text_{}", self.text_count);
                        self.events.push(json!({
                            "type": "text-start",
                            "id": text_id,
                            "metadata": Value::Null,
                        }));
                    }
                    self.events.push(json!({
                        "type": "text-delta",
                        "id": text_id,
                        "text": event.get("text").and_then(Value::as_str).unwrap_or_default(),
                    }));
                }
                "tool-call" => {
                    let call = json!({
                        "type": "tool-call",
                        "call_id": event.get("call_id").and_then(Value::as_str).unwrap_or_default(),
                        "name": event.get("name").and_then(Value::as_str).unwrap_or_default(),
                        "input": event.get("input").cloned().unwrap_or_else(|| json!({})),
                    });
                    self.events.push(call.clone());
                    step.tool_calls.push(call);
                }
                "finish" => {
                    step.finish_reason = event
                        .get("finish_reason")
                        .and_then(Value::as_str)
                        .unwrap_or("unknown")
                        .to_string();
                    step.usage = usage_from_loop_event(event.get("usage"));
                }
                _ => {}
            }
        }
        if text_started {
            self.events.push(json!({"type": "text-end", "id": text_id}));
        }
        step
    }

    fn record_doom_loop(&mut self, call: &Value) -> bool {
        let name = call.get("name").and_then(Value::as_str).unwrap_or_default();
        let input_value = call.get("input").cloned().unwrap_or_else(|| json!({}));
        let key = format!("{name}:{}", stable_json_dumps(&input_value));
        self.doom_history.push(key);
        let threshold = self.input.doom_loop_threshold as usize;
        if self.doom_history.len() > threshold {
            self.doom_history.remove(0);
        }
        self.doom_history.len() == threshold
            && self
                .doom_history
                .first()
                .is_some_and(|first| self.doom_history.iter().all(|item| item == first))
    }

    fn emit_fixture_echo_result(&mut self, call: &Value) {
        let call_id = call
            .get("call_id")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let input = call.get("input").cloned().unwrap_or_else(|| json!({}));
        let value = input.get("value").and_then(Value::as_str).unwrap_or("ok");
        let output = format!("echo:{value}");
        let original_bytes = output.len() as u64;
        self.events.push(json!({
            "type": "tool-result",
            "call_id": call_id,
            "output": output,
            "error": Value::Null,
            "metadata": {
                "context_preview": output,
                "kind": "fixture_echo",
                "original_bytes": original_bytes,
                "original_lines": 1,
                "output_truncated": false,
                "title": "Echo",
                "tool": "fixture_echo",
                "truncated": false,
            },
        }));
    }

    fn emit_question_result(&mut self, call: &Value) {
        let call_id = call
            .get("call_id")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let questions = call
            .get("input")
            .and_then(|input| input.get("questions"))
            .cloned()
            .unwrap_or_else(|| json!([]));
        self.pause_statuses.push("paused".to_string());
        self.events.push(json!({
            "type": "question-request",
            "request_id": "question_1",
            "session_id": "session_fixture",
            "tool_call_id": call_id,
            "questions": questions,
        }));
        if !self.input.reply_questions {
            self.events.push(json!({
                "type": "tool-result",
                "call_id": call_id,
                "output": "",
                "error": "The user dismissed this question",
                "metadata": {
                    "questions": questions,
                    "request_id": "question_1",
                    "count": questions.as_array().map_or(0, Vec::len),
                    "error_kind": "question_rejected",
                    "tool": "question",
                    "title": "Asked 1 question",
                    "truncated": false,
                    "output_truncated": false,
                    "original_lines": 0,
                    "original_bytes": 0,
                },
            }));
            return;
        }
        let question_text = questions
            .as_array()
            .and_then(|items| items.first())
            .and_then(|item| item.get("question"))
            .and_then(Value::as_str)
            .unwrap_or("Question");
        let output = format!(
            "User has answered your questions: \"{question_text}\"=\"Fast path\". You can now continue with the user's answers in mind."
        );
        let original_bytes = output.len() as u64;
        self.events.push(json!({
            "type": "tool-result",
            "call_id": call_id,
            "output": output,
            "error": Value::Null,
            "metadata": {
                "answers": [["Fast path"]],
                "context_preview": output,
                "count": questions.as_array().map_or(0, Vec::len),
                "original_bytes": original_bytes,
                "original_lines": 1,
                "output_truncated": false,
                "questions": questions,
                "request_id": "question_1",
                "title": "Asked 1 question",
                "tool": "question",
                "truncated": false,
            },
        }));
    }
}

#[derive(Clone, Debug)]
struct ModelStep {
    tool_calls: Vec<Value>,
    finish_reason: String,
    usage: Usage,
}

impl Default for ModelStep {
    fn default() -> Self {
        Self {
            tool_calls: Vec::new(),
            finish_reason: "unknown".to_string(),
            usage: Usage::default(),
        }
    }
}

fn usage_from_loop_event(value: Option<&Value>) -> Usage {
    let Some(value) = value else {
        return Usage::default();
    };
    Usage {
        input_tokens: value
            .get("input_tokens")
            .and_then(Value::as_u64)
            .unwrap_or(0),
        output_tokens: value
            .get("output_tokens")
            .and_then(Value::as_u64)
            .unwrap_or(0),
        cost: value.get("cost").and_then(Value::as_f64).unwrap_or(0.0),
    }
}

fn step_usage_warnings_from_options(
    options: &BTreeMap<String, Value>,
    usage: &Usage,
    step_index: u64,
) -> Vec<Value> {
    let Some(raw) = options.get("runtime_warnings").and_then(Value::as_object) else {
        return Vec::new();
    };
    let threshold = raw
        .get("max_step_total_tokens")
        .and_then(Value::as_u64)
        .unwrap_or(0);
    let enabled = raw
        .get("enabled")
        .and_then(Value::as_bool)
        .unwrap_or(threshold > 0);
    if !enabled || threshold == 0 {
        return Vec::new();
    }
    let total_tokens = usage.input_tokens + usage.output_tokens;
    if total_tokens <= threshold {
        return Vec::new();
    }
    let message = format!("Step total tokens exceeded budget: {total_tokens} > {threshold}.");
    vec![json!({
        "type": "runtime-warning",
        "severity": "warning",
        "code": "step_total_tokens_exceeded",
        "message": message,
        "metrics": {
            "step_index": step_index,
            "input_tokens": usage.input_tokens,
            "output_tokens": usage.output_tokens,
            "total_tokens": total_tokens,
            "cost": usage.cost,
            "threshold": threshold,
        },
        "display": {
            "kind": "runtime_warning",
            "severity": "warning",
            "title": "Step token budget exceeded",
            "body": message,
            "metrics": {
                "step_index": step_index,
                "input_tokens": usage.input_tokens,
                "output_tokens": usage.output_tokens,
                "total_tokens": total_tokens,
                "threshold": threshold,
            },
        },
    })]
}
