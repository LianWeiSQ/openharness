use super::*;

pub(super) fn auth_command(command_name: &str, args: &[String]) -> CliRunResult {
    if args.is_empty() || args.iter().any(|arg| is_help_flag(arg)) {
        return ok_text(auth_help(command_name));
    }
    match args[0].as_str() {
        "login" => auth_login(&args[1..]),
        "list" | "ls" => auth_list(&args[1..]),
        "methods" => auth_methods(&args[1..]),
        "logout" => auth_logout(&args[1..]),
        _ => err_text(2, format!("unknown {command_name} command: {}", args[0])),
    }
}

fn auth_login(args: &[String]) -> CliRunResult {
    let provider = value_for(args, &["--provider", "-p"]).unwrap_or_else(|| "openai".to_string());
    let provider = match normalize_provider(Some(&provider)) {
        Ok(provider) => provider,
        Err(error) => return err_text(2, error),
    };
    let api_key = value_for(args, &["--api-key"]).unwrap_or_default();
    let auth_file = auth_file_from_args(args);
    let mut auth = read_json_file(&auth_file);
    let providers = ensure_object_field(&mut auth, "providers");
    let base_url = value_for(args, &["--base-url"])
        .or_else(|| provider_default_base_url(&provider).ok().flatten())
        .unwrap_or_else(|| DEFAULT_BASE_URL.to_string());
    let model = value_for(args, &["--model"])
        .or_else(|| provider_default_model(&provider).ok().flatten())
        .unwrap_or_else(|| DEFAULT_MODEL.to_string());
    let wire_api = value_for(args, &["--wire-api"]).unwrap_or_else(|| DEFAULT_WIRE_API.to_string());
    providers.insert(
        provider.clone(),
        json!({
            "provider": provider,
            "type": value_for(args, &["--type"]).unwrap_or_else(|| "api".to_string()),
            "api_key": api_key,
            "base_url": base_url,
            "model": model,
            "wire_api": wire_api,
            "updated_at_ms": now_ms(),
        }),
    );
    let record = public_auth_record(
        &provider,
        providers.get(&provider).unwrap_or(&Value::Null),
        "auth_file",
    );
    if let Err(error) = write_json_file(&auth_file, &auth) {
        return err_text(1, error);
    }
    chmod_private(&auth_file);
    CliRunResult::ok_json(&json!({
        "status": "logged_in",
        "provider": provider,
        "auth_file": auth_file.to_string_lossy(),
        "record": record,
    }))
}

fn auth_list(args: &[String]) -> CliRunResult {
    let auth_file = auth_file_from_args(args);
    let auth = read_json_file(&auth_file);
    let providers = auth
        .get("providers")
        .and_then(Value::as_object)
        .map(|items| {
            items
                .iter()
                .map(|(provider, value)| public_auth_record(provider, value, "auth_file"))
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    let payload = json!({"auth_file": auth_file.to_string_lossy(), "providers": providers});
    if value_for(args, &["--format"]).as_deref() == Some("json") {
        CliRunResult::ok_json(&payload)
    } else {
        ok_text(format!(
            "{} provider(s)",
            payload["providers"].as_array().map_or(0, Vec::len)
        ))
    }
}

fn auth_methods(args: &[String]) -> CliRunResult {
    let provider = positional_args(args, &["--format"])
        .first()
        .cloned()
        .unwrap_or_else(active_provider);
    let present_env = env::vars().map(|(key, _)| key).collect::<BTreeSet<_>>();
    match provider_auth_methods(&provider, &present_env) {
        Ok(methods) => {
            let normalized = normalize_provider(Some(&provider)).unwrap_or(provider);
            let payload = json!({"provider": normalized, "methods": methods});
            if value_for(args, &["--format"]).as_deref() == Some("json") {
                CliRunResult::ok_json(&payload)
            } else {
                ok_text(format!(
                    "{} auth method(s)",
                    payload["methods"].as_array().map_or(0, Vec::len)
                ))
            }
        }
        Err(error) => err_text(2, error),
    }
}

fn auth_logout(args: &[String]) -> CliRunResult {
    let provider = value_for(args, &["--provider", "-p"]).unwrap_or_else(|| "openai".to_string());
    let provider = match normalize_provider(Some(&provider)) {
        Ok(provider) => provider,
        Err(error) => return err_text(2, error),
    };
    let auth_file = auth_file_from_args(args);
    let mut auth = read_json_file(&auth_file);
    let removed = auth
        .get_mut("providers")
        .and_then(Value::as_object_mut)
        .and_then(|providers| providers.remove(&provider))
        .is_some();
    if let Err(error) = write_json_file(&auth_file, &auth) {
        return err_text(1, error);
    }
    CliRunResult::ok_json(
        &json!({"provider": provider, "removed": removed, "auth_file": auth_file.to_string_lossy()}),
    )
}
