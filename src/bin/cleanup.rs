use clap::Parser;
use std::{env::VarError, error::Error, io::ErrorKind};

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
    let args = Args::parse();
    let db_url = match args.database_url {
        Some(x) => Ok(x),
        None => match std::env::var("DATABASE_URL") {
            Ok(x) => Ok(x),
            Err(VarError::NotPresent) => Err("DATABASE_URL env var not found"),
            Err(VarError::NotUnicode(_)) => Err("DATABASE_URL env var not valid unicode"),
        },
    }?;

    let conn = db::connect(db_url)?;
    let stuff_to_del = db::get_expired_files(&conn)?;
    let n_tok = stuff_to_del.len();
    for (token, files) in stuff_to_del {
        for file in files {
            println!("Removing file at {} with id {}", file.path, file.id);
            match std::fs::remove_file(&file.path) {
                Ok(_) => (),
                Err(err) => match err.kind() {
                    ErrorKind::NotFound => eprintln!(
                        "Attempted to delete file at {} but didn't find anything.",
                        file.path
                    ),
                    _ => {
                        eprintln!("Could not remove file it {}: {err:?}", file.path);
                        return Err(err.into());
                    }
                },
            }
        }
        let n = db::delete_files(&conn, &[token])?;
        println!("deleted a total of {n} files for {} tokens", n_tok);
    }

    Ok(())
}
