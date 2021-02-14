use http::uri::PathAndQuery;
use warp::{http::Uri, Filter, Rejection, Reply};
extern crate pretty_env_logger;
#[macro_use]
extern crate log;
extern crate handlebars;
use http::status::StatusCode;
use serde;
use std::collections::BTreeMap;
use std::convert::Infallible;
use std::sync::Arc;

use chrono::naive::{NaiveDate, NaiveDateTime, NaiveTime};
use chrono::{offset::Utc, DateTime, Duration};

use bytes;
use handlebars::Handlebars;

use vrac::api;
use vrac::db::{self, VracPersistence};
use vrac::errors;

struct WithTemplate<T: serde::Serialize> {
    name: &'static str,
    value: T,
}

fn render_hbs<T>(template: WithTemplate<T>, hbs: Arc<Handlebars>) -> impl warp::Reply
where
    T: serde::Serialize,
{
    let render = hbs
        .render(template.name, &template.value)
        .unwrap_or_else(|err| err.to_string());

    warp::reply::html(render)
}

#[tokio::main]
async fn main() {
    pretty_env_logger::init();

    let service = Arc::new(vrac::db::DB::new("./vrac.sqlite"));
    service.init_db().unwrap();

    // service
    //     .create_token(&db::CreateToken {
    //         path: "foo".to_string(),
    //         max_size_in_mb: None,
    //         expires_at: None,
    //     })
    //     .unwrap();
    //
    // service
    //     .create_token(&db::CreateToken {
    //         path: "bar".to_string(),
    //         max_size_in_mb: None,
    //         expires_at: None,
    //     })
    //     .unwrap();

    let service = warp::any().map(move || Arc::clone(&service));

    let mut hb = Handlebars::new();
    hb.register_template_file("base", "./templates/base.hbs")
        .unwrap();
    hb.register_template_file("gen_token", "./templates/gen_token.hbs")
        .unwrap();
    hb.register_template_file("upload_files", "./templates/upload_files.hbs")
        .unwrap();

    let hb = Arc::new(hb);

    // make it into a warp filter
    let hb = warp::any().map(move || Arc::clone(&hb));

    // let hello_test = warp::get()
    //     .and(warp::path!("hello" / String))
    //     .and(service.clone())
    //     .and_then(gen_token)
    //     .and(hb.clone())
    //     .map(render_hbs);

    let get_token = warp::get()
        .map(|| WithTemplate {
            name: "gen_token",
            value: (),
        })
        .and(hb.clone())
        .map(render_hbs);

    let post_token = warp::post()
        .and(warp::body::content_length_limit(1024))
        .and(api::parse_token())
        .and(service.clone())
        .and_then(create_token);

    let token_routes = warp::path!("gen")
        .and(warp::path::end())
        .and(get_token.or(post_token));

    let get_file = warp::get()
        .and(warp::path!(String))
        .and(warp::path::end())
        .and(service.clone())
        .and(hb.clone())
        .and_then(get_files_handler);

    let post_file = warp::post()
        .and(warp::path!(String))
        .and(warp::path::end())
        .and(service.clone())
        .and_then(upload_file_handler)
        .and(warp::header::<u64>("content-length"))
        .and_then(content_length_limit)
        .and(warp::body::stream())
        // .and_then(|db_token: db::Token| -> Result<db::Token, warp::Rejection> {
        //     Ok("coucou")
        // })
        .map(|_, _| "coucou");

    let file_routes = warp::path("f")
        .and(get_file.or(post_file))
        .with(warp::log("file_route"));

    // let file_routes = warp::get()
    //     .and(warp::path("f"))
    //     .and(warp::path!(String))
    //     .and(service.clone())
    //     .and_then(get_files)
    //     .with(warp::log("file_route"));

    let other = warp::get()
        .and(warp::path!("coucou"))
        .map(|| {
            let mut template_data = BTreeMap::new();
            template_data.insert("title", "dat title though".to_string());
            template_data.insert("body_title", "je suis charlie".to_string());
            WithTemplate {
                name: "base",
                value: template_data,
            }
        })
        .and(hb.clone())
        .map(render_hbs);

    let test_body = warp::post()
        .and(warp::path!("upload"))
        .and(warp::body::content_length_limit(1024 * 1024 * 1024))
        .and(warp::body::bytes())
        .map(|bytes: bytes::Bytes| format!("got some bytes ({})\n", bytes.len()));

    warp::serve(
        token_routes.or(file_routes).recover(handle_rejection),
        // test_body
        //     .or(other)
        //     .or(token_routes)
        //     .or(file_routes)
        //     .recover(handle_rejection),
    )
    .run(([127, 0, 0, 1], 8888))
    .await;
}

async fn create_token(
    token: vrac::api::Token,
    db: Arc<vrac::db::DB>,
) -> Result<impl Reply, warp::Rejection> {
    let db_token = to_db_token(token);
    tokio::task::block_in_place(|| db.create_token(&db_token))?;

    let uri = Uri::builder()
        .path_and_query(&format!("/f/{}", db_token.path)[..])
        .build()
        .unwrap();
    Ok(warp::redirect::redirect(uri))
}

