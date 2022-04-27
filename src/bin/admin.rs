use clap::Parser;
use std::{env::VarError, error::Error};

use vrac::cleanup;
use vrac::db;

type AdminResult<R> = std::result::Result<R, Box<dyn Error>>;

/// Utility binary to manage the users, files and other useful stuff like that.
#[derive(Debug, Parser)]
#[clap(version, author, about)]
struct Opts {
    /// defaults to DATABASE_URL env variable if not provided
    #[clap(short, long)]
    database_url: Option<String>,

    #[clap(subcommand)]
    cmd: SubCommand,
}

#[derive(Debug, Parser)]
enum SubCommand {
    /// Force a cleanup of expired files and tokens
    Cleanup,

    /// Create a user with the given username/password
    GenUser {
        #[clap(short, long)]
        username: String,

        #[clap(short, long)]
        password: String,
    },

    /// Delete the corresponding token and its associated files,
    /// regardless of their validity/expiration date.
    Delete {
        #[clap(short, long)]
        token: String,
    },
}

/// remove files associated with expired tokens, and
/// cleanup the DB afterward as well
fn main() -> AdminResult<()> {
    env_logger::init();
    let opts = Opts::parse();
    let database_url = opts.database_url;

    match Opts::parse().cmd {
        SubCommand::Cleanup => cleanup(database_url),
        SubCommand::GenUser { username, password } => gen_user(database_url, username, password),
        SubCommand::Delete { token } => delete_token(database_url, token),
    }
}

fn cleanup(database_url: Option<String>) -> AdminResult<()> {
    let db_url = get_db_url(database_url)?;
    let conn = db::connect(&db_url)?;
    cleanup::cleanup_once(&conn)?;
    Ok(())
}

fn gen_user(database_url: Option<String>, username: String, password: String) -> AdminResult<()> {
    let db_url = get_db_url(database_url)?;
    let conn = db::connect(&db_url)?;
    db::gen_user(&conn, username, password)?;
    Ok(())
}

fn delete_token(database_url: Option<String>, token_path: String) -> AdminResult<()> {
    let conn = db::connect(&get_db_url(database_url)?)?;
    match db::get_valid_token(&conn, &token_path)? {
        Some(tok) => {
            let n = db::delete_files(&conn, &[tok.id])?;
            db::delete_token(&conn, tok.id)?;
            cleanup::remove_token_dir(&tok.path)?;
            log::info!("Deleted {n} files for token at path {}", tok.path);
        }
        None => {
            log::info!("No token found at path {token_path}");
            cleanup::remove_token_dir(&token_path)?;
            log::info!("Removed everything under {token_path}");
        },
    };
    Ok(())
}

fn get_db_url(database_url: Option<String>) -> AdminResult<String> {
    match database_url {
        Some(x) => Ok(x),
        None => match std::env::var("DATABASE_URL") {
            Ok(x) => Ok(x),
            Err(VarError::NotPresent) => Err("DATABASE_URL env var not found".into()),
            Err(VarError::NotUnicode(_)) => Err("DATABASE_URL env var not valid unicode".into()),
        },
    }
}
