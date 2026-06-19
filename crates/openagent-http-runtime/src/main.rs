fn main() {
    let argv = std::env::args().skip(1).collect::<Vec<_>>();
    let result = openagent_http_runtime::run_cli(&argv);
    print!("{}", result.stdout);
    eprint!("{}", result.stderr);
    std::process::exit(result.exit_code);
}
