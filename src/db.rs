use chrono::naive::{NaiveDate, NaiveDateTime, NaiveTime};
use rusqlite::Connection;
use std::error::Error;
use std::fmt;
use std::path::Path;

use crate::errors;

#[derive(Debug)]
pub struct Token {
    id: u32,
    path: String,
    status: TokenStatus,
    max_size_in_bytes: Option<i32>,
    created_at: NaiveDateTime,
    expires_at: Option<NaiveDateTime>,
    deleted_at: Option<NaiveDateTime>,
}

pub struct CreateToken {
    pub path: String,
    pub max_size_in_bytes: Option<i32>,
    pub expires_at: Option<NaiveDateTime>,
}

#[derive(Debug)]
pub enum TokenStatus {
    Fresh,
    Used,
    Expired,
    Deleted,
}

impl rusqlite::ToSql for TokenStatus {
    fn to_sql(&self) -> rusqlite::Result<rusqlite::types::ToSqlOutput<'_>> {
        let str = match self {
            TokenStatus::Fresh => "fresh",
            TokenStatus::Used => "used",
            TokenStatus::Expired => "expired",
            TokenStatus::Deleted => "deleted",
        };
        Ok(str.into())
    }
}

#[derive(Debug)]
pub struct TokenStatusError {
    details: String,
}

impl fmt::Display for TokenStatusError {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "{}", self.details)
    }
}

impl Error for TokenStatusError {
    fn description(&self) -> &str {
        &self.details
    }
}

impl rusqlite::types::FromSql for TokenStatus {
    fn column_result(value: rusqlite::types::ValueRef) -> rusqlite::types::FromSqlResult<Self> {
        match String::column_result(value)?.as_str() {
            "fresh" => Ok(TokenStatus::Fresh),
            "used" => Ok(TokenStatus::Used),
            "expired" => Ok(TokenStatus::Expired),
            "deleted" => Ok(TokenStatus::Deleted),
            other => {
                let err = TokenStatusError {
                    details: other.to_string(),
                };
                Err(rusqlite::types::FromSqlError::Other(Box::new(err)))
            }
        }
    }
}

pub trait VracPersistence {
    fn init_db(&self) -> Result<(), errors::Error>;
    fn create_token(&self, token: &CreateToken) -> Result<Token, errors::Error>;
    fn get_token_by_id(&self, token_id: u32) -> Result<Option<Token>, errors::Error>;
    fn get_valid_token_by_path(&self, path: String) -> Result<Option<Token>, errors::Error>;
}

#[derive(Debug, Clone)]
pub struct DB {
    db_path: String
}

impl DB {
    pub fn new(path_str: &str) -> Self {
        DB {
            db_path: path_str.to_string()
        }
    }

    fn open(&self) -> rusqlite::Result<Connection> {
        Connection::open(Path::new(&self.db_path))
    }
}

impl VracPersistence for DB {

    fn init_db(&self) -> Result<(), errors::Error> {
        let conn = self.open()?;
        conn.execute(
            "CREATE TABLE IF NOT EXISTS token (
                id INTEGER PRIMARY KEY,
                path TEXT NOT NULL,
                status TEXT NOT NULL,
                max_size INTEGER,
                created_at DATETIME NOT NULL DEFAULT (datetime('now')),
                expires_at DATETIME,
                deleted_at DATETIME
                )",
            rusqlite::params![],
        )?;

        conn.execute("PRAGMA encoding='UTF-8'", rusqlite::params![])?;
        conn.execute("PRAGMA foreign_key = ON;", rusqlite::params![])?;
        Ok(())
    }

    fn create_token(&self, tok: &CreateToken) -> Result<Token, errors::Error> {
        let id;
        {
            let conn = self.open()?;
            let n = conn.execute_named(
                "INSERT INTO token (path, status, max_size, expires_at) SELECT :path, :status, :max_size, :expires_at WHERE NOT EXISTS(SELECT 1 FROM token where path = :path AND (status = :status_fresh OR status = :status_used) AND deleted_at IS NULL)",
                &[  (":path", &tok.path),
                    (":status", &TokenStatus::Fresh),
                    (":max_size", &tok.max_size_in_bytes),
                    (":expires_at", &tok.expires_at),
                    (":status_fresh", &TokenStatus::Fresh),
                    (":status_used", &TokenStatus::Used),
                ],
            )?;

            if n == 0 {
                return Err(errors::VracError::DuplicateToken(tok.path.clone()).into());
            }

            let mut stmt = conn.prepare("SELECT last_insert_rowid()")?;
            id = stmt.query(rusqlite::params![])?.next()?.unwrap().get(0)?;
        }
        let token = self.get_token_by_id(id)?;
        // safe to unwrap since the id we got is guaranteed to be in the table
        // (and there is no DELETE)
        Ok(token.unwrap())
    }

    fn get_token_by_id(&self, token_id: u32) -> Result<Option<Token>, errors::Error> {
        let conn = self.open()?;
        let mut stmt = conn.prepare("SELECT id, path, status, max_size, created_at, expires_at, deleted_at FROM token WHERE id = ?1 AND deleted_at IS NULL")?;
        let mut result_iter = stmt.query_map(rusqlite::params![token_id], token_from_row)?;

        match result_iter.next() {
            None => Ok(None),
            Some(r) => {
                let result = r?;
                Ok(Some(result))
            }
        }
    }

    fn get_valid_token_by_path(&self, token_path: String) -> Result<Option<Token>, errors::Error> {
        let conn = self.open()?;
        let mut stmt = conn.prepare("SELECT id, path, status, max_size, created_at, expires_at, deleted_at FROM token WHERE path = ?1 AND (status = ?2 OR status = ?3) AND deleted_at IS NULL")?;
        let mut result_iter = stmt.query_map(rusqlite::params![token_path, TokenStatus::Fresh, TokenStatus::Used], token_from_row)?;

        match result_iter.next() {
            None => Ok(None),
            Some(r) => {
                let result = r?;
                Ok(Some(result))
            }
        }
    }
}

fn token_from_row(row: &rusqlite::Row) -> rusqlite::Result<Token> {
    Ok(Token {
        id: row.get(0)?,
        path: row.get(1)?,
        status: row.get(2)?,
        max_size_in_bytes: row.get(3)?,
        created_at: row.get(4)?,
        expires_at: row.get(5)?,
        deleted_at: row.get(6)?,
    })
}
