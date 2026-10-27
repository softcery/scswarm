pub const VERSION: &str = env!("SCSWARM_VERSION");
pub const ROOT_NAME: &str = env!("CARGO_BIN_NAME");

pub const ROOT_ABOUT: &str = include_str!("help/scswarm.txt");
pub const ROOT_EXAMPLES: &str = include_str!("help/scswarm_examples.txt");
pub const SERVE_ABOUT: &str = include_str!("help/serve.txt");
pub const STATUS_ABOUT: &str = include_str!("help/status.txt");
pub const LOG_ABOUT: &str = include_str!("help/log.txt");
pub const RESET_ABOUT: &str = include_str!("help/reset.txt");

/// Titled block after options: `Title:` in `--help`, uppercase man section.
pub struct Section<'a> {
    pub title: &'a str,
    pub body: Vec<&'a str>,
}

/// Text after options, by clap command name.
pub fn after(command: &str) -> &'static str {
    match command {
        ROOT_NAME => concat!(
            include_str!("help/queue.txt"),
            "\nSWARM.yaml:\n",
            include_str!("help/track.yaml"),
            include_str!("help/track.txt"),
            "\nscswarm.yaml:\n",
            include_str!("help/config.yaml"),
            include_str!("help/config.txt"),
            "\n",
            include_str!("help/scswarm_after.txt"),
        ),
        "serve" => concat!(
            "Host file:\n",
            include_str!("help/host.yaml"),
            include_str!("help/serve_after.txt"),
        ),
        "status" => include_str!("help/status_after.txt"),
        "log" => include_str!("help/log_after.txt"),
        "reset" => include_str!("help/reset_after.txt"),
        "completions" => include_str!("help/completions_after.txt"),
        "man" => include_str!("help/man_after.txt"),
        _ => "",
    }
}

/// Set `after` of `command` and of each subcommand as its `--help` tail.
pub fn attach(command: clap::Command) -> clap::Command {
    let names: Vec<String> = command
        .get_subcommands()
        .map(|sub| sub.get_name().to_string())
        .collect();
    let mut command = command.after_long_help(after(ROOT_NAME).trim_end());
    for name in names {
        command = command.mut_subcommand(&name, |sub| sub.after_long_help(after(&name).trim_end()));
    }
    command
}

/// Split `after` text at its headings: unindented lines that end in `:`.
pub fn sections(text: &str) -> Vec<Section<'_>> {
    let mut sections: Vec<Section> = Vec::new();
    for line in text.trim_end().lines() {
        match (line.strip_suffix(':'), sections.last_mut()) {
            (Some(title), _) if !line.starts_with(' ') => sections.push(Section {
                title,
                body: Vec::new(),
            }),
            (_, Some(section)) => section.body.push(line),
            (_, None) => {}
        }
    }
    for section in &mut sections {
        while section.body.last().is_some_and(|line| line.is_empty()) {
            section.body.pop();
        }
    }
    sections
}

#[cfg(test)]
mod tests {
    use crate::board::Track;
    use crate::config::Config;

    /// Users copy these examples, so the parser must take them as printed.
    #[test]
    fn examples_parse() {
        let config =
            Config::parse(include_str!("help/config.yaml")).expect("help scswarm.yaml must parse");
        let tip = "0123456789abcdef0123456789abcdef01234567";
        let track = Track::parse("laser", tip, include_str!("help/track.yaml"))
            .expect("help SWARM.yaml must parse");
        track.board(&config).expect("help rows must settle");
    }
}
