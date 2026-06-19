fn main() {
    let argv = std::env::args().skip(1).collect::<Vec<_>>();
    let result = openagent_cli::run_cli_command(&argv);
    print!("{}", result.stdout);
    eprint!("{}", result.stderr);
    std::process::exit(result.exit_code);
}
