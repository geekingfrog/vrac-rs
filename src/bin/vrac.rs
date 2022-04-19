use chrono;
use chrono_humanize;
use futures::stream::{self, Stream, StreamExt, TryStreamExt};
use multer::bytes::{Bytes, BytesMut};
use rocket::data::{ByteUnit, Data, ToByteUnit};
use rocket::fairing::AdHoc;
use rocket::form::{Form, FromForm};
use rocket::outcome::Outcome;
use rocket::request::FlashMessage;
use rocket::response::{Flash, Redirect, Responder};
use rocket::serde::{de::Error, Deserialize, Deserializer, Serialize};
use rocket::tokio::sync::Mutex;
use rocket::tokio::{fs, io, io::AsyncWrite, io::AsyncWriteExt};
use rocket::{http, request, response};
use rocket_dyn_templates::Template;
use rocket_sync_db_pools::database;
use scrypt::password_hash::{PasswordHash, PasswordVerifier};
use scrypt::Scrypt;
use std::path::{Path, PathBuf};
use tokio_util::codec;

use multer::{Constraints, Multipart, SizeLimit};

use anyhow::Context;

use vrac::cleanup;
use vrac::db;
use vrac::errors;

#[derive(Debug, Deserialize)]
#[serde(crate = "rocket::serde")]
struct VracConfig {
    root_path: PathBuf,
}

impl Default for VracConfig {
    fn default() -> Self {
        Self {
            root_path: std::env::current_dir().expect("Cannot access current dir???"),
        }
    }
}

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
fn gen_token_get(_admin: AdminUser, flash: Option<FlashMessage<'_>>) -> Template {
    let ctx: Option<FlashData> = flash.map(|f| f.into());
    Template::render("gen_token", &ctx)
}

struct RequiresBasicAuth;

impl<'r> Responder<'r, 'static> for RequiresBasicAuth {
    fn respond_to(self, _request: &'r rocket::Request<'_>) -> response::Result<'static> {
        let hdr = http::Header {
            name: "WWW-Authenticate".into(),
            value: r#"Basic realm="vrac""#.into(),
        };

        Ok(rocket::response::Response::build()
            .status(http::Status::Unauthorized)
            .header(hdr)
            .finalize())
    }
}

#[rocket::get("/gen", rank = 2)]
fn gen_token_get_pecore<'r>() -> impl Responder<'r, 'static> {
    // WWW-Authenticate: Basic realm="Our Site"
    // log::info!("NO LOGIN!");
    // let ctx: Option<FlashData> = flash.map(|f| f.into());
    // Template::render("gen_token", &ctx)
    RequiresBasicAuth {}
}

#[rocket::post("/gen", data = "<form_input>")]
async fn gen_token_post<'a, 'o>(
    form_input: Form<TokenInput<'_>>,
    conn: VracDbConn,
    write_lock: &rocket::State<WriteLock>,
    _admin: AdminUser,
) -> errors::Result<Flash<Redirect>> {
    let now = chrono::Utc::now();
    let token_expires_at =
        (now + chrono::Duration::hours(form_input.token_valid_for as _)).naive_utc();
    let content_expires_after_hours = form_input
        .content_expires_after_hours
        .map(|h| chrono::Duration::hours(h as _));
    let token = db::CreateToken {
        path: form_input.path.to_string(),
        max_size_in_mib: form_input.max_size,
        token_expires_at,
        content_expires_after_hours,
    };
    let new_token = {
        let _guard = write_lock.0.lock().await;
        conn.run(|c| db::create_token(c, token)).await
    };
    match new_token {
        Ok(new_token) => {
            let redir = response::Redirect::to(rocket::uri!(get_file(new_token.path)));
            Ok(Flash::success(redir, "Token created"))
        }
        Err(err) => {
            let redir = Redirect::to(rocket::uri!(gen_token_get()));
            Ok(Flash::error(redir, format!("{err}")))
        }
    }
}

#[rocket::post("/gen", rank = 2)]
async fn gen_token_post_pecore<'r>() -> impl Responder<'r, 'static> {
    RequiresBasicAuth {}
}

#[derive(Serialize)]
struct FileView {
    id: i32,
    name: Option<String>,
    content_type: Option<String>,
    dl_uri: String,
    is_image: bool,
}

