use clap::Parser;
use std::{env::VarError, error::Error};

use vrac::cleanup;
use vrac::db;

/// Utility binary to manage the users, files and other useful stuff like that.
#[derive(Debug, Parser)]
#[clap(version, author, about)]
struct Opts {
    #[clap(subcommand)]
    cmd: SubCommand,
}

#[derive(Debug, Parser)]
enum SubCommand {
    /// Force a cleanup of expired files and tokens
    Cleanup {
        /// defaults to DATABASE_URL env variable if not provided
        #[clap(short, long)]
        database_url: Option<String>,
    },
    GenUser {
        #[clap(short, long)]
        username: String,

        #[clap(short, long)]
        password: String,

        /// defaults to DATABASE_URL env variable if not provided
        #[clap(short, long)]
        database_url: Option<String>,
    },
}

/// remove files associated with expired tokens, and
/// cleanup the DB afterward as well
fn main() -> Result<(), Box<dyn Error>> {
    env_logger::init();

    match Opts::parse().cmd {
        SubCommand::Cleanup { database_url } => cleanup(database_url),
        SubCommand::GenUser {
            username,
            password,
            database_url,
        } => gen_user(database_url, username, password),
    }
}

fn cleanup(database_url: Option<String>) -> Result<(), Box<dyn Error>> {
    let db_url = get_db_url(database_url)?;
    let conn = db::connect(&db_url)?;
    cleanup::cleanup_once(&conn)?;
    Ok(())
}

fn gen_user(
    database_url: Option<String>,
    username: String,
    password: String,
) -> Result<(), Box<dyn Error>> {
    let db_url = get_db_url(database_url)?;
    let conn = db::connect(&db_url)?;
    db::gen_user(&conn, username, password)?;
    Ok(())
}

fn get_db_url(database_url: Option<String>) -> Result<String, Box<dyn Error>> {
    match database_url {
        Some(x) => Ok(x),
        None => match std::env::var("DATABASE_URL") {
            Ok(x) => Ok(x),
            Err(VarError::NotPresent) => Err("DATABASE_URL env var not found".into()),
            Err(VarError::NotUnicode(_)) => Err("DATABASE_URL env var not valid unicode".into()),
        },
    }
}
