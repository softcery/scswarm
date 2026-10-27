use std::fs::File;
use std::io::{self, Write};
use std::path::Path;

use clap::Command;
use clap_mangen::roff::{roman, Roff};
use clap_mangen::Man;

use crate::help;

/// Write one page per visible command into `dir`: clap's sections, then the
/// help sections as man sections with their layout kept.
pub fn write(command: Command, dir: &Path) -> io::Result<()> {
    let mut command = command.disable_help_subcommand(true);
    command.build();
    walk(&command, dir)
}

fn walk(command: &Command, dir: &Path) -> io::Result<()> {
    for sub in command.get_subcommands().filter(|sub| !sub.is_hide_set()) {
        walk(sub, dir)?;
    }
    let man = Man::new(command.clone());
    let mut out = File::create(dir.join(man.get_filename()))?;
    man.render_title(&mut out)?;
    man.render_name_section(&mut out)?;
    man.render_synopsis_section(&mut out)?;
    man.render_description_section(&mut out)?;
    if command.get_arguments().any(|arg| !arg.is_hide_set()) {
        man.render_options_section(&mut out)?;
    }
    if command.get_subcommands().any(|sub| !sub.is_hide_set()) {
        man.render_subcommands_section(&mut out)?;
    }
    sections(&help::sections(help::after(command.get_name()))).to_writer(&mut out)?;
    if command.get_version().is_some() {
        man.render_version_section(&mut out)?;
    }
    out.flush()
}

/// No-fill, so examples and tables keep their lines.
fn sections(sections: &[help::Section]) -> Roff {
    let mut roff = Roff::default();
    for section in sections {
        roff.control("SH", [section.title.to_uppercase().as_str()]);
        roff.control("nf", []);
        for line in &section.body {
            roff.text([roman(line.strip_prefix("  ").unwrap_or(line))]);
        }
        roff.control("fi", []);
    }
    roff
}
