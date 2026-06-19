use std::{
    error::Error,
    fs,
    path::PathBuf,
    time::{SystemTime, UNIX_EPOCH},
};

use openagent_tools::{
    LocalWorkspaceRuntime, TodoItem, ToolContext, ToolRegistry, Toolkit, blocked_command,
    ensure_within_root, exclusive_schema, format_read_output_from_text, qualify_tool_id,
    readonly_schema, register_builtin_tools, truncate_output,
};
use serde::Serialize;
use serde_json::{Value, json};

#[test]
fn tool_runtime_fixture_matches_python_oracle() -> Result<(), Box<dyn Error>> {
    let fixture: Value = serde_json::from_str(include_str!(
        "../../../tests/golden/rust_rewrite/tool_runtime.json"
    ))?;
    assert_eq!(fixture, tool_runtime_fixture()?);
    Ok(())
}

#[test]
fn file_tools_enforce_path_safety_read_before_write_and_metadata() -> Result<(), Box<dyn Error>> {
    let root = unique_temp_dir("openagent-tools-file")?;
    fs::write(root.join("notes.txt"), "alpha\nbeta\ngamma\n")?;
    fs::create_dir_all(root.join("src"))?;
    fs::write(
        root.join("src").join("main.rs"),
        "fn main() {\n  println!(\"beta\");\n}\n",
    )?;

    let toolkit = Toolkit::with_builtins();
    let mut ctx = ToolContext::new(&root).with_session_id("session/file");

    let escaped = toolkit.execute(
        "read",
        json!({"file_path": "../outside.txt"}),
        "call_escape",
        &mut ctx,
    );
    assert!(
        escaped
            .error
            .as_deref()
            .unwrap_or_default()
            .contains("Path escapes session root")
    );

    let blocked_write = toolkit.execute(
        "write",
        json!({"file_path": "notes.txt", "content": "blocked"}),
        "call_blocked_write",
        &mut ctx,
    );
    assert!(
        blocked_write
            .error
            .as_deref()
            .unwrap_or_default()
            .contains("Must read existing file before writing")
    );

    let read = toolkit.execute(
        "read",
        json!({"file_path": "notes.txt", "offset": 1, "limit": 1}),
        "call_read",
        &mut ctx,
    );
    assert!(read.error.is_none());
    assert_eq!(
        read.output,
        "<file>\n00002| beta\n\n(File has more lines. Use 'offset' parameter to read beyond line 2)\n</file>"
    );
    assert_eq!(read.metadata["preview"], json!("beta"));
    assert_eq!(read.metadata["tool"], json!("read"));
    assert_eq!(read.metadata["title"], json!("notes.txt"));
    assert_eq!(read.metadata["truncated"], json!(true));

    let edit = toolkit.execute(
        "edit",
        json!({
            "file_path": "notes.txt",
            "old_string": "beta",
            "new_string": "delta",
        }),
        "call_edit",
        &mut ctx,
    );
    assert!(edit.error.is_none());
    assert_eq!(
        fs::read_to_string(root.join("notes.txt"))?,
        "alpha\ndelta\ngamma\n"
    );
    assert_eq!(edit.metadata["replace_all"], json!(false));

    let write_new = toolkit.execute(
        "write",
        json!({"file_path": "new.txt", "content": "fresh"}),
        "call_write_new",
        &mut ctx,
    );
    assert!(write_new.error.is_none());
    assert_eq!(write_new.metadata["exists"], json!(false));

    let glob = toolkit.execute("glob", json!({"pattern": "**/*.rs"}), "call_glob", &mut ctx);
    assert!(glob.output.contains("main.rs"));
    assert_eq!(glob.metadata["count"], json!(1));

    let grep = toolkit.execute(
        "grep",
        json!({"pattern": "println", "include": "*.rs"}),
        "call_grep",
        &mut ctx,
    );
    assert!(grep.output.contains("Found 1 matches"));
    assert_eq!(grep.metadata["include"], json!("*.rs"));

    let ls = toolkit.execute("ls", json!({"ignore": ["new.txt"]}), "call_ls", &mut ctx);
    assert!(ls.output.contains("notes.txt"));
    assert!(!ls.output.contains("new.txt"));
    assert!(ls.metadata["count"].as_u64().unwrap_or_default() >= 2);

    let code_search = toolkit.execute(
        "code_search",
        json!({"query": "delta", "glob": "*.txt"}),
        "call_code_search",
        &mut ctx,
    );
    assert!(code_search.output.contains("notes.txt"));
    assert_eq!(code_search.metadata["count"], json!(1));

    fs::remove_dir_all(root)?;
    Ok(())
}

