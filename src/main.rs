use chrono;
use futures::stream::{self, Stream, StreamExt, TryStreamExt};
use log::info;
use multer::bytes::{Bytes, BytesMut};
use rocket;
use rocket::data::{Data, ToByteUnit};
use rocket::form::{Form, FromForm};
use rocket::serde::{de::Error, Deserialize, Deserializer, Serialize};
use rocket::tokio::sync::Mutex;
use rocket::tokio::{fs, io, io::AsyncWrite, io::AsyncWriteExt};
use rocket::{http, request, response};
use rocket_dyn_templates::Template;
use rocket_sync_db_pools::database;
use std::path::{Path, PathBuf};
use tokio_util::codec;

use multer::{Constraints, Multipart, SizeLimit};

#[macro_use]
extern crate anyhow;
use anyhow::Context;

#[macro_use]
extern crate diesel;
#[macro_use]
extern crate diesel_migrations;

mod db;
mod errors;
mod schema;

#[rocket::get("/")]
fn index() -> &'static str {
    "Coucou"
}

#[derive(Debug, FromForm, Deserialize)]
struct TokenInput<'r> {
    path: &'r str,
    #[field(name = "max-size")]
    #[serde(deserialize_with = "deserialize_sentinel")]
    max_size: Option<u32>,

    #[field(name = "content-expires")]
    #[serde(deserialize_with = "deserialize_sentinel")]
    content_expires_after_hours: Option<u64>,
    #[field(name = "token-valid-for")]
    token_valid_for: u64,
}

#[rocket::get("/gen")]
fn gen_token_get() -> Template {
    let ctx = ();
    Template::render("gen_token", &ctx)
}

#[rocket::post("/gen", data = "<form_input>")]
async fn gen_token_post(
    form_input: Form<TokenInput<'_>>,
    conn: VracDbConn,
    write_lock: &rocket::State<WriteLock>,
) -> String {
    println!("got a form input: {:?}", form_input);
    let now = chrono::Utc::now();
    let token_expires_at =
        (now + chrono::Duration::hours(form_input.token_valid_for as _)).naive_utc();
    let content_expires_after_hours = form_input
        .content_expires_after_hours
        .map(|h| chrono::Duration::hours(h as _));
    let token = db::CreateToken {
        path: form_input.path.to_string(),
        max_size_in_mb: None,
        token_expires_at,
        content_expires_after_hours,
    };
    let new_token = {
        let _guard = write_lock.0.lock().await;
        conn.run(|c| db::create_token(c, token)).await
    };
    format!("{:?}", new_token)
}

#[rocket::get("/f/<tok>")]
async fn get_file(tok: &str, conn: VracDbConn) -> errors::Result<Option<Template>> {
    let tokstr = tok.to_string();
    let tok: Option<db::Token> = conn.run(|c| db::get_valid_token(&c, tokstr)).await?;

    match tok {
        None => Ok(None),
        Some(tok) => match &tok.status {
            db::TokenStatus::Fresh => Ok(Some(get_file_upload(tok).await)),
            db::TokenStatus::Used => Ok(Some(Template::render("get_files", &()))),
            db::TokenStatus::Expired => unreachable!("valid token cannot be expired"),
            db::TokenStatus::Deleted => unreachable!("valid token cannot be deleted"),
        },
    }
}

#[derive(Serialize)]
struct UploadFilesData {
    form_action: String,
    max_size_in_mb: Option<i32>,
}

async fn get_file_upload(tok: db::Token) -> Template {
    let ctx = UploadFilesData {
        form_action: rocket::uri!(get_file(tok.path)).to_string(),
        max_size_in_mb: tok.max_size_in_mb,
    };
    Template::render("upload_files", &ctx)
}

#[derive(Debug)]
struct MultipartBoundary<'r>(&'r str);

#[rocket::async_trait]
impl<'r> request::FromRequest<'r> for MultipartBoundary<'r> {
    type Error = std::convert::Infallible;

    async fn from_request(request: &'r rocket::Request<'_>) -> request::Outcome<Self, Self::Error> {
        let ct = request.guard::<&http::ContentType>().await;
        ct.and_then(|ct| match ct.media_type().param("boundary").as_ref() {
            Some(boundary) => request::Outcome::Success(MultipartBoundary(boundary)),
            None => request::Outcome::Forward(()),
        })
    }
}

