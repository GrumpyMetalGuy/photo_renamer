use std::fs::File;
use std::io::{Read, Write};
use std::path::Path;

use serde_derive::{Deserialize, Serialize};
use toml;

static CONFIG_FILENAME: &str = "renamer.toml";

#[derive(Serialize, Deserialize, Debug)]
pub struct RenamerConfig {
    /// Which paths will be searched for images and videos
    pub root_paths: Vec<String>,

    /// Where will non-RAW results be written out to?
    pub output_path: String,

    /// Where will non-RAW results be written out to?
    pub raw_output_path: String,

    /// Path fragments to exclude from processing
    pub exclusions: Vec<String>,
}

impl RenamerConfig {
    pub fn new() -> Self {
        RenamerConfig {
            root_paths: vec![String::from(".")],
            output_path: Path::new(".")
                .join("output")
                .into_os_string()
                .into_string()
                .unwrap(),
            raw_output_path: Path::new(".")
                .join("output_raw")
                .into_os_string()
                .into_string()
                .unwrap(),
            exclusions: vec![String::from("exclusions"), String::from("output")],
        }
    }

    pub fn read_or_create() -> Result<Option<Self>, std::io::Error> {
        let mut config_file = match File::open(CONFIG_FILENAME) {
            Ok(file) => file,
            Err(_) => {
                let default_config = RenamerConfig::new();
                let serialised = toml::to_string(&default_config).unwrap();

                let mut config_file = File::create(CONFIG_FILENAME)?;
                config_file.write(&serialised.as_bytes())?;

                println!("New config file {} created, please edit settings and re-run to begin renaming.", CONFIG_FILENAME);

                return Ok(None);
            }
        };

        let mut config_buffer = vec![];

        config_file.read_to_end(&mut config_buffer)?;

        let config_object: Self = toml::from_slice(&config_buffer)?;

        Ok(Some(config_object))
    }
}
