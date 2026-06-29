#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
pub struct WorkState {
    pub task: String,
    pub progress: Vec<String>,
    pub decisions: Vec<String>,
    pub files: Vec<WorkStateFile>,
    pub tool_findings: Vec<String>,
    pub todos: Vec<String>,
    pub open_questions: Vec<String>,
    pub blockers: Vec<String>,
    pub next_steps: Vec<String>,
    pub risks: Vec<String>,
}

#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
pub struct WorkStateFile {
    pub path: String,
    pub status: String,
    pub note: String,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct CompactionRecord {
    pub schema_version: u64,
    pub format: String,
    pub state: WorkState,
    pub summary: String,
    pub compacted_until: u64,
    pub updated_at: u64,
    pub source: String,
}

#[must_use]
pub fn render_work_state(state: &WorkState) -> String {
    let mut sections = vec![
        "[Structured work state]".to_string(),
        "Task:".to_string(),
        if state.task.is_empty() {
            "(unspecified)".to_string()
        } else {
            state.task.clone()
        },
    ];

    append_text_section(&mut sections, "Progress", &state.progress);
    append_text_section(&mut sections, "Decisions", &state.decisions);
    append_files_section(&mut sections, &state.files);
    append_text_section(&mut sections, "Tool findings", &state.tool_findings);
    append_text_section(&mut sections, "Todos", &state.todos);
    append_text_section(&mut sections, "Open questions", &state.open_questions);
    append_text_section(&mut sections, "Blockers", &state.blockers);
    append_text_section(&mut sections, "Next steps", &state.next_steps);
    append_text_section(&mut sections, "Risks", &state.risks);

    sections.join("\n").trim().to_string()
}

#[must_use]
pub fn build_compaction_record(
    state: WorkState,
    compacted_until: u64,
    updated_at: u64,
) -> CompactionRecord {
    CompactionRecord {
        schema_version: 1,
        format: "structured_work_state".to_string(),
        summary: render_work_state(&state),
        state,
        compacted_until,
        updated_at,
        source: "model_json".to_string(),
    }
}
