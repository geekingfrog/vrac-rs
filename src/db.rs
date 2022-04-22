use anyhow::Context;
use chrono::naive::NaiveDateTime;
use chrono::Utc;
use diesel::{
    backend::Backend, deserialize::FromSql, prelude::*, result::OptionalExtension,
    serialize::ToSql, sql_types, sql_types::Text, Connection, Insertable, Queryable,
    SqliteConnection,
};
use scrypt::{
    password_hash::{rand_core::OsRng, PasswordHasher, SaltString},
    Scrypt,
};
use std::collections::HashMap;

use crate::errors;
use crate::schema::{auth, file, token};

diesel_migrations::embed_migrations!("./migrations/");

#[derive(Debug, Queryable, Identifiable, Hash, PartialEq, Eq)]
#[table_name = "token"]
pub struct Token {
    pub id: i32,
    pub path: String,
    pub status: TokenStatus,
    pub max_size_in_mib: Option<i32>,
    pub created_at: NaiveDateTime,
    pub token_expires_at: NaiveDateTime,
    /// any file associated to this token after `content_expires_at` can be deleted
    pub content_expires_at: Option<NaiveDateTime>,
    /// we need to store in the token how long the associated content will
    /// live for. At token creation, we can't set the expiration date.
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

#[derive(Debug, FromSqlRow, AsExpression, Clone, Copy, Hash, PartialEq, Eq)]
#[sql_type = "Text"]
pub enum TokenStatus {
    Fresh,
    Used,
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

