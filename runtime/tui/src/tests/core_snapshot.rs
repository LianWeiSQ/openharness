#[test]
fn terminal_render_snapshot_contains_core_regions() {
    let backend = TestBackend::new(80, 18);
    let mut terminal = Terminal::new(backend).expect("terminal");
    let mut state = TuiState::new();
    state.status = "running".to_string();
    state
        .timeline
        .push(TimelineLine::new("user", "> hello", true));
    state
        .timeline
        .push(TimelineLine::new("assistant", "world", true));
    state.input_buffer = "next".to_string();

    terminal
        .draw(|frame| draw_terminal_frame(frame, "OpenAgent", &state))
        .expect("draw frame");
    let rendered = format!("{:?}", terminal.backend().buffer());

    assert!(rendered.contains("App Bridge"));
    assert!(rendered.contains("Timeline"));
    assert!(rendered.contains("Prompt"));
    assert!(rendered.contains("OpenAgent"));
    assert!(rendered.contains("world"));
}