#[derive(Serialize)]
struct GetFilesView<'a> {
    tok_str: &'a str,
    files: Vec<FileView>,
    flash: Option<FlashData>,
}

#[rocket::get("/f/<tok>")]
async fn get_file(
    tok: &str,
    conn: VracDbConn,
    flash: Option<FlashMessage<'_>>,
) -> errors::Result<Option<Template>> {
    let tokstr = tok.to_string();
    let tok: Option<db::Token> = conn.run(|c| db::get_valid_token(c, tokstr)).await?;

    match tok {
        None => Ok(None),
        Some(tok) => match &tok.status {
            db::TokenStatus::Fresh => Ok(Some(get_file_upload(tok, flash).await)),
            db::TokenStatus::Used => get_files_view(tok, conn, flash).await,
            db::TokenStatus::Deleted => unreachable!("valid token cannot be deleted"),
        },
    }
}

async fn get_files_view(
    token: db::Token,
    conn: VracDbConn,
    flash: Option<FlashMessage<'_>>,
) -> errors::Result<Option<Template>> {
    let path = token.path.clone();
    let files = conn.run(move |c| db::get_files(c, &token)).await?;
    let ctx = GetFilesView {
        tok_str: &path,
        files: files
            .into_iter()
            .map(|f| {
                let is_image = f
                    .content_type
                    .as_ref()
                    .map(|ct| ct.starts_with("image"))
                    .unwrap_or(false);

                FileView {
                    id: f.id,
                    name: f.name,
                    content_type: f.content_type,
                    dl_uri: rocket::uri!(download_file(path.clone(), f.id)).to_string(),
                    is_image,
                }
            })
            .collect(),
        flash: flash.map(|f| f.into()),
    };
    Ok(Some(Template::render("get_files", &ctx)))
}

#[rocket::get("/f/<tok_id>/<f_id>")]
async fn download_file(
    tok_id: String,
    f_id: i32,
    conn: VracDbConn,
) -> errors::Result<Option<(http::ContentType, fs::File)>> {
    let file: Option<db::File> = conn
        .run(move |c| {
            let token = db::get_valid_token(c, tok_id)?;
            let token = match token {
                Some(t) => t,
                None => return Ok(None),
            };
            let file = db::get_file(c, &token, f_id)?;
            let r: errors::Result<Option<db::File>> = Ok(file);
            r
        })
        .await?;

    let file = match file {
        Some(f) => f,
        None => return Ok(None),
    };

    let fd = fs::File::open(file.path).await?;
    // box & dyn don't play well with the Responder implementations, so
    // default to a content type instead of returning different type of response
    // depending on the match on file.content_type
    let content_type = file
        .content_type
        .and_then(|ct| http::ContentType::parse_flexible(&ct))
        .unwrap_or(http::ContentType::Binary);
    Ok(Some((content_type, fd)))
}

#[derive(Serialize)]
struct UploadFilesData {
    form_action: String,
    max_size_in_mib: Option<i32>,
    token_expires_at_human: String,
    content_expires_after_human: Option<String>,
    flash: Option<FlashData>,
}

#[derive(Serialize)]
struct FlashData {
    color: &'static str,
    message: String,
}

impl<'f> std::convert::From<FlashMessage<'f>> for FlashData {
    fn from(flash: FlashMessage) -> Self {
        let color = match flash.kind() {
            "success" => "limegreen",
            "error" => "red",
            "warning" => "orange",
            _ => "default",
        };
        FlashData {
            color,
            message: flash.message().to_string(),
        }
    }
}

