#[must_use]
pub fn dockerfile_lines() -> Vec<&'static str> {
    vec![
        "FROM rust:1.85-bookworm AS builder",
        "WORKDIR /app",
        "COPY . .",
        "RUN cargo build --release -p openagent-http-runtime",
        "FROM debian:bookworm-slim",
        "COPY --from=builder /app/target/release/openagent-http-runtime /usr/local/bin/openagent-http-runtime",
        "EXPOSE 8787",
        "HEALTHCHECK CMD [\"openagent-http-runtime\", \"--health-json\"]",
        "ENTRYPOINT [\"openagent-http-runtime\"]",
        "CMD [\"--host\", \"0.0.0.0\", \"--port\", \"8787\", \"--headless\"]",
    ]
}

#[must_use]
pub fn docker_smoke_command() -> Vec<&'static str> {
    vec![
        "docker",
        "run",
        "--rm",
        "openagent-http-runtime:goal12",
        "--health-json",
    ]
}

#[must_use]
pub fn parse_cli_args(args: &[String]) -> (HttpRuntimeConfig, bool, bool) {
    let mut config = HttpRuntimeConfig::default();
    let mut health_json = false;
    let mut docker_smoke = false;
    let mut index = 0;
    while index < args.len() {
        match args[index].as_str() {
            "--host" => {
                if let Some(value) = args.get(index + 1) {
                    config.host = value.clone();
                    index += 1;
                }
            }
            "--port" => {
                if let Some(value) = args
                    .get(index + 1)
                    .and_then(|value| value.parse::<u16>().ok())
                {
                    config.port = value;
                    index += 1;
                }
            }
            "--workspace" => {
                if let Some(value) = args.get(index + 1) {
                    config.workspace = Some(value.clone());
                    index += 1;
                }
            }
            "--session-root" => {
                if let Some(value) = args.get(index + 1) {
                    config.session_store_root = Some(value.clone());
                    index += 1;
                }
            }
            "--headless" => {
                config.serve_static = false;
            }
            "--auth-token" => {
                if let Some(value) = args.get(index + 1) {
                    config.auth_token = Some(value.clone());
                    index += 1;
                }
            }
            "--username" | "-u" => {
                if let Some(value) = args.get(index + 1) {
                    config.auth_username = Some(value.clone());
                    index += 1;
                }
            }
            "--password" | "-p" => {
                if let Some(value) = args.get(index + 1) {
                    config.auth_password = Some(value.clone());
                    index += 1;
                }
            }
            "--cors-origin" => {
                if let Some(value) = args.get(index + 1) {
                    config.cors_origin = value.clone();
                    index += 1;
                }
            }
            "--mdns-name" => {
                if let Some(value) = args.get(index + 1) {
                    config.mdns_name = Some(value.clone());
                    index += 1;
                }
            }
            "--no-mdns" => {
                config.mdns_name = None;
            }
            "--health-json" => {
                health_json = true;
            }
            "--docker-smoke" => {
                docker_smoke = true;
            }
            _ => {}
        }
        index += 1;
    }
    (config, health_json, docker_smoke)
}

#[must_use]
pub fn run_cli(args: &[String]) -> CliRunResult {
    let (config, health_json, docker_smoke) = parse_cli_args(args);
    if args
        .iter()
        .any(|arg| matches!(arg.as_str(), "--help" | "-h"))
    {
        return CliRunResult {
            exit_code: 0,
            stdout: "Usage: openagent-http-runtime [--host <host>] [--port <port>] [--workspace <path>] [--session-root <path>] [--headless] [--auth-token <token>] [-u|--username <name>] [-p|--password <password>] [--cors-origin <origin>] [--mdns-name <name>] [--no-mdns] [--health-json]\n".to_string(),
            stderr: String::new(),
        };
    }
    if health_json || docker_smoke {
        let smoke_config = HttpRuntimeConfig {
            serve_static: false,
            auth_token: config.auth_token,
            ..HttpRuntimeConfig::default()
        };
        return CliRunResult {
            exit_code: 0,
            stdout: format!("{}\n", stable_json_dumps(&health_payload(&smoke_config))),
            stderr: String::new(),
        };
    }
    serve_blocking(config)
}

fn serve_blocking(config: HttpRuntimeConfig) -> CliRunResult {
    let listener = match TcpListener::bind((config.host.as_str(), config.port)) {
        Ok(listener) => listener,
        Err(error) => {
            return CliRunResult {
                exit_code: 1,
                stdout: String::new(),
                stderr: format!("failed to bind HTTP runtime: {error}\n"),
            };
        }
    };
    let local = listener
        .local_addr()
        .map(|addr| addr.to_string())
        .unwrap_or_else(|_| format!("{}:{}", config.host, config.port));
    println!("openagent HTTP runtime listening on http://{local}");
    start_background_task_worker(config.clone());
    for stream in listener.incoming() {
        match stream {
            Ok(mut stream) => {
                let config = config.clone();
                thread::spawn(move || {
                    let _ = handle_http_stream(&mut stream, &config);
                });
            }
            Err(error) => eprintln!("openagent HTTP runtime accept failed: {error}"),
        }
    }
    CliRunResult {
        exit_code: 0,
        stdout: String::new(),
        stderr: String::new(),
    }
}
