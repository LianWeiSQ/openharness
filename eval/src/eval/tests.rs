#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn links_to_core_crate() {
        assert_eq!(crate_name(), "openagent-eval");
        assert_eq!(core_crate_name(), "openagent-core");
    }

    #[test]
    fn terminal_command_quoting_matches_legacy_shape() {
        assert_eq!(
            terminal_bench_wrap_command(
                "printf 'hello world'",
                Some("/app/project"),
                "__OPENAGENT_TBENCH_EXIT_fixture__",
            ),
            "bash -lc 'set +e\ncd /app/project\n(\nprintf '\"'\"'hello world'\"'\"'\n)\nstatus=$?\nprintf '\"'\"'\\n__OPENAGENT_TBENCH_EXIT_fixture__%s\\n'\"'\"' \"$status\"'"
        );
    }
}
