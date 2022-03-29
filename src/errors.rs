use std::io::Cursor;

use diesel;
use multer;
use rocket::http::{ContentType, Status};
use rocket::response;
use thiserror::Error;

pub type Result<T> = std::result::Result<T, VracError>;

#[derive(Error, Debug)]
pub enum VracError {
    #[error("database error {0:?}")]
    DbError(#[from] diesel::result::Error),

    #[error("multipart decoding error {0:?}")]
    MultipartError(#[from] multer::Error),

    #[error("IO error")]
    IoError(#[from] std::io::Error),

    #[error("Token already exists: {0}")]
    TokenAlreadyExists(String),

    #[error("User already exists: {0}")]
    UserAlreadyExists(String),

    #[error(transparent)]
    Other(#[from] anyhow::Error),
}

impl<'r> response::Responder<'r, 'static> for VracError {
    fn respond_to(self, _: &'r rocket::Request<'_>) -> response::Result<'static> {
        let (err_str, status) = match &self {
            VracError::TokenAlreadyExists(tok) => {
                let err_str = format!("Token already exists for path {}", tok);
                (err_str, Status::BadRequest)
            },
            _ => {
                log::error!("got a generic error! {:?}", self);
                (format!("{:#?}", self), Status::InternalServerError)
            }
        };
        response::Response::build()
            .sized_body(err_str.len(), Cursor::new(err_str))
            .status(status)
            .header(ContentType::Text)
            .ok()
    }
}
