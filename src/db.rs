use anyhow::{Context, Result};
use chrono::naive::NaiveDateTime;
use chrono::Utc;
// use std::fmt;
use tokio::sync::{mpsc, oneshot};

use diesel::{
    backend::Backend, deserialize::FromSql, prelude::*, serialize::ToSql, sql_types,
    sql_types::Text, Connection, Insertable, Queryable, SqliteConnection,
};

use crate::schema::token::{self, dsl};

diesel_migrations::embed_migrations!("./migrations/");

#[derive(Debug, Queryable)]
pub struct Token {
    pub id: i32,
    pub path: String,
    pub status: TokenStatus,
    pub max_size_in_mb: Option<i32>,
    pub created_at: NaiveDateTime,
    pub token_expires_at: NaiveDateTime,
    pub content_expires_at: Option<NaiveDateTime>,
    pub deleted_at: Option<NaiveDateTime>,
}

#[derive(Debug)]
pub struct CreateToken {
    pub path: String,
    pub max_size_in_mb: Option<u32>,
    pub token_expires_at: NaiveDateTime,
    pub content_expires_at: Option<NaiveDateTime>,
}

#[derive(Debug, Insertable)]
#[table_name = "token"]
struct CreateTokenSQLite {
    path: String,
    status: TokenStatus,
    max_size: Option<i32>,
    created_at: NaiveDateTime,
    token_expires_at: NaiveDateTime,
    content_expires_at: Option<NaiveDateTime>,
    deleted_at: Option<NaiveDateTime>,
}

#[derive(Debug, FromSqlRow, AsExpression, Clone, Copy)]
#[sql_type = "Text"]
pub enum TokenStatus {
    Fresh,
    Used,
    Expired,
    Deleted,
}

impl<DB> FromSql<sql_types::Text, DB> for TokenStatus
where
    DB: Backend,
    String: FromSql<sql_types::Text, DB>,
{
    fn from_sql(bytes: Option<&DB::RawValue>) -> diesel::deserialize::Result<Self> {
        match &(String::from_sql(bytes)?)[..] {
            "FRESH" => Ok(TokenStatus::Fresh),
            "USED" => Ok(TokenStatus::Used),
            "EXPIRED" => Ok(TokenStatus::Expired),
            "DELETED" => Ok(TokenStatus::Deleted),
            x => Err(format!("Unknown token status: {}", x).into()),
        }
    }
}

impl<DB> ToSql<sql_types::Text, DB> for TokenStatus
where
    DB: Backend,
{
    fn to_sql<W: std::io::Write>(
        &self,
        out: &mut diesel::serialize::Output<W, DB>,
    ) -> diesel::serialize::Result {
        let tag = match self {
            TokenStatus::Fresh => "FRESH",
            TokenStatus::Used => "USED",
            TokenStatus::Expired => "EXPIRED",
            TokenStatus::Deleted => "DELETED",
        };
        ToSql::<sql_types::Text, DB>::to_sql(tag, out)
    }
}

#[derive(Debug)]
pub struct TokenStatusError {
    details: String,
}

impl std::fmt::Display for TokenStatusError {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        write!(f, "{}", self.details)
    }
}

pub trait VracPersistence {
    fn init_db(&self) -> Result<()>;
    fn create_token(&self, token: CreateToken) -> Result<Token>;
    // fn get_token_by_id(&self, token_id: u32) -> Result<Option<Token>>;
    // fn get_valid_token_by_path(&self, path: &str) -> Result<Option<Token>>;
}

#[derive(Debug, Clone)]
pub struct DB {
    pub db_path: String,
}

impl DB {
    pub fn new(path_str: &str) -> Self {
        DB {
            db_path: path_str.to_string(),
        }
    }

    fn open(&self) -> Result<SqliteConnection> {
        SqliteConnection::establish(&self.db_path)
            .context(format!("Cannot connect to db at {}", &self.db_path))
    }
}

diesel::no_arg_sql_function!(last_insert_rowid, sql_types::Integer);
// let last_row_id = diesel::select(last_insert_rowid).get_result::<i32>(&conn)?;

