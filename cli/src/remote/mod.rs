use super::*;

mod auth;
mod client;
mod commands;
mod events;
mod http;
mod render;
mod terminal;

use client::{remote_list_sessions, remote_tasks_payload};
use events::{app_event_dedupe_key, app_event_sequence, remote_turn_id};
use http::{http_json, http_json_with_auth, http_text_with_auth};
use render::{remote_sessions_text, remote_tasks_text, tui_lines};
use terminal::RemoteTerminalHandler;

pub(super) use auth::{RemoteAuth, remote_auth_from_args};
pub(super) use client::{
    remote_select_session, remote_select_session_with_auth, remote_start_turn,
    remote_start_turn_with_auth,
};
pub(super) use commands::{attach_command, http_runtime_command, tui_command};
pub(super) use events::{remote_events_for_payload, text_from_app_events};