#[test]
fn shell_runtime_blocks_destructive_commands_and_saves_truncated_output()
-> Result<(), Box<dyn Error>> {
    let root = unique_temp_dir("openagent-tools-shell")?;
    let mut toolkit = Toolkit::with_builtins();
    toolkit.max_output_bytes = 8;
    let mut ctx = ToolContext::new(&root).with_session_id("session-shell");

    let blocked = toolkit.execute(
        "bash",
        json!({"command": "printf ok; rm -rf tmp"}),
        "call_rm",
        &mut ctx,
    );
    assert!(
        blocked
            .error
            .as_deref()
            .unwrap_or_default()
            .contains("rm command is disabled")
    );
    assert_eq!(
        blocked_command("printf ok; rm -rf tmp"),
        Some("rm".to_string())
    );

    let result = toolkit.execute(
        "bash",
        json!({"command": "printf abcdefghijklmnopqrstuvwxyz", "description": "long output"}),
        "call_long",
        &mut ctx,
    );
    assert!(result.error.is_none());
    assert!(result.output.contains("... output truncated"));
    assert_eq!(result.metadata["output_truncated"], json!(true));
    let output_path = result.metadata["output_path"].as_str().unwrap_or_default();
    assert_eq!(
        fs::read_to_string(output_path)?,
        "abcdefghijklmnopqrstuvwxyz"
    );

    let runtime = LocalWorkspaceRuntime::new(&root);
    let command = runtime.run_command("printf runtime-ok", None, 120_000)?;
    assert_eq!(command.returncode, 0);
    assert_eq!(command.stdout, "runtime-ok");
    assert!(runtime.resolve_path(Some("../outside"), true).is_err());

    fs::remove_dir_all(root)?;
    Ok(())
}

#[test]
fn todo_memory_and_question_tools_round_trip_session_state() -> Result<(), Box<dyn Error>> {
    let root = unique_temp_dir("openagent-tools-state")?;
    let toolkit = Toolkit::with_builtins();
    let mut ctx = ToolContext::new(&root).with_session_id("session/state");

    let missing = toolkit.execute(
        "memory_read",
        json!({"key": "missing"}),
        "call_mem_missing",
        &mut ctx,
    );
    assert_eq!(missing.output, "null");
    let write = toolkit.execute(
        "memory_write",
        json!({"key": "profile", "value": {"name": "Ada"}}),
        "call_mem_write",
        &mut ctx,
    );
    assert_eq!(write.output, "ok");
    let read = toolkit.execute(
        "memory_read",
        json!({"key": "profile"}),
        "call_mem_read",
        &mut ctx,
    );
    assert_eq!(read.output, "{\"name\":\"Ada\"}");

    let todos = vec![TodoItem::new(
        "port tools",
        "in_progress",
        "high",
        "todo-fixture",
    )];
    let todo_write = toolkit.execute(
        "todowrite",
        json!({"todos": todos}),
        "call_todo_write",
        &mut ctx,
    );
    assert_eq!(todo_write.metadata["title"], json!("1 todos"));
    assert!(todo_write.output.contains("\"id\": \"todo-fixture\""));
    let todo_read = toolkit.execute("todoread", json!({}), "call_todo_read", &mut ctx);
    assert_eq!(todo_read.output, todo_write.output);

    ctx.set_question_answers(vec![vec!["Fast".to_string()]]);
    let question = toolkit.execute(
        "question",
        json!({
            "questions": [{
                "header": "Mode",
                "question": "Pick a mode",
                "options": [{"label": "Fast", "description": "Run quickly"}],
            }]
        }),
        "call_question",
        &mut ctx,
    );
    assert_eq!(
        question.output,
        "User has answered your questions: \"Pick a mode\"=\"Fast\". You can now continue with the user's answers in mind."
    );
    assert_eq!(question.metadata["count"], json!(1));

    fs::remove_dir_all(root)?;
    Ok(())
}

