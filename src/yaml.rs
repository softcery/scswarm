use serde::de::DeserializeOwned;

use crate::error::{Error, Result};

/// Sole YAML reader. Error names file and line.
pub fn parse<T: DeserializeOwned>(file: &str, text: &str) -> Result<T> {
    serde_saphyr::from_str_with_options(text, options())
        .map_err(|why| Error::Invalid(format!("{file}: {why}")))
}

/// Boards alias shared steps per row.
fn options() -> serde_saphyr::Options {
    serde_saphyr::options! {
        budget: serde_saphyr::budget! { enforce_alias_anchor_ratio: false },
    }
}
