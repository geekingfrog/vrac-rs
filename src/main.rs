#![allow(unused_imports)]
#[macro_use]
extern crate anyhow;
#[macro_use]
extern crate diesel_migrations;

#[macro_use]
extern crate diesel;

use bytes::{buf::BufMut, BytesMut};
use core::task::Poll;
use futures::io::AsyncBufRead;
use futures::{AsyncRead, Future};
use handlebars::Handlebars;
use http_types::{convert::Deserialize, mime};
use serde::de::{self, Deserializer, Error, Visitor};
use std::{
    pin::{self, Pin},
    sync::Arc,
    task,
};
use tide::prelude::*;
use tide::{Body, Request, Response, ResponseBuilder};
use tide_handlebars::prelude::*;
// use tokio::fs::File;
// use tokio::io::{
//     self, AsyncBufRead, AsyncBufReadExt, AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt,
//     BufReader,
// };
use pin_project::pin_project;

mod db;
mod schema;
use db::VracPersistence;
use std::str;

struct AppState {
    hb_registry: Handlebars<'static>,
    db: db::DBHandler,
}

// #[tokio::main]
// async fn main() -> anyhow::Result<()> {
//     let mut stdin = tokio::io::stdin();
//     let mut stdout = tokio::io::stdout();
//     let mut buf = BytesMut::with_capacity(4);
//
//     loop {
//         let read_quantity = stdin.read(&mut buf).await?;
//         if read_quantity == 0 {
//             break;
//         } else {
//             stdout.write(&buf[0..read_quantity]).await?;
//             buf.clear();
//         }
//     }
//
//     Ok(())
// }

#[tokio::main]
async fn main() -> tide::Result<()> {
    tide::log::start();
    println!("max u64: {}", u64::MAX);

    let (db_handler, db_manager) = db::init_db("vrac.sqlite".to_string());

    let mut app_state = AppState {
        hb_registry: Handlebars::new(),
        db: db_handler,
    };

    // the extension in the template name will drive the mime type
    app_state
        .hb_registry
        .register_template_file("base.html", "./templates/base.hbs")
        .unwrap();
    app_state
        .hb_registry
        .register_template_file("gen_token.html", "./templates/gen_token.hbs")
        .unwrap();
    app_state
        .hb_registry
        .register_template_file("upload_files.html", "./templates/upload_files.hbs")
        .unwrap();

    let mut app = tide::with_state(Arc::new(app_state));
    app.at("/").get(root);
    app.at("/gen").get(gen_token_get).post(gen_token_post);
    app.at("/f/:token_path").get(get_files).post(upload_files);

    tokio::try_join!(
        async move {
            app.listen("127.0.0.1:8888").await?;
            Ok(())
        },
        db_manager.run()
    )?;
    Ok(())
}

async fn root<State>(mut _req: Request<State>) -> tide::Result<String> {
    Ok("coucou".to_string())
}

async fn gen_token_get(req: Request<Arc<AppState>>) -> tide::Result<Response> {
    let hb = &req.state().hb_registry;
    Ok(hb.render_response("gen_token.html", &())?)
}

#[derive(Debug, Serialize)]
struct UploadFilesData {
    form_action: String,
    max_size_in_mb: Option<i32>,
}

async fn get_files(req: Request<Arc<AppState>>) -> tide::Result<Response> {
    let token_path = req.param("token_path")?;
    let state = &req.state();
    let db = &state.db;
    let hb = &state.hb_registry;

    match db.get_valid_token(token_path).await? {
        Some(token) => match &token.status {
            db::TokenStatus::Fresh => {
                let template_data = UploadFilesData {
                    form_action: format!("/f/{}", token.path),
                    max_size_in_mb: token.max_size_in_mb,
                };
                Ok(hb.render_response("upload_files.html", &template_data)?)
            }
            db::TokenStatus::Used => todo!("download files"),
            _ => unreachable!("SQL is broken !"),
        },
        None => Ok("Token expired or invalid".into()),
    }
}

