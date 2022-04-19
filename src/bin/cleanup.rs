use clap::Parser;
use std::{env::VarError, error::Error};

use vrac::cleanup;
use vrac::db;

#[derive(Parser, Debug)]
#[clap(author, version, about)]
struct Args {
    /// defaults to DATABASE_URL env variable if not provided
    #[clap(short, long)]
    database_url: Option<String>,
}

/// remove files associated with expired tokens, and
/// cleanup the DB afterward as well
fn main() -> Result<(), Box<dyn Error>> {
    env_logger::init();

    let args = Args::parse();
    let db_url = match args.database_url {
        Some(x) => Ok(x),
        None => match std::env::var("DATABASE_URL") {
            Ok(x) => Ok(x),
            Err(VarError::NotPresent) => Err("DATABASE_URL env var not found"),
            Err(VarError::NotUnicode(_)) => Err("DATABASE_URL env var not valid unicode"),
        },
    }?;

    let conn = db::connect(&db_url)?;
    cleanup::cleanup_once(&conn)?;

    Ok(())
}