async fn get_files_handler(
    token_path: String,
    db: Arc<vrac::db::DB>,
    hbs: Arc<Handlebars<'_>>,
) -> Result<impl Reply, warp::Rejection> {
    info!("Getting a token from path: {}", token_path.clone());

    let mb_token = tokio::task::block_in_place(|| db.get_valid_token_by_path(&token_path))?;
    match mb_token {
        None => {
            info!("Token not found for path: {}", token_path);
            Err(warp::reject::not_found())
        }
        Some(token) => match token.status {
            db::TokenStatus::Fresh => {
                let mut template_data = BTreeMap::new();
                template_data.insert(
                    "max_size_in_mb",
                    token.max_size_in_mb.unwrap_or(0).to_string(),
                );
                template_data.insert("form_action", format!("/f/{}", token.path));

                let template = WithTemplate {
                    name: "upload_files",
                    value: template_data,
                };
                Ok(render_hbs(template, hbs))
            }
            db::TokenStatus::Used => {
                let template = WithTemplate {
                    name: "get_files",
                    value: BTreeMap::new(),
                };
                Ok(render_hbs(template, hbs))
            }
            _ => {
                let err_msg = format!("Expected a valid token but got: {:?}", token);
                let err = errors::VracError::Panic(err_msg);
                Err(warp::reject::custom(errors::Error::AppError(err)))
            }
        },
    }
}

async fn upload_file_handler(
    token_path: String,
    db: Arc<vrac::db::DB>,
) -> Result<db::Token, warp::Rejection> {
    info!("Getting files for token: {}", token_path.clone());

    let mb_token = tokio::task::block_in_place(|| db.get_valid_token_by_path(&token_path))?;

    match mb_token {
        Some(db_token) => {
            Ok(db_token)
            // let uri = Uri::builder()
            //     .path_and_query(&format!("/f/{}", db_token.path)[..])
            //     .build()
            //     .unwrap();
            // Ok(Box::new(warp::redirect::redirect(uri)))
        }
        None => Err(warp::reject::not_found()),
    }
}

// lifted from warp::filters::body::content_length_limit
// since the filters cannot be dynamically added, and the limit depends on
// the token
async fn content_length_limit(
    db_token: db::Token,
    max_size_header: u64,
) -> Result<db::Token, warp::Rejection> {
    if let Some(limit) = db_token.max_size_in_mb {
        if max_size_header > limit as u64 {
            let err: vrac::errors::Error =
                vrac::errors::VracError::PayloadTooLarge(max_size_header).into();
            return Err(err.into());
        }
    };
    Ok(db_token)
}

async fn gen_token(
    path: String,
    db: Arc<vrac::db::DB>,
) -> Result<WithTemplate<impl serde::Serialize>, warp::Rejection> {
    let title = format!("Hello, {}!", path);
    let mut template_data = BTreeMap::new();
    template_data.insert("title", "dat title though".to_string());
    template_data.insert("body_title", title);

    // let db_result = tokio::task::block_in_place(|| {
    //     db.create_token(&db::CreateToken {
    //         path: path.to_string(),
    //         max_size_in_mb: None,
    //         content_expires_at: None,
    //     })
    // });
    //
    // db_result?;

    Ok(WithTemplate {
        name: "base",
        value: template_data,
    })
}

fn to_db_token(tok: api::Token) -> db::CreateToken {
    let now: NaiveDateTime = Utc::now().naive_utc();
    let expires: Option<Duration> = tok.token_valid_for.into();

    db::CreateToken {
        path: tok.path,
        max_size_in_mb: tok.max_size_in_mb.into(),
        token_expires_at: expires.map(|duration| now + duration),
        content_expires_at: now + tok.content_expires_after,
    }
}

async fn handle_rejection(err: Rejection) -> Result<impl warp::Reply, Infallible> {
    info!("handling error: {:?}", err);

    let (status_code, body) = if err.is_not_found() {
        (StatusCode::NOT_FOUND, "".to_string())
    } else if let Some(e) = err.find::<warp::reject::PayloadTooLarge>() {
        (
            StatusCode::PAYLOAD_TOO_LARGE,
            "payload too large".to_string(),
        )
    } else if let Some(e) = err.find::<vrac::api::Invalid>() {
        (StatusCode::BAD_REQUEST, format!("BAD_REQUEST: {:?}", e))
    } else if let Some(e) = err.find::<vrac::errors::Error>() {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("INTERNAL ERROR {:?}", e),
        )
    } else if let Some(e) = err.find::<warp::filters::body::BodyDeserializeError>() {
        (StatusCode::BAD_REQUEST, format!("BAD_REQUEST: {:?}", e))
    } else if let Some(_) = err.find::<warp::reject::MethodNotAllowed>() {
        (
            StatusCode::METHOD_NOT_ALLOWED,
            "METHOD_NOT_ALLOWED".to_string(),
        )
    } else {
        error!("unhandled rejection: {:?}", err);
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            "UNHANDLED_REJECTION".to_string(),
        )
    };

    Ok(warp::reply::with_status(body, status_code))
}