#[rocket::post("/f/<tok>", data = "<data>")]
async fn upload_file(
    tok: &str,
    conn: VracDbConn,
    data: Data<'_>,
    boundary: MultipartBoundary<'_>,
) -> errors::Result<Option<response::Redirect>> {
    let tokstr = tok.to_string();
    let tok = conn.run(|c| db::get_valid_token(&c, tokstr)).await?;

    let tok: db::Token = match tok {
        None => return Ok(None),
        Some(tok) => tok,
    };
    println!("token: {:#?}", tok);

    let max_stream_size = match tok.max_size_in_mb {
        // add 1 to account for the boundaries in the actual form
        Some(s) => (s + 1).mebibytes(),
        None => usize::MAX.mebibytes(),
    };
    let stream = codec::FramedRead::new(data.open(max_stream_size), codec::BytesCodec::new());
    let mut multipart = Multipart::new(stream, boundary.0.to_string());

    let dest_path = Path::new("/tmp/vractest/foo");
    fs::create_dir_all(dest_path)
        .await
        .context("Cannot create temporary file")?;

    while let Some(mut field) = multipart.next_field().await? {
        let mut file_path = dest_path.to_path_buf();
        match field.file_name() {
            Some(file_name) => {
                if file_name.is_empty() {
                    // avoid creating empty files
                    continue;
                } else {
                    file_path.push(file_name);
                }
            }
            None => continue,
        };

        info!(
            "going to write some bytes to {}",
            &file_path.to_string_lossy(),
        );

        let file_to_write = fs::OpenOptions::new()
            .write(true)
            .create(true)
            .open(&file_path)
            .await
            .with_context(|| {
                format!(
                    "Error opening file {} for write",
                    &file_path.to_string_lossy()
                )
            })?;
        let mut writer = io::BufWriter::new(file_to_write);

        while let Some(chunk) = field.chunk().await? {
            writer.write(&chunk).await.with_context(|| {
                format!("Error writing to file {}", &file_path.to_string_lossy())
            })?;
        }
        writer
            .shutdown()
            .await
            .with_context(|| format!("Error writing to file {}", &file_path.to_string_lossy()))?;
    }

    let tok_path = tok.path.clone();
    conn.run(move |c| db::consume_token(c, tok)).await?;
    Ok(Some(response::Redirect::to(rocket::uri!(get_file(
        &tok_path
    )))))
}

#[database("sqlite_vrac")]
struct VracDbConn(diesel::SqliteConnection);

// simplify sqlite tx by only supporting one writer at a time.
struct WriteLock(Mutex<()>);

#[rocket::launch]
fn rocket_main() -> _ {
    rocket::build()
        .mount(
            "/",
            rocket::routes![index, gen_token_get, gen_token_post, get_file, upload_file,],
        )
        .attach(Template::fairing())
        .attach(VracDbConn::fairing())
        .manage(WriteLock(Mutex::new(())))
}

// See:
// https://stackoverflow.com/questions/56384447/how-do-i-transform-special-values-into-optionnone-when-using-serde-to-deserial
fn deserialize_sentinel<'de, T, D>(deserializer: D) -> Result<Option<T>, D::Error>
where
    D: Deserializer<'de>,
    T: std::str::FromStr,
{
    let value: Result<Maybe<T>, _> = Deserialize::deserialize(deserializer);

    match value {
        Ok(Maybe::Just(x)) => Ok(x),
        Ok(Maybe::Nothing(raw)) => {
            if raw == "None" {
                Ok(None)
            } else {
                Err(serde::de::Error::custom(format!(
                    "Unexpected string {}",
                    raw
                )))
            }
        }
        Err(e) => {
            eprintln!("got err: {:?}", e);
            Err(e)
        }
    }
}

// serde(untagged) and serde(flatten) are buggy with serde_qs and serde_urlencoded
// there is a workaround:
// https://github.com/nox/serde_urlencoded/issues/33
// https://github.com/samscott89/serde_qs/issues/14#issuecomment-456865916
// the following is an adaptation to wrap the value into an Option
#[derive(Deserialize, Debug)]
#[serde(untagged)]
enum Maybe<U: std::str::FromStr> {
    #[serde(deserialize_with = "from_option_str")]
    Just(Option<U>),
    // #[serde(deserialize_with = "from_str")]
    Nothing(String),
}

fn from_option_str<'de, D, S>(deserializer: D) -> Result<Option<S>, D::Error>
where
    D: serde::Deserializer<'de>,
    S: std::str::FromStr,
{
    let s: Option<&str> = Deserialize::deserialize(deserializer)?;
    match s {
        Some(s) => S::from_str(&s)
            .map(Some)
            .map_err(|_| D::Error::custom("could not parse string")),
        None => Ok(None),
    }
}