impl VracPersistence for DB {
    fn init_db(&self) -> Result<()> {
        let conn = self.open()?;
        embedded_migrations::run_with_output(&conn, &mut std::io::stdout())?;
        Ok(())
    }
    fn create_token(&self, tok: CreateToken) -> Result<Token> {
        let conn = self.open()?;

        // let existing_token = dsl::token
        //     .filter(dsl::status.eq_any(vec![TokenStatus::Fresh, TokenStatus::Used]))
        //     .filter(dsl::path.eq(&tok.path))
        //     .select(diesel::dsl::count_star())
        //     .first(&conn)?;

        let existing_count: i64 = token::table
            .select(diesel::dsl::count_star())
            .first(&conn)?;
        if existing_count > 0 {
            bail!("A valid token already exists for this path")
        };

        let sql_tok = CreateTokenSQLite {
            path: tok.path,
            status: TokenStatus::Fresh,
            max_size: tok.max_size_in_mb.map(|s| s as _),
            created_at: Utc::now().naive_utc(),
            token_expires_at: tok.token_expires_at,
            content_expires_at: tok.content_expires_at,
            deleted_at: None,
        };

        conn.transaction::<_, anyhow::Error, _>(|| {
            let n_inserted = diesel::insert_into(token::table)
                .values(&sql_tok)
                .execute(&conn)
                .with_context(|| format!("Cannot insert {:?} into token table", &sql_tok))?;

            println!("inserted returned: {:#?}", n_inserted);
            if n_inserted == 0 {
                Err(anyhow!("Didn't insert token: {:?}", sql_tok))
            } else {
                let inserted_token = token::table.order(token::id.desc()).first(&conn)?;
                Ok(inserted_token)
            }
        })
    }
}

type Responder<T> = oneshot::Sender<Result<T>>;

#[derive(Debug)]
pub enum Command {
    CreateToken {
        token: CreateToken,
        resp: Responder<Token>,
    },
    GetValidToken {
        token_path: String, // figure out if it's possible to have a borrowed version
        resp: Responder<Option<Token>>,
    },
}

pub struct DBHandler {
    cmd_chan: mpsc::Sender<Command>,
}

impl DBHandler {
    pub async fn create_token(&self, tok: CreateToken) -> Result<Token> {
        let (resp_tx, resp_rx) = oneshot::channel();
        let cmd = Command::CreateToken {
            token: tok,
            resp: resp_tx,
        };
        self.cmd_chan.send(cmd).await?;
        let result_token = resp_rx.await?;
        result_token
    }

    pub async fn get_valid_token(&self, token_path: &str) -> Result<Option<Token>> {
        let (resp_tx, resp_rx) = oneshot::channel();
        let cmd = Command::GetValidToken {
            token_path: token_path.to_string(),
            resp: resp_tx,
        };
        self.cmd_chan.send(cmd).await?;
        let result_token = resp_rx.await?;
        result_token
    }

    // pub fn handle_command(&self, cmd: Command) {
    //     match cmd {
    //         Command::CreateToken { token, resp } => {
    //             let result = self.handle_create_token(token);
    //             let _ = resp.send(result);
    //         }
    //     }
    // }

    // fn handle_create_token(&self, tok: CreateToken) -> Result<Token> {
    //     let conn = self.open()?;
    //
    //     // let existing_token = dsl::token
    //     //     .filter(dsl::status.eq_any(vec![TokenStatus::Fresh, TokenStatus::Used]))
    //     //     .filter(dsl::path.eq(&tok.path))
    //     //     .select(diesel::dsl::count_star())
    //     //     .first(&conn)?;
    //
    //     let existing_count: i64 = token::table
    //         .select(diesel::dsl::count_star())
    //         .first(&conn)?;
    //     if existing_count > 0 {
    //         bail!("A valid token already exists for this path")
    //     };
    //
    //     let sql_tok = CreateTokenSQLite {
    //         path: tok.path,
    //         status: TokenStatus::Fresh,
    //         max_size: tok.max_size_in_mb.map(|s| s as _),
    //         created_at: Utc::now().naive_utc(),
    //         token_expires_at: tok.token_expires_at,
    //         content_expires_at: tok.content_expires_at,
    //         deleted_at: None,
    //     };
    //
    //     conn.transaction::<_, anyhow::Error, _>(|| {
    //         let n_inserted = diesel::insert_into(token::table)
    //             .values(&sql_tok)
    //             .execute(&conn)
    //             .with_context(|| format!("Cannot insert {:?} into token table", &sql_tok))?;
    //
    //         println!("inserted returned: {:#?}", n_inserted);
    //         if n_inserted == 0 {
    //             Err(anyhow!("Didn't insert token: {:?}", sql_tok))
    //         } else {
    //             let inserted_token = token::table.order(token::id.desc()).first(&conn)?;
    //             Ok(inserted_token)
    //         }
    //     })
    // }

