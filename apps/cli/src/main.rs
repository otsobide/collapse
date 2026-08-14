use clap::Parser;
use collapse_cli::{run, Cli};

fn main() {
    let cli = Cli::parse();
    match run(cli) {
        Ok(outcome) => outcome.report(),
        Err(err) => {
            eprintln!("error: {err}");
            std::process::exit(1);
        }
    }
}