async fn upload_files(mut req: Request<Arc<AppState>>) -> tide::Result<Response> {
    let token_path = req.param("token_path")?;
    let state = &req.state();
    let db = &state.db;
    // let hb = &state.hb_registry;

    match db.get_valid_token(token_path).await? {
        Some(token) => {
            for name in req.header_names() {
                tide::log::info!("header {} - {}", name, req.header(name).unwrap());
            }
            // println!("header names: {:?}", req.header_names());
            // println!("header values: {:?}", req.header_names());
            let mut body = req.take_body().into_reader();
            let mut target =
                async_std::fs::File::create(format!("/tmp/vrac/{}.part", token.path)).await?;
            let limit = token
                .max_size_in_mb
                // conservatively add 1 MB for the text boundaries in the body
                // since we're saving the raw body and not just the files
                .map(|s| (s + 1) as u64 * 1024 * 1024)
                .unwrap_or(u64::MAX);
            let copy_result = copy_limit(&mut body, &mut target, limit).await?;
            tide::log::info!("copy result: {:?}", copy_result);

            // tide::log::info!("body: {}", body);
            todo!()
        }
        None => Ok("Token expired or invalid".into()),
    }
}

async fn gen_token_post(mut req: Request<Arc<AppState>>) -> tide::Result<Response> {
    let token_form: TokenForm = req.body_form().await?;
    let db = &req.state().db;
    let now = chrono::Utc::now().naive_utc();
    let token_expires_at = now + chrono::Duration::hours(token_form.token_valid_for as i64);
    let content_expires_at = token_form
        .content_expires_in_hours
        .map(|h| now + chrono::Duration::hours(h as i64));
    tide::log::info!("token form is: {:?}", &token_form);
    let create_token = db::CreateToken {
        path: token_form.path,
        max_size_in_mb: token_form.max_size,
        token_expires_at,
        content_expires_at,
    };
    let token = db.create_token(create_token).await?;
    let url = format!("/f/{}", token.path);
    Ok(tide::Redirect::new(url).into())
}

#[derive(Debug)]
enum CopyResult {
    Ok(u64),
    Truncated,
}

// This whole thing is copied from
// https://docs.rs/async-std/1.9.0/src/async_std/io/copy.rs.html#48-95
// with some minor tweak to add the limit to the number of copied bytes.
async fn copy_limit<R, W>(
    reader: &mut R,
    writer: &mut W,
    limit: u64,
) -> async_std::io::Result<CopyResult>
where
    R: async_std::io::Read + Unpin + ?Sized,
    W: async_std::io::Write + Unpin + ?Sized,
{
    #[pin_project]
    struct CopyFuture<R, W> {
        #[pin]
        reader: R,
        #[pin]
        writer: W,
        amt: u64,
        limit: u64,
    }

    impl<R, W> Future for CopyFuture<R, W>
    where
        R: async_std::io::BufRead,
        W: async_std::io::Write + Unpin,
    {
        type Output = async_std::io::Result<CopyResult>;

        fn poll(self: Pin<&mut Self>, cx: &mut task::Context<'_>) -> Poll<Self::Output> {
            let mut this = self.project();
            loop {
                let buffer = futures_core::ready!(this.reader.as_mut().poll_fill_buf(cx))?;
                if buffer.is_empty() {
                    futures_core::ready!(this.writer.as_mut().poll_flush(cx))?;
                    return Poll::Ready(Ok(CopyResult::Ok(*this.amt)));
                }

                let i = futures_core::ready!(this.writer.as_mut().poll_write(cx, buffer))?;
                if i == 0 {
                    return Poll::Ready(Err(async_std::io::ErrorKind::WriteZero.into()));
                }
                let tmp: u64 = *this.amt;
                *this.amt += i as u64;

                if *this.amt > *this.limit {
                    let wtf = this.amt > this.limit;
                    println!(
                        "truncated with {} > {} ? {}",
                        this.amt,
                        this.limit,
                        wtf,
                    );
                    let x = tmp + buffer.len() as u64;
                    println!("but buffer: {:?}", x);
                    return Poll::Ready(Ok(CopyResult::Truncated));
                };
                this.reader.as_mut().consume(i);
            }
        }
    }

    let future = CopyFuture {
        reader: async_std::io::BufReader::new(reader),
        writer,
        amt: 0,
        limit,
    };
    // future.await.context(|| String::from("io::copy failed"))
    future.await
}

#[derive(Debug, Deserialize, Serialize)]
struct TokenForm {
    path: String,

    #[serde(
        rename = "max-size",
        deserialize_with = "deserialize_sentinel",
        default
    )]
    max_size: Option<u32>,

    #[serde(
        rename = "content-expires",
        deserialize_with = "deserialize_sentinel",
        default
    )]
    content_expires_in_hours: Option<u64>,
    #[serde(rename = "link-valid-for")]
    token_valid_for: u64,
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
#[derive(Deserialize, Serialize, Debug)]
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
