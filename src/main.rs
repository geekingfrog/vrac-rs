use warp::{Filter, Rejection, Reply};
extern crate pretty_env_logger;
#[macro_use]
extern crate log;
extern crate handlebars;
use http::status::StatusCode;
use serde;
use std::collections::BTreeMap;
use std::convert::Infallible;
use std::sync::Arc;

use bytes;
use handlebars::Handlebars;

use vrac::db::{self, VracPersistence};

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
    service
        .create_token(&db::CreateToken {
            path: "foo".to_string(),
            max_size_in_bytes: None,
            expires_at: None,
        })
        .unwrap();

    service
        .create_token(&db::CreateToken {
            path: "bar".to_string(),
            max_size_in_bytes: None,
            expires_at: None,
        })
        .unwrap();

    let service = warp::any().map(move || Arc::clone(&service));

    let mut hb = Handlebars::new();
    hb.register_template_file("base", "./templates/base.hbs")
        .unwrap();

    let hb = Arc::new(hb);

    // make it into a warp filter
    let hb = warp::any().map(move || Arc::clone(&hb));

    let hello = warp::get()
        .and(warp::path!("hello" / String))
        .and(service.clone())
        .and_then(gen_token)
        .and(hb.clone())
        .map(render_hbs);

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

    let body = warp::post()
        .and(warp::path!("upload"))
        .and(warp::body::content_length_limit(1024 * 1024 * 1024))
        .and(warp::body::bytes())
        .map(|bytes: bytes::Bytes| format!("got some bytes ({})\n", bytes.len()));

    warp::serve(hello.or(body).or(other).recover(handle_rejection))
        .run(([127, 0, 0, 1], 8888))
        .await;
}

async fn gen_token(
    path: String,
    db: Arc<vrac::db::DB>,
// ) -> impl Filter<Extract = (WithTemplate<impl serde::Serialize>,), Error = Rejection> {
) -> Result<WithTemplate<impl serde::Serialize>, warp::Rejection> {
// fn div_by() -> impl Filter<Extract = (NonZeroU16,), Error = Rejection> + Copy {
    let title = format!("Hello, {}!", path);
    let mut template_data = BTreeMap::new();
    template_data.insert("title", "dat title though".to_string());
    template_data.insert("body_title", title);

    let db_result = tokio::task::block_in_place(|| {
        db.create_token(&db::CreateToken {
            path: path.to_string(),
            max_size_in_bytes: None,
            expires_at: None,
        })
    });

    db_result?;

    Ok(WithTemplate {
        name: "base",
        value: template_data,
    })
}

async fn handle_rejection(err: Rejection) -> Result<impl warp::Reply, Infallible> {
    Ok(warp::reply::with_status(
        "coucou",
        StatusCode::INTERNAL_SERVER_ERROR,
    ))
}