    conn.transaction(|| {
        let now = chrono::Utc::now().naive_utc();
        let existing_count: i64 = token::table
            .select(diesel::dsl::count_star())
            .filter(dsl::path.eq(&tok.path))
            .filter(
                token::token_expires_at
                    .ge(now)
                    .or(token::content_expires_at.ge(now)),
            )
            .first(conn)?;

        if existing_count > 0 {
            return Err(errors::VracError::TokenAlreadyExists(tok.path));
        };

        let sql_tok = CreateTokenSQLite {
            path: tok.path,
            status: TokenStatus::Fresh,
            max_size_mib: tok.max_size_in_mib.map(|s| s as _),
            created_at: Utc::now().naive_utc(),
            token_expires_at: tok.token_expires_at,
            content_expires_at: None,
            content_expires_after_hours: tok
                .content_expires_after_hours
                .map(|d| d.num_hours() as _),
            deleted_at: None,
        };

        let n_inserted = diesel::insert_into(token::table)
            .values(&sql_tok)
            .execute(conn)
            .with_context(|| format!("Cannot insert {:?} into token table", &sql_tok))?;

        println!("inserted returned: {:#?}", n_inserted);
        if n_inserted == 0 {
            Err(anyhow!("Didn't insert token: {:?}", sql_tok).into())
        } else {
            let inserted_token = token::table.order(token::id.desc()).first(conn)?;
            Ok(inserted_token)
        }
    })
}

/// returns a token with a status of Fresh or Used, and also ensure
/// that the associated content hasn't expired yet
pub fn get_valid_token(
    conn: &SqliteConnection,
    token_path: String,
) -> std::result::Result<Option<Token>, diesel::result::Error> {
    // there should be at most one token with a given path in status fresh or used.
    let now = chrono::Utc::now().naive_utc();
    let tok: Vec<Token> = token::table
        .filter(token::path.eq(token_path))
        .filter(
            token::token_expires_at
                .ge(now)
                .or(token::content_expires_at.ge(now)),
        )
        .load(conn)?;
    Ok(tok.into_iter().next())
}

/// Returns a list of expired token and their associated file
pub fn get_expired_files(
    conn: &SqliteConnection,
) -> std::result::Result<HashMap<Token, Vec<File>>, Box<dyn std::error::Error>> {
    let now = chrono::Utc::now().naive_utc();
    let expired_tokens: Vec<Token> = token::table
        .filter(token::content_expires_at.le(now))
        .filter(token::dsl::deleted_at.is_null())
        .load(conn)?;

    let mut result = HashMap::new();

    // It's sqlite so n+1 requests is no big deal
    for tok in expired_tokens {
        let expired_files = File::belonging_to(&tok).load::<File>(conn)?;
        result.insert(tok, expired_files);
    }

    Ok(result)
}

/// mark all expired token as deleted and returns their paths.
pub fn delete_expired_tokens(
    conn: &SqliteConnection,
) -> std::result::Result<Vec<String>, Box<dyn std::error::Error>> {
    let now = chrono::Utc::now().naive_utc();

    let to_delete: Vec<Token> = token::table
        .filter(
            token::dsl::token_expires_at
                .le(now)
                .or(token::dsl::content_expires_at.le(now)),
        )
        .filter(token::dsl::deleted_at.is_null())
        .load(conn)?;

    let ids_to_del = to_delete.iter().map(|t| t.id);
    diesel::update(token::dsl::token.filter(token::dsl::id.eq_any(ids_to_del)))
        .set((
            token::dsl::deleted_at.eq(now),
            token::dsl::status.eq(TokenStatus::Deleted),
        ))
        .execute(conn)?;

    let deleted_paths = to_delete.into_iter().map(|t| t.path).collect();
    Ok(deleted_paths)
}

/// mark the given tokens and their associated files as deleted in the DB
/// Returns the total number of deleted files.
pub fn delete_files(
    conn: &SqliteConnection,
    tokens: &[Token],
) -> std::result::Result<usize, Box<dyn std::error::Error>> {
    let deleted_file_count = conn.transaction::<_, diesel::result::Error, _>(|| {
        // use file::dsl::file;
        let mut deleted_file_count = 0;

        let now = chrono::Utc::now().naive_utc();
        for tok in tokens {
            deleted_file_count +=
                diesel::update(file::dsl::file.filter(file::dsl::token_id.eq(tok.id)))
                    .set(file::dsl::deleted_at.eq(now))
                    .execute(conn)?;
            diesel::update(token::dsl::token.find(tok.id))
                .set((
                    token::dsl::deleted_at.eq(now),
                    token::dsl::status.eq(TokenStatus::Deleted),
                ))
                .execute(conn)?;
        }
        Ok(deleted_file_count)
    })?;
    Ok(deleted_file_count)
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
            // cannot use get_result because sqlite doesn't support RETURNING :(
            .execute(conn)?;

        if n_inserted == 0 {
            Err(anyhow!("Didn't insert file: {:?}", create_file).into())
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

/// remove the corresponding row in the file table. When something goes wrong
/// during the upload, this should be used to cleanup afterward.
pub fn abort_upload(conn: &SqliteConnection, file_id: i32) -> errors::Result<()> {
    use crate::schema::file::dsl;
    diesel::delete(dsl::file.find(file_id)).execute(conn)?;
    Ok(())
}

pub fn get_files(conn: &SqliteConnection, token: &Token) -> errors::Result<Vec<File>> {
    use crate::schema::file::dsl;
    let files = File::belonging_to(token)
        .filter(dsl::file_upload_status.eq(FileUploadStatus::Completed))
        .load(conn)?;
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

pub fn connect(db_url: &str) -> errors::Result<SqliteConnection> {
    Ok(SqliteConnection::establish(db_url)
        .with_context(|| format!("cannot connect to {db_url}"))?)
}

// Atfer spending a few hours trying to figure out the intricacies of
// Insertable and Queryable traits, I give up and use a different enum
// for auth, the conversion will be done manually.
// Storing an enum with field is unscrutable black magic (for me atm at least)
#[derive(Debug, Insertable, Queryable)]
#[table_name = "auth"]
struct AuthRow {
    id: String,
    typ: String,
    data: String,
}

#[derive(Debug)]
#[non_exhaustive]
pub enum Auth {
    Basic { phc: String },
}

pub fn gen_user(
    conn: &SqliteConnection,
    username: String,
    cleartext_password: String,
) -> errors::Result<()> {
    let salt = SaltString::generate(&mut OsRng);
    let phc = Scrypt
        .hash_password(cleartext_password.as_bytes(), &salt)
        .with_context(|| format!("Cannot hash password for user {username}"))?
        .to_string();

    let auth = AuthRow {
        id: username,
        typ: "BASIC".to_string(),
        data: phc,
    };

    // don't care if the user already exist and this fails.
    diesel::insert_into(auth::table)
        .values(&auth)
        .execute(conn)
        .with_context(|| "cannot create user")?;
    Ok(())
}

/// returns the hashed password for the given user in the [PHC
/// format](https://github.com/P-H-C/phc-string-format/blob/master/phc-sf-spec.md)
pub fn get_user_auth(conn: &SqliteConnection, username: String) -> errors::Result<Auth> {
    use crate::schema::auth::dsl;
    let result: AuthRow = dsl::auth.find(username).get_result(conn)?;
    match &result.typ[..] {
        "BASIC" => Ok(Auth::Basic { phc: result.data }),
        _ => todo!(),
    }
}
