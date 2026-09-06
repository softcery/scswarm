mod board;
mod config;
mod error;
mod git;
mod paths;
mod serve;
mod slot;
mod stage;
mod state;
mod turn;

use clap::{Parser, Subcommand};

use crate::board::{Board, Row};
use crate::config::Config;
use crate::error::Result;
use crate::git::Git;
use crate::state::State;

const HEADER: [&str; 5] = ["id", "state", "branch", "stage", "worker"];
const GAP: &str = "  ";
const NONE: &str = "-";

#[derive(Parser)]
#[command(version, about)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    #[command(about = "Run scheduler and worker until stopped")]
    Serve,
    #[command(about = "Print board rows with state derived from git")]
    Status,
}

fn main() {
    let cli = Cli::parse();
    if let Err(why) = dispatch(cli.command) {
        eprintln!("{why}");
        std::process::exit(why.code());
    }
}

fn dispatch(command: Command) -> Result<()> {
    match command {
        Command::Serve => serve::serve(&Git::discover()?),
        Command::Status => status(),
    }
}

fn status() -> Result<()> {
    let git = Git::discover()?;
    let config = Config::from_main(&git)?;
    let board = Board::from_main(&git, &config)?;
    let total = config.stages.len();
    let states = state::derive(&board, total, &git)?;
    print!("{}", render(&board, &states, total));
    Ok(())
}

fn render(board: &Board, states: &[State], total: usize) -> String {
    let mut table = vec![HEADER.map(str::to_string)];
    table.extend(board.rows().iter().zip(states).map(|(row, state)| {
        [
            row.id().to_string(),
            state.to_string(),
            branch_cell(row, *state),
            stage_cell(*state, total),
            NONE.to_string(),
        ]
    }));
    let widths = column_widths(&table);
    table.iter().map(|row| line(row, &widths)).collect()
}

fn branch_cell(row: &Row, state: State) -> String {
    match state {
        State::Ready | State::Blocked => NONE.to_string(),
        _ => state::branch(row.id()),
    }
}

fn stage_cell(state: State, total: usize) -> String {
    match state {
        State::Running { stage } => format!("{stage}/{total}"),
        State::Landed | State::Done => format!("{total}/{total}"),
        State::Ready | State::Blocked => NONE.to_string(),
    }
}

fn column_widths(table: &[[String; 5]]) -> [usize; 5] {
    let mut widths = [0; 5];
    for row in table {
        for (width, cell) in widths.iter_mut().zip(row) {
            *width = (*width).max(cell.chars().count());
        }
    }
    widths
}

fn line(row: &[String; 5], widths: &[usize; 5]) -> String {
    let last = row.len() - 1;
    let cells: Vec<String> = row
        .iter()
        .zip(widths)
        .enumerate()
        .map(|(index, (cell, &width))| {
            if index == last {
                cell.clone()
            } else {
                format!("{cell:<width$}")
            }
        })
        .collect();
    format!("{}\n", cells.join(GAP))
}