    // fn open(&self) -> Result<SqliteConnection> {
    //     SqliteConnection::establish(&self.db_path)
    //         .context(format!("Cannot connect to db at {}", &self.db_path))
    // }
}

pub fn init_db(db_path: String) -> (DBHandler, DBManager) {
    let (cmd_tx, cmd_rx) = mpsc::channel(8);
    (
        DBHandler { cmd_chan: cmd_tx },
        DBManager {
            db_path,
            cmd_chan: cmd_rx,
        },
    )
}

pub struct DBManager {
    pub db_path: String,
    cmd_chan: mpsc::Receiver<Command>,
}

impl DBManager {
    pub async fn run(mut self) -> Result<()> {
        tokio::spawn(async move {
            while let Some(cmd) = self.cmd_chan.recv().await {
                match cmd {
                    Command::CreateToken { token, resp } => {
                        let result = self.create_token(token);
                        let _ = resp.send(result);
                    },
                    Command::GetValidToken {token_path, resp } => {
                        let result = self.get_valid_token(token_path);
                        let _ = resp.send(result);
                    },
                }
            }
        })
        .await?;
        Ok(())
    }

    pub fn create_token(&self, token: CreateToken) -> Result<Token> {
        let conn = self.open()?;

        // there should be at most one token with a given path in status fresh or used.
        let existing_count: i64 = token::table
            .select(diesel::dsl::count_star())
            .filter(token::path.eq(&token.path))
            .filter(token::status.eq_any(vec![TokenStatus::Fresh, TokenStatus::Used]))
            .first(&conn)?;

        if existing_count > 0 {
            bail!("A valid token already exists for this path")
        };

        let sql_tok = CreateTokenSQLite {
            path: token.path,
            status: TokenStatus::Fresh,
            max_size: token.max_size_in_mb.map(|s| s as _),
            created_at: Utc::now().naive_utc(),
            token_expires_at: token.token_expires_at,
            content_expires_at: token.content_expires_at,
            deleted_at: None,
        };

        conn.transaction::<_, anyhow::Error, _>(|| {
            let n_inserted = diesel::insert_into(token::table)
                .values(&sql_tok)
                .execute(&conn)
                .with_context(|| format!("Cannot insert {:?} into token table", &sql_tok))?;

            println!("inserted returned: {:#?}", n_inserted);
            if n_inserted == 0 {
                Err(anyhow!("Didn't insert token: {:?}", sql_tok))
            } else {
                let inserted_token = token::table.order(token::id.desc()).first(&conn)?;
                Ok(inserted_token)
            }
        })
    }

    pub fn get_valid_token(&self, token_path: String) -> Result<Option<Token>> {
        let conn = self.open()?;
        // there should be at most one token with a given path in status fresh or used.
        let tok: Vec<Token> = token::table
                .filter(token::path.eq(token_path))
                .filter(token::status.eq_any(vec![TokenStatus::Fresh, TokenStatus::Used]))
                .load(&conn)?;
        Ok(tok.into_iter().next())
    }

    fn open(&self) -> Result<SqliteConnection> {
        SqliteConnection::establish(&self.db_path)
            .context(format!("Cannot connect to db at {}", &self.db_path))
    }
}