fn tool_runtime_fixture() -> Result<Value, Box<dyn Error>> {
    let mut registry = ToolRegistry::new();
    register_builtin_tools(&mut registry);
    let selected = [
        "read",
        "write",
        "edit",
        "glob",
        "grep",
        "ls",
        "bash",
        "code_search",
        "memory_read",
        "memory_write",
        "todowrite",
        "todoread",
        "question",
    ];
    let mut tools = serde_json::Map::new();
    for tool_id in selected {
        let tool = registry
            .get(tool_id)
            .ok_or_else(|| format!("missing tool: {tool_id}"))?;
        let properties = sorted_property_names(&tool.parameter_schema);
        let required = tool
            .parameter_schema
            .get("required")
            .cloned()
            .unwrap_or_else(|| json!([]));
        tools.insert(
            tool_id.to_string(),
            json!({
                "group": tool.group,
                "dangerous": tool.dangerous,
                "execution_scope": to_value(&tool.execution_scope)?,
                "execution_schema": to_value(&tool.execution_schema)?,
                "parameter_schema": {
                    "required": required,
                    "properties": properties,
                },
            }),
        );
    }

    let read_format = format_read_output_from_text("alpha\nbeta\ngamma\n", 1, 1);
    let path_escape_error = ensure_within_root("/tmp/openagent-fixture", "/tmp/outside.txt")
        .err()
        .ok_or_else(|| "path escape fixture did not fail".to_string())?;
    let todo_output = serde_json::to_string_pretty(&vec![TodoItem::new(
        "port tools",
        "in_progress",
        "high",
        "todo-fixture",
    )])?;

    Ok(json!({
        "schema_version": 1,
        "tools": Value::Object(tools),
        "registry_namespace": {
            "default": qualify_tool_id("fixture", "default"),
            "custom": qualify_tool_id("fixture", "custom"),
        },
        "execution_schemas": {
            "readonly": to_value(readonly_schema("workspace-read", false, true, None))?,
            "exclusive": to_value(exclusive_schema(
                "workspace-write",
                true,
                true,
                false,
                false,
                false,
                Some("file:{file_path}"),
            ))?,
        },
        "read_format": to_value(read_format)?,
        "truncation": {
            "line": to_value(truncate_output("L1\nL2\nL3", Some(2), Some(999)))?,
            "byte": to_value(truncate_output("abcdef", Some(999), Some(4)))?,
        },
        "path_escape_error": path_escape_error,
        "blocked_shell_command": blocked_command("printf ok; rm -rf tmp"),
        "todo_output": todo_output,
        "memory_outputs": {"missing": "null", "write": "ok"},
        "question_output": "User has answered your questions: \"Pick a mode\"=\"Fast\". You can now continue with the user's answers in mind.",
    }))
}

fn sorted_property_names(schema: &Value) -> Vec<String> {
    let mut properties = schema
        .get("properties")
        .and_then(Value::as_object)
        .map(|items| items.keys().cloned().collect::<Vec<_>>())
        .unwrap_or_default();
    properties.sort();
    properties
}

fn to_value<T: Serialize>(value: T) -> Result<Value, serde_json::Error> {
    serde_json::to_value(value)
}

fn unique_temp_dir(prefix: &str) -> Result<PathBuf, Box<dyn Error>> {
    let nanos = match SystemTime::now().duration_since(UNIX_EPOCH) {
        Ok(duration) => duration.as_nanos(),
        Err(_) => 0,
    };
    let path = std::env::temp_dir().join(format!("{prefix}-{}-{nanos}", std::process::id()));
    fs::create_dir_all(&path)?;
    Ok(path)
}
