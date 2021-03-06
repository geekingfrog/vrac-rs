use std::{error::Error, io::ErrorKind};

use diesel::SqliteConnection;

use crate::db;

/// checks the DB for expired tokens and remove the associated files, then
/// delete the tokens.
pub fn cleanup_once(conn: &SqliteConnection) -> Result<(), Box<dyn Error>> {
    log::debug!("cleaning up files");
    let stuff_to_del = db::get_expired_files(conn)?;
    let n_tok = stuff_to_del.len();
    let mut n = 0;
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
        n += db::delete_files(conn, &[token])?;
    }
    log::info!("deleted a total of {n} files for {} tokens", n_tok);

    let del_token_paths = db::delete_expired_tokens(conn)?;
    for path in &del_token_paths {
        match std::fs::remove_dir_all(&path) {
            Ok(_) => (),
            // if for some reason, the directory isn't there, ignore the error
            Err(err) if err.kind() == ErrorKind::NotFound => (),
            Err(err) => return Err(err.into()),
        }
    }

    log::info!(
        "Marked {} tokens as deleted for paths: {:?}",
        del_token_paths.len(),
        del_token_paths
    );

    Ok(())
}
