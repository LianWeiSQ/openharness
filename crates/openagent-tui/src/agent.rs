use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use serde_json::Value;

use crate::{
    TerminalEventHandler, TuiState, apply_handler_output, picker::filter_agents_for_picker,
};

#[derive(Clone, Debug, Default, PartialEq)]
pub struct AgentPickerState {
    pub query: String,
    pub selected: usize,
    pub agents: Vec<Value>,
    pub candidates: Vec<Value>,
}

pub(crate) fn handle_agent_picker_key<H: TerminalEventHandler>(
    key: KeyEvent,
    state: &mut TuiState,
    handler: &mut H,
) -> Result<(), String> {
    match key.code {
        KeyCode::Esc => {
            state.close_agent_picker();
        }
        KeyCode::Enter => {
            select_agent_picker_from_handler(state, handler)?;
        }
        KeyCode::Up => {
            state.agent_picker_previous();
        }
        KeyCode::Down | KeyCode::Tab => {
            state.agent_picker_next();
        }
        KeyCode::Backspace => {
            if let Some(picker) = state.agent_picker.as_mut() {
                picker.query.pop();
            }
            state.filter_agent_picker();
        }
        KeyCode::Char(ch) if !key.modifiers.contains(KeyModifiers::CONTROL) => {
            if let Some(picker) = state.agent_picker.as_mut() {
                picker.query.push(ch);
            }
            state.filter_agent_picker();
        }
        _ => {}
    }
    Ok(())
}

pub(crate) fn open_agent_picker_from_handler<H: TerminalEventHandler>(
    state: &mut TuiState,
    handler: &mut H,
    query: &str,
) -> Result<(), String> {
    let payload = handler.list_agents()?;
    let agents = payload
        .get("agents")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    state.open_agent_picker(query, agents);
    Ok(())
}

pub(crate) fn select_agent_picker_from_handler<H: TerminalEventHandler>(
    state: &mut TuiState,
    handler: &mut H,
) -> Result<(), String> {
    let Some(agent_id) = state.selected_agent_picker_id() else {
        state.status = "agent picker empty".to_string();
        return Ok(());
    };
    state.close_agent_picker();
    let lines = handler.handle_command(&format!("/agent {agent_id}"))?;
    apply_handler_output(state, handler, lines);
    Ok(())
}

impl TuiState {
    pub fn open_agent_picker(&mut self, query: &str, agents: Vec<Value>) {
        self.file_picker = None;
        self.session_picker = None;
        self.model_picker = None;
        self.choice_picker = None;
        let query = query.trim().to_string();
        let candidates = filter_agents_for_picker(&agents, &query);
        self.agent_picker = Some(AgentPickerState {
            query,
            selected: 0,
            agents,
            candidates,
        });
        self.status = "agent picker".to_string();
    }

    pub fn close_agent_picker(&mut self) {
        self.agent_picker = None;
        self.status = "agent picker closed".to_string();
    }

    pub fn filter_agent_picker(&mut self) {
        let Some(picker) = self.agent_picker.as_mut() else {
            return;
        };
        picker.candidates = filter_agents_for_picker(&picker.agents, &picker.query);
        picker.selected = picker
            .selected
            .min(picker.candidates.len().saturating_sub(1));
        self.status = "agent picker".to_string();
    }

    pub fn agent_picker_previous(&mut self) {
        let Some(picker) = self.agent_picker.as_mut() else {
            return;
        };
        picker.selected = picker.selected.saturating_sub(1);
        self.status = "agent picker".to_string();
    }

    pub fn agent_picker_next(&mut self) {
        let Some(picker) = self.agent_picker.as_mut() else {
            return;
        };
        if !picker.candidates.is_empty() {
            picker.selected = (picker.selected + 1).min(picker.candidates.len() - 1);
        }
        self.status = "agent picker".to_string();
    }

    pub fn selected_agent_picker_id(&self) -> Option<String> {
        self.agent_picker
            .as_ref()
            .and_then(|picker| picker.candidates.get(picker.selected))
            .and_then(|agent| agent.get("id").and_then(Value::as_str))
            .filter(|value| !value.is_empty())
            .map(ToString::to_string)
    }
}
