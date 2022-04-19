use clap::Parser;
use std::{env::VarError, error::Error};

use vrac::db;

#[derive(Parser, Debug)]
#[clap(author, version, about)]
struct Args {
    #[clap(short, long)]
    username: String,

    #[clap(short, long)]
    password: String,

    /// defaults to DATABASE_URL env variable if not provided
    #[clap(short, long)]
    database_url: Option<String>,
}

fn main() -> Result<(), Box<dyn Error>> {
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
    db::gen_user(&conn, args.username, args.password)?;
    Ok(())
}
