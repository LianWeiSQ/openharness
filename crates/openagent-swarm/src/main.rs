use std::env;

use openagent_swarm::{
    SwarmRuntime, build_transport_registry, load_swarm_config, swarm_run_result_to_json,
};

#[tokio::main]
async fn main() {
    let code = match run().await {
        Ok(code) => code,
        Err(error) => {
            eprintln!(
                "{}",
                serde_json::json!({"status": "error", "error": error.to_string()})
            );
            2
        }
    };
    std::process::exit(code);
}

async fn run() -> Result<i32, Box<dyn std::error::Error + Send + Sync + 'static>> {
    let args = env::args().skip(1).collect::<Vec<_>>();
    if args.first().map(String::as_str) != Some("run") {
        println!("{}", openagent_swarm::command_name());
        return Ok(0);
    }

    let config_path = args
        .get(1)
        .ok_or_else(|| "openagent-swarm run requires a config path".to_string())?;
    let task_id = arg_value(&args, "--task")
        .ok_or_else(|| "openagent-swarm run requires --task".to_string())?;
    let run_id = arg_value(&args, "--run-id").unwrap_or_else(|| "swarm_cli".to_string());
    let pretty = args.iter().any(|arg| arg == "--pretty");
    let config = load_swarm_config(config_path)?;
    let task = config
        .task(&task_id)
        .ok_or_else(|| format!("unknown task: {task_id}"))?;
    let registry = build_transport_registry(&config)?;
    let runtime = SwarmRuntime::new(registry, config.fanout_budget.clone());
    let result = runtime.run_task(&task, Some(run_id.clone())).await;
    let payload = swarm_run_result_to_json(&result, &run_id);
    let output = if pretty {
        serde_json::to_string_pretty(&payload)?
    } else {
        serde_json::to_string(&payload)?
    };
    println!("{output}");
    if matches!(result.status.as_str(), "completed" | "partial") {
        Ok(0)
    } else {
        Ok(1)
    }
}

fn arg_value(args: &[String], name: &str) -> Option<String> {
    args.windows(2)
        .find(|items| items.first().map(String::as_str) == Some(name))
        .and_then(|items| items.get(1))
        .cloned()
}
