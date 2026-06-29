#[test]
fn composer_history_and_stash_round_trip() {
    let mut state = TuiState::new();
    state.remember_history("first prompt");
    state.remember_history("second prompt");

    state.history_previous();
    assert_eq!(state.input_buffer, "second prompt");
    state.history_previous();
    assert_eq!(state.input_buffer, "first prompt");
    state.history_next();
    assert_eq!(state.input_buffer, "second prompt");
    state.history_next();
    assert_eq!(state.input_buffer, "");

    state.input_buffer = "draft body".to_string();
    state.stash_current_input();
    assert_eq!(state.input_buffer, "");
    assert_eq!(state.stash.len(), 1);
    state.restore_latest_stash();
    assert_eq!(state.input_buffer, "draft body");

    state.input_buffer = "/stash queued draft".to_string();
    assert!(!state.submit());
    assert_eq!(state.stash.last().map(String::as_str), Some("queued draft"));
    state.input_buffer = "/unstash".to_string();
    assert!(!state.submit());
    assert_eq!(state.input_buffer, "queued draft");
}

#[test]
fn composer_expands_file_line_ranges_and_image_attachments() {
    let root = std::env::temp_dir().join(format!("openagent-tui-composer-{}", std::process::id()));
    let src = root.join("src");
    fs::create_dir_all(&src).expect("create src");
    fs::write(src.join("main.rs"), "line1\nline2\nline3\nline4\n").expect("write source");
    fs::write(root.join("logo.png"), [0_u8, 1, 2, 3]).expect("write image");

    let expanded = expand_file_attachments(&root, "review @main.rs:2-3 and @logo.png");

    assert!(expanded.prompt.contains("Attached file: src/main.rs:2-3"));
    assert!(expanded.prompt.contains("line2\nline3"));
    assert!(!expanded.prompt.contains("line1\n"));
    assert!(expanded.prompt.contains("Attached image: logo.png"));
    assert_eq!(expanded.lines.len(), 2);

    let _ = fs::remove_dir_all(root);
}

#[test]
fn composer_file_picker_and_attach_controls_insert_references() {
    let root =
        std::env::temp_dir().join(format!("openagent-tui-file-picker-{}", std::process::id()));
    let src = root.join("src");
    let docs = root.join("docs");
    fs::create_dir_all(&src).expect("create src");
    fs::create_dir_all(&docs).expect("create docs");
    fs::write(src.join("main.rs"), "fn main() {}\n").expect("write main");
    fs::write(docs.join("guide.md"), "guide\n").expect("write guide");
    fs::write(root.join("logo.png"), [0_u8, 1, 2, 3]).expect("write image");

    let matches = fuzzy_find_files(&root, "main", 10);
    assert_eq!(
        matches.first().map(|item| item.reference.as_str()),
        Some("@src/main.rs")
    );
    let lines = file_picker_lines("main", &matches);
    assert!(lines.iter().any(|line| line.text.contains("@src/main.rs")));

    let mut state = TuiState::new();
    state.input_buffer = "/attach src/main.rs:2-3".to_string();
    assert!(!state.submit());
    assert_eq!(state.input_buffer, "@src/main.rs:2-3 ");

    state.input_buffer = "review".to_string();
    let selected = state.apply_control_request(&json!({
        "path": "/tui/select-file",
        "body": {"path": "docs/guide.md", "start": 4, "end": 6}
    }));
    assert_eq!(selected["applied"], json!(true));
    assert_eq!(selected["reference"], json!("@docs/guide.md:4-6"));
    assert_eq!(state.input_buffer, "review @docs/guide.md:4-6 ");

    let image = state.apply_control_request(&json!({
        "path": "/tui/publish",
        "body": {"type": "tui.file.attach", "properties": {"path": "logo.png"}}
    }));
    assert_eq!(image["applied"], json!(true));
    assert!(state.input_buffer.ends_with("@logo.png "));

    let _ = fs::remove_dir_all(root);
}