// impl rusqlite::types::FromSql for TokenStatus {
//     fn column_result(value: rusqlite::types::ValueRef) -> rusqlite::types::FromSqlResult<Self> {
//         match String::column_result(value)?.as_str() {
//             "fresh" => Ok(TokenStatus::Fresh),
//             "used" => Ok(TokenStatus::Used),
//             "expired" => Ok(TokenStatus::Expired),
//             "deleted" => Ok(TokenStatus::Deleted),
//             other => {
//                 let err = TokenStatusError {
//                     details: other.to_string(),
//                 };
//                 Err(rusqlite::types::FromSqlError::Other(Box::new(err)))
//             }
//         }
//     }
// }
//
//
// #[derive(Debug, Clone)]
// pub struct DB {
//     db_path: String
// }
//
// impl DB {
//     pub fn new(path_str: &str) -> Self {
//         DB {
//             db_path: path_str.to_string()
//         }
//     }
//
//     fn open(&self) -> rusqlite::Result<Connection> {
//         Connection::open(Path::new(&self.db_path))
//     }
// }
//
// impl VracPersistence for DB {
//
//     fn init_db(&self) -> Result<(), errors::Error> {
//         let conn = self.open()?;
//         conn.execute(
//             "CREATE TABLE IF NOT EXISTS token (
//                 id INTEGER PRIMARY KEY,
//                 path TEXT NOT NULL,
//                 status TEXT NOT NULL,
//                 max_size INTEGER,
//                 created_at DATETIME NOT NULL DEFAULT (datetime('now')),
//                 token_expires_at DATETIME NOT NULL,
//                 content_expires_at DATETIME,
//                 deleted_at DATETIME
//                 )",
//             rusqlite::params![],
//         )?;
//
//         conn.execute("PRAGMA encoding='UTF-8'", rusqlite::params![])?;
//         conn.execute("PRAGMA foreign_key = ON;", rusqlite::params![])?;
//         Ok(())
//     }
//
//     fn create_token(&self, tok: &CreateToken) -> Result<Token, errors::Error> {
//         let id;
//         {
//             let conn = self.open()?;
//             let n = conn.execute_named(
//                 "INSERT INTO token (path, status, max_size, content_expires_at, token_expires_at) SELECT :path, :status, :max_size, :content_expires_at, :token_expires_at WHERE NOT EXISTS(SELECT 1 FROM token where path = :path AND (status = :status_fresh OR status = :status_used) AND deleted_at IS NULL)",
//                 &[  (":path", &tok.path),
//                     (":status", &TokenStatus::Fresh),
//                     (":max_size", &tok.max_size_in_mb),
//                     (":content_expires_at", &tok.content_expires_at),
//                     (":token_expires_at", &tok.token_expires_at),
//                     (":status_fresh", &TokenStatus::Fresh),
//                     (":status_used", &TokenStatus::Used),
//                 ],
//             )?;
//
//             if n == 0 {
//                 return Err(errors::VracError::DuplicateToken(tok.path.clone()).into());
//             }
//
//             let mut stmt = conn.prepare("SELECT last_insert_rowid()")?;
//             id = stmt.query(rusqlite::params![])?.next()?.unwrap().get(0)?;
//         }
//         let token = self.get_token_by_id(id)?;
//         // safe to unwrap since the id we got is guaranteed to be in the table
//         // (and there is no DELETE)
//         Ok(token.unwrap())
//     }
//
//     fn get_token_by_id(&self, token_id: u32) -> Result<Option<Token>, errors::Error> {
//         let conn = self.open()?;
//         let mut stmt = conn.prepare("SELECT id, path, status, max_size, created_at, content_expires_at, token_expires_at, deleted_at FROM token WHERE id = ?1 AND deleted_at IS NULL")?;
//         let mut result_iter = stmt.query_map(rusqlite::params![token_id], token_from_row)?;
//
//         match result_iter.next() {
//             None => Ok(None),
//             Some(r) => {
//                 let result = r?;
//                 Ok(Some(result))
//             }
//         }
//     }
//
//     fn get_valid_token_by_path(&self, token_path: &str) -> Result<Option<Token>, errors::Error> {
//         let conn = self.open()?;
//         let mut stmt = conn.prepare("SELECT id, path, status, max_size, created_at, content_expires_at, token_expires_at, deleted_at FROM token WHERE path = ?1 AND (status = ?2 OR status = ?3) AND deleted_at IS NULL")?;
//         let mut result_iter = stmt.query_map(rusqlite::params![token_path, TokenStatus::Fresh, TokenStatus::Used], token_from_row)?;
//
//         match result_iter.next() {
//             None => Ok(None),
//             Some(r) => {
//                 let result = r?;
//                 Ok(Some(result))
//             }
//         }
//     }
// }
//
// fn token_from_row(row: &rusqlite::Row) -> rusqlite::Result<Token> {
//     Ok(Token {
//         id: row.get(0)?,
//         path: row.get(1)?,
//         status: row.get(2)?,
//         max_size_in_mb: row.get(3)?,
//         created_at: row.get(4)?,
//         content_expires_at: row.get(5)?,
//         token_expires_at: row.get(6)?,
//         deleted_at: row.get(7)?,
//     })
// }
