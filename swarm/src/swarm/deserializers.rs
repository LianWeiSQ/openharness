fn deserialize_runner_configs<'de, D>(deserializer: D) -> Result<Vec<RunnerConfig>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let value = Value::deserialize(deserializer)?;
    match value {
        Value::Array(items) => items
            .into_iter()
            .map(|item| {
                serde_json::from_value::<RunnerConfig>(item).map_err(serde::de::Error::custom)
            })
            .collect(),
        Value::Object(items) => items
            .into_iter()
            .map(|(id, value)| {
                let mut config = serde_json::from_value::<RunnerConfig>(value)
                    .map_err(serde::de::Error::custom)?;
                config.id = id;
                Ok(config)
            })
            .collect(),
        _ => Ok(Vec::new()),
    }
}

fn deserialize_task_configs<'de, D>(deserializer: D) -> Result<Vec<TaskConfig>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let value = Value::deserialize(deserializer)?;
    match value {
        Value::Array(items) => items
            .into_iter()
            .map(|item| {
                serde_json::from_value::<TaskConfig>(item).map_err(serde::de::Error::custom)
            })
            .collect(),
        Value::Object(items) => items
            .into_iter()
            .map(|(id, value)| {
                let mut config = serde_json::from_value::<TaskConfig>(value)
                    .map_err(serde::de::Error::custom)?;
                config.id = id;
                Ok(config)
            })
            .collect(),
        _ => Ok(Vec::new()),
    }
}