async fn get_file_upload(tok: db::Token, flash: Option<FlashMessage<'_>>) -> Template {
    let ctx = UploadFilesData {
        form_action: rocket::uri!(get_file(tok.path)).to_string(),
        max_size_in_mib: tok.max_size_in_mib,
        token_expires_at_human: tok.token_expires_at.format("%F %r").to_string(),
        content_expires_after_human: tok
            .content_expires_after_hours
            .map(|h| chrono_humanize::HumanTime::from(chrono::Duration::hours(h as _)).to_string()),
        flash: flash.map(|f| f.into()),
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

struct AdminUser;

#[rocket::async_trait]
impl<'r> request::FromRequest<'r> for AdminUser {
    type Error = std::convert::Infallible;

    async fn from_request(request: &'r rocket::Request<'_>) -> request::Outcome<Self, Self::Error> {
        match request.headers().get_one("Authorization") {
            Some(auth) => {
                if let Some(encoded_creds) = auth.strip_prefix("Basic ") {
                    let conn = match request.guard::<VracDbConn>().await {
                        Outcome::Success(conn) => conn,
                        Outcome::Failure(_) | Outcome::Forward(_) => return Outcome::Forward(()),
                    };
                    if is_basic_auth_valid(conn, encoded_creds).await {
                        log::debug!("auth is valid!");
                        Outcome::Success(AdminUser {})
                    } else {
                        log::debug!("auth is invalid!");
                        Outcome::Forward(())
                    }
                } else {
                    request::Outcome::Forward(())
                }
            }
            None => request::Outcome::Forward(()),
        }
    }
}

#[rocket::post("/f/<tok>", data = "<data>")]
async fn upload_file<'a, 'o>(
    tok: &str,
    conn: VracDbConn,
    data: Data<'_>,
    boundary: MultipartBoundary<'_>,
    write_lock: &rocket::State<WriteLock>,
    vrac_config: &rocket::State<VracConfig>,
) -> errors::Result<Option<Flash<Redirect>>> {
    log::info!("vrac config is: {vrac_config:?}");
    let tokstr = tok.to_string();
    let dbtoken: db::Token = match conn.run(|c| db::get_valid_token(c, tokstr)).await? {
        // TODO would be better to redirect to get_file or something along these lines?
        // may not work for API usage though
        None => return Ok(None),
        Some(tok) => tok,
    };

    log::debug!("token: {:#?}", dbtoken);

    let max_stream_size = match dbtoken.max_size_in_mib {
        // add 10 kiB (generous) to account for the boundaries in the actual form
        Some(s) => s.mebibytes() + ByteUnit::Kibibyte(10),
        None => usize::MAX.mebibytes(),
    };
    log::info!("streaming at most {} mebibytes", max_stream_size);

    // open(size) will close the connection after the limit. This result in a broken pipe
    // for the client, on a browser you get a page "connectio was reset" which isn't ideal
    // TODO: perhaps, when the limit is reached, continue reading but discard everything
    // and return the correct error? That could be used to use a lot of network resource though.
    // Also, figure out how to clean up stuff already uploaded
    let stream =
        codec::FramedRead::new(data.open(usize::MAX.mebibytes()), codec::BytesCodec::new());

    // TODO allow more files
    let constraints = Constraints::new()
        .allowed_fields(vec!["file-1"])
        .size_limit(SizeLimit::new().whole_stream(max_stream_size.as_u64()));
    let mut multipart = Multipart::with_constraints(stream, boundary.0.to_string(), constraints);

    // TODO: use cap_std to prevent an attacker to escape the root path with
    // some chosen value of tok.path
    // This is fairly minimal though since only admins/owner should have the
    // ability to generate tokens.
    let dest_path = vrac_config.root_path.as_path().join(&dbtoken.path);
    fs::create_dir_all(&dest_path)
        .await
        .context("Cannot create temporary file")?;

    while let Some(mut field) = multipart.next_field().await.context("multipart issue")? {
        let mut file_path = dest_path.to_path_buf();
        let mut file_size = ByteUnit::Mebibyte(0);
        match field.name().or_else(|| field.file_name()) {
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

        log::info!(
            "going to write some bytes to {}",
            &file_path.to_string_lossy(),
        );

        let db_file = {
            let _guard = write_lock.0.lock().await;
            let create_file = db::CreateFile {
                token_id: dbtoken.id,
                name: field.file_name().map(|s| s.to_string()),
                path: file_path.clone(),
                content_type: field.content_type().map(|ct| ct.to_string()),
            };
            conn.run(move |c| db::create_file(c, create_file)).await?
        };

        let file_to_write = fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .open(&file_path)
            .await
            .with_context(|| {
                format!(
                    "Error opening file {} for write",
                    &file_path.to_string_lossy()
                )
            })?;
        let mut writer = file_to_write;

        // TODO do something to cleanup the file on disk if there is an error.
        log::debug!("coucou for field {:?}", field);
        while let Some(chunk) = field.chunk().await.transpose() {
            let mut chunk = match chunk {
                Ok(c) => c,
                Err(err) => {
                    // TODO: here I can catch the exact error for size exceeded
                    log::error!("got an error while reading a chunk: {:?}", err);
                    let redir = Redirect::to(rocket::uri!(get_file(&tok)));
                    return Ok(Some(Flash::error(redir, "kaboom")));
                }
            };

            file_size = file_size + chunk.len().bytes();
            log::debug!(
                "written so far: {}  (wrote {})",
                file_size,
                chunk.len().bytes()
            );
            writer.write_all_buf(&mut chunk).await.with_context(|| {
                format!("Error writing to file {}", &file_path.to_string_lossy())
            })?;
            writer.flush().await.unwrap();
        }
        writer
            .shutdown()
            .await
            .with_context(|| format!("Error writing to file {}", &file_path.to_string_lossy()))?;
        let file_size_mib = file_size.as_u64();

        {
            let _guard = write_lock.0.lock().await;
            conn.run(move |c| db::complete_upload(c, db_file.id))
                .await?;
        }

        log::info!(
            "for file {} wrote {} - {} MiB",
            &file_path.to_string_lossy(),
            file_size,
            file_size_mib
        );
    }

    let tok_path = dbtoken.path.clone();
    {
        let _guard = write_lock.0.lock().await;
        log::info!("Consumming token {tok:?}");
        conn.run(move |c| db::consume_token(c, dbtoken)).await?;
    }
    let redir = Redirect::to(rocket::uri!(get_file(&tok_path)));
    Ok(Some(Flash::success(redir, "File uploaded.")))
}

#[database("sqlite_vrac")]
struct VracDbConn(diesel::SqliteConnection);

// simplify sqlite tx by only supporting one writer at a time.
struct WriteLock(Mutex<()>);

fn build_app() -> rocket::Rocket<rocket::Build> {
    rocket::custom(rocket::Config::figment())
        .mount(
            "/",
            rocket::routes![
                index,
                gen_token_get,
                gen_token_get_pecore,
                gen_token_post,
                gen_token_post_pecore,
                get_file,
                upload_file,
                download_file
            ],
        )
        .attach(Template::fairing())
        .attach(VracDbConn::fairing())
        .attach(AdHoc::config::<VracConfig>())
        .manage(WriteLock(Mutex::new(())))
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let app = build_app().ignite().await?;

    let pool = VracDbConn::get_one(&app)
        .await
        .ok_or("Cannot access connection pool")?;

    let web_server = async {
        app.launch().await?;
        Ok::<_, Box<dyn std::error::Error>>(())
    };

    let background_job = async {
        pool.run(|c| cleanup::cleanup_once(c).map_err(|err| format!("{:?}", err)))
            .await?;
        Ok(())
    };

    futures::try_join!(web_server, background_job)?;

    Ok(())
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

async fn is_basic_auth_valid(conn: VracDbConn, encoded_creds: &str) -> bool {
    let f = || async move {
        let bytes = base64::decode(encoded_creds)?;
        let s = std::str::from_utf8(&bytes[..])?;
        let split_idx = s.find(':').ok_or("Cannot find separator :")?;
        let (username, password) = s.split_at(split_idx);
        let password = &password[1..]; // remove the :
        log::debug!("verifying auth for username {username}");
        // grmbl, need that because conn.run expects 'static
        let username = username.to_string();
        let auth = conn.run(move |c| db::get_user_auth(c, username)).await?;
        match auth {
            db::Auth::Basic { phc } => {
                let parsed_hash = PasswordHash::new(&phc)?;
                Scrypt.verify_password(password.as_bytes(), &parsed_hash)?;
                Ok(())
            }
            _ => Err("oops".into()),
        }
    };

    let r: std::result::Result<_, Box<dyn std::error::Error>> = f().await;
    match r {
        Ok(_) => true,
        Err(err) => {
            log::error!("{err:?}");
            false
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
        Some(s) => S::from_str(s)
            .map(Some)
            .map_err(|_| D::Error::custom("could not parse string")),
        None => Ok(None),
    }
}
