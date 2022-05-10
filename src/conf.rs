use figment::{
    providers::{Format, Toml},
    Figment,
};
use serde::Deserialize;
use std::path::PathBuf;

#[derive(Debug, Deserialize)]
#[serde(crate = "rocket::serde")]
pub struct VracConfig {
    pub root_path: PathBuf,
}

impl Default for VracConfig {
    fn default() -> Self {
        Self {
            root_path: std::env::current_dir().expect("Cannot access current dir???"),
        }
    }
}

impl VracConfig {
    pub fn from_rocket_config() -> Result<Self, figment::Error> {
        Figment::from(Toml::file("Rocket.toml")).extract()
    }
}
