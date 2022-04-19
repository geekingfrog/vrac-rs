use std::{error::Error, io::ErrorKind};

use diesel::SqliteConnection;

use crate::db;

pub async fn cleanup(db_url: &str) -> Result<(), Box<dyn Error>> {
    log::info!("Starting cleanup job");

    loop {
        let db_url = db_url.to_string();
        tokio::task::spawn_blocking(move || {
            let conn = db::connect(&db_url).unwrap();
            // somehow, the big enum error in crate::error using thiserror doesn't
            // get Send, so attempting to convert with `?` fails here.
            // So instead, manually log the error and returns a stringified error
            match cleanup_once(&conn) {
                Ok(_) => Ok(()),
                Err(err) => {
                    log::error!("Cleanup job errored out with {err:?}");
                    Err(format!("{err:?}"))
                }
            }
        })
        .await??;

        tokio::time::sleep(std::time::Duration::from_secs(60 * 10)).await;
    }
}

/// checks the DB for expired tokens and remove the associated files, then
/// delete the tokens.
pub fn cleanup_once(conn: &SqliteConnection) -> Result<(), Box<dyn Error>> {
    log::debug!("cleaning up files");
    let stuff_to_del = db::get_expired_files(conn)?;
    let n_tok = stuff_to_del.len();
    for (token, files) in stuff_to_del {
        for file in files {
            log::info!("Removing file at {} with id {}", file.path, file.id);
            match std::fs::remove_file(&file.path) {
                Ok(_) => (),
                Err(err) => match err.kind() {
                    ErrorKind::NotFound => log::error!(
                        "Attempted to delete file at {} but didn't find anything.",
                        file.path
                    ),
                    _ => {
                        log::error!("Could not remove file it {}: {err:?}", file.path);
                        return Err(err.into());
                    }
                },
            }
        }
        let n = db::delete_files(conn, &[token])?;
        log::info!("deleted a total of {n} files for {} tokens", n_tok);
    }

    Ok(())
}
