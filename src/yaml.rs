use serde::de::DeserializeOwned;

use crate::error::{Error, Result};

/// Sole YAML reader. Error names file and line.
pub fn parse<T: DeserializeOwned>(file: &str, text: &str) -> Result<T> {
    serde_saphyr::from_str(text).map_err(|why| Error::Invalid(format!("{file}: {why}")))
}
