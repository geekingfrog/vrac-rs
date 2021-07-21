use crate::errors;
use anyhow::{Context, Result};
use chrono::naive::NaiveDateTime;
use chrono::Utc;
use rocket::data::ByteUnit;

use diesel::{
    backend::Backend, deserialize::FromSql, prelude::*, result::OptionalExtension,
    serialize::ToSql, sql_types, sql_types::Text, Connection, Insertable, Queryable,
    SqliteConnection,
};

use crate::schema::{file, token};

diesel_migrations::embed_migrations!("./migrations/");

#[derive(Debug, Queryable, Identifiable)]
#[table_name = "token"]
pub struct Token {
    pub id: i32,
    pub path: String,
    pub status: TokenStatus,
    pub max_size_in_mib: Option<i32>,
    pub created_at: NaiveDateTime,
    pub token_expires_at: NaiveDateTime,
    pub content_expires_at: Option<NaiveDateTime>,
    pub content_expires_after_hours: Option<i32>,
    pub deleted_at: Option<NaiveDateTime>,
}

#[derive(Debug)]
pub struct CreateToken {
    pub path: String,
    pub max_size_in_mib: Option<u32>,
    pub token_expires_at: NaiveDateTime,
    pub content_expires_after_hours: Option<chrono::Duration>,
}

