use rusqlite;
use warp;

// pub type Foo<T, E = Error> = Result<T, E>

#[derive(Debug)]
pub enum Error {
    SqliteError(rusqlite::Error),
    AppError(VracError),
}


#[derive(Debug)]
pub enum VracError {
    DuplicateToken(String)
}

impl From<rusqlite::Error> for Error {
    fn from(err: rusqlite::Error) -> Self {
        Error::SqliteError(err)
    }
}

impl From<VracError> for Error {
    fn from(err: VracError) -> Self {
        Error::AppError(err)
    }
}

impl warp::reject::Reject for Error {}

impl From<Error> for warp::reject::Rejection {
    fn from(err: Error) -> Self {
        warp::reject::custom(err)
    }
}
