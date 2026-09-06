use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(version, about)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    #[command(about = "Create bare repo, slots, image for a project")]
    Init,
    #[command(about = "Run scheduler and workers until stopped")]
    Serve,
    #[command(about = "Print board rows with state derived from git")]
    Status,
    #[command(about = "Tail transcript of one task")]
    Log { id: String },
    #[command(about = "Drop branch of one task and of every dependant")]
    Reset { id: String },
}

fn main() {
    let cli = Cli::parse();
    match cli.command {
        Command::Init | Command::Serve | Command::Status => {}
        Command::Log { .. } | Command::Reset { .. } => {}
    }
}