#[derive(Debug, Insertable)]
#[table_name = "token"]
struct CreateTokenSQLite {
    path: String,
    status: TokenStatus,
    max_size_mib: Option<i32>,
    created_at: NaiveDateTime,
    token_expires_at: NaiveDateTime,
    content_expires_at: Option<NaiveDateTime>,
    content_expires_after_hours: Option<i32>,
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
diesel::no_arg_sql_function!(last_insert_rowid, sql_types::Integer);

#[derive(Debug, FromSqlRow, AsExpression, Clone, Copy)]
#[sql_type = "Text"]
pub enum FileUploadStatus {
    Started,
    Completed,
}

impl<DB> FromSql<sql_types::Text, DB> for FileUploadStatus
where
    DB: Backend,
    String: FromSql<sql_types::Text, DB>,
{
    fn from_sql(bytes: Option<&DB::RawValue>) -> diesel::deserialize::Result<Self> {
        match &(String::from_sql(bytes)?)[..] {
            "STARTED" => Ok(FileUploadStatus::Started),
            "COMPLETED" => Ok(FileUploadStatus::Completed),
            x => Err(format!("Unknown file upload status: {}", x).into()),
        }
    }
}

impl<DB> ToSql<sql_types::Text, DB> for FileUploadStatus
where
    DB: Backend,
{
    fn to_sql<W: std::io::Write>(
        &self,
        out: &mut diesel::serialize::Output<W, DB>,
    ) -> diesel::serialize::Result {
        let tag = match self {
            FileUploadStatus::Started => "STARTED",
            FileUploadStatus::Completed => "COMPLETED",
        };
        ToSql::<sql_types::Text, DB>::to_sql(tag, out)
    }
}

#[derive(Debug, Queryable, Associations, Identifiable)]
#[belongs_to(Token)]
#[table_name = "file"]
pub struct File {
    pub id: i32,
    pub token_id: i32,
    pub name: Option<String>,
    pub path: String,
    pub content_type: Option<String>,
    pub size_mib: Option<i32>,
    pub created_at: NaiveDateTime,
    pub deleted_at: Option<NaiveDateTime>,
    pub file_upload_status: FileUploadStatus,
}

#[derive(Debug)]
pub struct CreateFile {
    pub path: std::path::PathBuf,
    pub name: Option<String>,
    pub content_type: Option<String>,
    pub token_id: i32,
}

#[derive(Debug, Insertable)]
#[table_name = "file"]
struct CreateFileSQLite {
    token_id: i32,
    name: Option<String>,
    path: String,
    content_type: Option<String>,
    size_mib: Option<i32>,
    file_upload_status: FileUploadStatus,
    created_at: NaiveDateTime,
    deleted_at: Option<NaiveDateTime>,
}

pub fn create_token(
    conn: &mut SqliteConnection,
    tok: CreateToken,
) -> std::result::Result<Token, errors::VracError> {
    use token::dsl;
    // let existing_token = dsl::token
    //     .filter(dsl::status.eq_any(vec![TokenStatus::Fresh, TokenStatus::Used]))
    //     .filter(dsl::path.eq(&tok.path))
    //     .select(diesel::dsl::count_star())
    //     .first(&conn)?;

    conn.transaction(|| {
        let existing_count: i64 = token::table
            .select(diesel::dsl::count_star())
            .filter(dsl::status.eq_any(vec![TokenStatus::Fresh, TokenStatus::Used]))
            .filter(dsl::path.eq(&tok.path))
            .first(conn)?;

        if existing_count > 0 {
            return Err(errors::VracError::TokenAlreadyExists(tok.path))
        };

        let sql_tok = CreateTokenSQLite {
            path: tok.path,
            status: TokenStatus::Fresh,
            max_size_mib: tok.max_size_in_mib.map(|s| s as _),
            created_at: Utc::now().naive_utc(),
            token_expires_at: tok.token_expires_at,
            content_expires_at: None,
            content_expires_after_hours: tok.content_expires_after_hours.map(|d| d.num_hours() as _),
            deleted_at: None,
        };

        let n_inserted = diesel::insert_into(token::table)
            .values(&sql_tok)
            .execute(conn)
            .with_context(|| format!("Cannot insert {:?} into token table", &sql_tok))?;

        println!("inserted returned: {:#?}", n_inserted);
        if n_inserted == 0 {
            Err(anyhow!("Didn't insert token: {:?}", sql_tok))?
        } else {
            let inserted_token = token::table.order(token::id.desc()).first(conn)?;
            Ok(inserted_token)
        }
    })
}

pub fn get_valid_token(
    conn: &SqliteConnection,
    token_path: String,
) -> std::result::Result<Option<Token>, diesel::result::Error> {
    // there should be at most one token with a given path in status fresh or used.
    let tok: Vec<Token> = token::table
        .filter(token::path.eq(token_path))
        .filter(token::status.eq_any(vec![TokenStatus::Fresh, TokenStatus::Used]))
        .load(conn)?;
    Ok(tok.into_iter().next())
}

/// Mark the given as Used, all files have been uploaded
pub fn consume_token(
    conn: &SqliteConnection,
    tok: Token,
) -> std::result::Result<(), diesel::result::Error> {
    use token::dsl;
    let expires_at = tok
        .content_expires_after_hours
        .map(|h| (chrono::Utc::now() + chrono::Duration::hours(h as _)).naive_utc());
    diesel::update(token::table.find(tok.id))
        .set((
            dsl::status.eq(TokenStatus::Used),
            dsl::content_expires_at.eq(expires_at),
        ))
        .execute(conn)
        .map(|_| ())
}

pub fn create_file(conn: &SqliteConnection, file: CreateFile) -> errors::Result<File> {
    use crate::schema::file::dsl;

    let create_file = CreateFileSQLite {
        token_id: file.token_id,
        name: file.name,
        path: file.path.to_string_lossy().to_string(),
        content_type: file.content_type,
        size_mib: None,
        file_upload_status: FileUploadStatus::Started,
        created_at: Utc::now().naive_utc(),
        deleted_at: None,
    };
    conn.transaction(move || {
        let n_inserted = diesel::insert_into(file::table)
            .values(&create_file)
            .execute(conn)?;

        if n_inserted == 0 {
            Err(anyhow!("Didn't insert file: {:?}", create_file))?
        } else {
            let inserted_file = dsl::file.order(file::id.desc()).first(conn)?;
            Ok(inserted_file)
        }
    })
}

pub fn complete_upload(conn: &SqliteConnection, file_id: i32) -> errors::Result<()> {
    use crate::schema::file::dsl;
    diesel::update(dsl::file.find(file_id))
        .set(dsl::file_upload_status.eq(FileUploadStatus::Completed))
        .execute(conn)?;
    Ok(())
}

pub fn get_files(conn: &SqliteConnection, token: &Token) -> errors::Result<Vec<File>> {
    let files = File::belonging_to(token).load(conn)?;
    Ok(files)
}

pub fn get_file(
    conn: &SqliteConnection,
    token: &Token,
    file_id: i32,
) -> errors::Result<Option<File>> {
    use crate::schema::file::dsl;
    let f = File::belonging_to(token)
        .filter(dsl::id.eq(file_id))
        .first(conn)
        .optional()?;
    Ok(f)
}
