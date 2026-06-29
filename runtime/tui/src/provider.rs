use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use serde_json::Value;

use crate::{
    TerminalEventHandler, TuiState, apply_handler_output, picker::filter_models_for_picker,
};

#[derive(Clone, Debug, Default, PartialEq)]
pub struct ModelPickerState {
    pub query: String,
    pub selected: usize,
    pub models: Vec<Value>,
    pub candidates: Vec<Value>,
}

pub(crate) fn handle_model_picker_key<H: TerminalEventHandler>(
    key: KeyEvent,
    state: &mut TuiState,
    handler: &mut H,
) -> Result<(), String> {
    match key.code {
        KeyCode::Esc => {
            state.close_model_picker();
        }
        KeyCode::Enter => {
            select_model_picker_from_handler(state, handler)?;
        }
        KeyCode::Up => {
            state.model_picker_previous();
        }
        KeyCode::Down | KeyCode::Tab => {
            state.model_picker_next();
        }
        KeyCode::Backspace => {
            if let Some(picker) = state.model_picker.as_mut() {
                picker.query.pop();
            }
            state.filter_model_picker();
        }
        KeyCode::Char(ch) if !key.modifiers.contains(KeyModifiers::CONTROL) => {
            if let Some(picker) = state.model_picker.as_mut() {
                picker.query.push(ch);
            }
            state.filter_model_picker();
        }
        _ => {}
    }
    Ok(())
}

pub(crate) fn open_model_picker_from_handler<H: TerminalEventHandler>(
    state: &mut TuiState,
    handler: &mut H,
    query: &str,
) -> Result<(), String> {
    let payload = handler.list_models()?;
    let models = payload
        .get("models")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    state.open_model_picker(query, models);
    Ok(())
}

pub(crate) fn select_model_picker_from_handler<H: TerminalEventHandler>(
    state: &mut TuiState,
    handler: &mut H,
) -> Result<(), String> {
    let Some(model_id) = state.selected_model_picker_id() else {
        state.status = "model picker empty".to_string();
        return Ok(());
    };
    state.close_model_picker();
    let lines = handler.handle_command(&format!("/models {model_id}"))?;
    apply_handler_output(state, handler, lines);
    Ok(())
}

impl TuiState {
    pub fn open_model_picker(&mut self, query: &str, models: Vec<Value>) {
        self.file_picker = None;
        self.session_picker = None;
        self.agent_picker = None;
        self.choice_picker = None;
        let query = query.trim().to_string();
        let candidates = filter_models_for_picker(&models, &query);
        self.model_picker = Some(ModelPickerState {
            query,
            selected: 0,
            models,
            candidates,
        });
        self.status = "model picker".to_string();
    }

    pub fn close_model_picker(&mut self) {
        self.model_picker = None;
        self.status = "model picker closed".to_string();
    }

    pub fn filter_model_picker(&mut self) {
        let Some(picker) = self.model_picker.as_mut() else {
            return;
        };
        picker.candidates = filter_models_for_picker(&picker.models, &picker.query);
        picker.selected = picker
            .selected
            .min(picker.candidates.len().saturating_sub(1));
        self.status = "model picker".to_string();
    }

    pub fn model_picker_previous(&mut self) {
        let Some(picker) = self.model_picker.as_mut() else {
            return;
        };
        picker.selected = picker.selected.saturating_sub(1);
        self.status = "model picker".to_string();
    }

    pub fn model_picker_next(&mut self) {
        let Some(picker) = self.model_picker.as_mut() else {
            return;
        };
        if !picker.candidates.is_empty() {
            picker.selected = (picker.selected + 1).min(picker.candidates.len() - 1);
        }
        self.status = "model picker".to_string();
    }

    pub fn selected_model_picker_id(&self) -> Option<String> {
        self.model_picker
            .as_ref()
            .and_then(|picker| picker.candidates.get(picker.selected))
            .and_then(|model| model.get("id").and_then(Value::as_str))
            .filter(|value| !value.is_empty())
            .map(ToString::to_string)
    }
}
