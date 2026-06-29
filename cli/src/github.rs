use super::*;

pub(super) fn github_command(args: &[String]) -> CliRunResult {
    if args.is_empty() || args.iter().any(|arg| is_help_flag(arg)) {
        return ok_text("Usage: openagent github <status|issue|pr|workflow> [args...]");
    }
    match args[0].as_str() {
        "status" => run_external_json("gh", &["status"]),
        "issue" => github_issue_command(&args[1..]),
        "pr" => run_external_json(
            "gh",
            &[
                "pr",
                "list",
                "--limit",
                "20",
                "--json",
                "number,title,state,url,headRefName",
            ],
        ),
        "workflow" | "worktree" | "start" => github_workflow_command(&args[1..]),
        other => err_text(2, format!("unknown github command: {other}")),
    }
}

pub(super) fn pr_command(args: &[String]) -> CliRunResult {
    if args.is_empty() || args.iter().any(|arg| is_help_flag(arg)) {
        return ok_text("Usage: openagent pr <list|view|checkout|template|review> [number]");
    }
    match args[0].as_str() {
        "list" | "ls" => run_external_json(
            "gh",
            &[
                "pr",
                "list",
                "--limit",
                "20",
                "--json",
                "number,title,state,url,headRefName",
            ],
        ),
        "view" => {
            let Some(number) = args.get(1) else {
                return err_text(2, "pr view requires a number");
            };
            run_external_json(
                "gh",
                &[
                    "pr",
                    "view",
                    number,
                    "--json",
                    "number,title,state,url,headRefName,body,reviewDecision",
                ],
            )
        }
        "checkout" => {
            let Some(number) = args.get(1).or_else(|| args.first()) else {
                return err_text(2, "pr checkout requires a number");
            };
            run_external_json("gh", &["pr", "checkout", number])
        }
        "template" | "review" => {
            let number = args
                .get(1)
                .cloned()
                .unwrap_or_else(|| "<number>".to_string());
            CliRunResult::ok_json(&json!({
                "schema_version": "openagent.pr_review.v1",
                "number": number,
                "checklist": [
                    "summarize intent and changed files",
                    "run tests or inspect CI",
                    "review behavior regressions and missing tests",
                    "write actionable findings with file/line references"
                ],
                "commands": [
                    format!("gh pr view {number} --json files,comments,reviews,checks"),
                    format!("gh pr diff {number}"),
                ]
            }))
        }
        number => run_external_json("gh", &["pr", "checkout", number]),
    }
}

fn github_issue_command(args: &[String]) -> CliRunResult {
    match args.first().map(String::as_str).unwrap_or("list") {
        "list" | "ls" => run_external_json(
            "gh",
            &[
                "issue",
                "list",
                "--limit",
                "20",
                "--json",
                "number,title,state,url,labels,assignees",
            ],
        ),
        "view" => {
            let Some(number) = args.get(1) else {
                return err_text(2, "github issue view requires a number");
            };
            run_external_json(
                "gh",
                &[
                    "issue",
                    "view",
                    number,
                    "--json",
                    "number,title,state,url,body,labels,assignees",
                ],
            )
        }
        "start" => github_workflow_command(&args[1..]),
        other => err_text(2, format!("unknown github issue command: {other}")),
    }
}

fn github_workflow_command(args: &[String]) -> CliRunResult {
    let positionals = positional_args(args, &["--workspace", "--dir", "--format", "--title"]);
    let issue = positionals
        .iter()
        .find(|value| value.chars().any(|item| item.is_ascii_alphanumeric()))
        .cloned()
        .unwrap_or_else(|| value_for(args, &["--title"]).unwrap_or_else(|| "manual".to_string()));
    let workflow_id = format!("workflow_{}", sanitize_identifier(&issue));
    let path = workspace_from_args(args)
        .join(".openagent/github/workflows")
        .join(format!("{workflow_id}.json"));
    let branch = format!("openagent/{}", sanitize_identifier(&issue));
    let payload = json!({
        "schema_version": "openagent.github_workflow.v1",
        "id": workflow_id,
        "issue": issue,
        "branch": branch,
        "status": "planned",
        "steps": [
            "inspect issue and repository state",
            "create or switch to the workflow branch",
            "implement the smallest verified slice",
            "run tests and capture evidence",
            "open or update a pull request"
        ],
        "commands": [
            format!("git switch -c {branch}"),
            "openagent run --skip-doctor \"implement issue scope\"".to_string(),
            "gh pr create --draft".to_string()
        ],
        "created_at_ms": now_ms_cli(),
    });
    if let Err(error) = write_json_file(&path, &payload) {
        return err_text(1, error);
    }
    CliRunResult::ok_json(
        &json!({"created": true, "path": path.to_string_lossy(), "workflow": payload}),
    )
}
