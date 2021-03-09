use handlebars::Handlebars;
use http_types::{convert::Deserialize, mime};
use serde::de::{self, Visitor};
use std::sync::Arc;
use tide::prelude::*;
use tide::{Body, Request, Response, ResponseBuilder};

#[async_std::main]
async fn main() -> tide::Result<()> {
    tide::log::start();
    let mut app = tide::new();

    let mut hb = Handlebars::new();
    hb.register_template_file("base", "./templates/base.hbs")
        .unwrap();
    hb.register_template_file("gen_token", "./templates/gen_token.hbs")
        .unwrap();
    hb.register_template_file("upload_files", "./templates/upload_files.hbs")
        .unwrap();

    let hb = Arc::new(hb);

    app.at("/").get(root);
    app.at("/gen")
        .get(move |req| {
            let hb = hb.clone();
            async move { gen_token_get(&hb, req).await }
        })
        .post(gen_token_post);

    app.listen("127.0.0.1:8888").await?;
    Ok(())
}

async fn root(mut _req: Request<()>) -> tide::Result<String> {
    Ok("coucou".to_string())
}

async fn gen_token_get(hbs: &Arc<Handlebars<'_>>, mut _req: Request<()>) -> tide::Result<Response> {
    let rendered = hbs
        .render("gen_token", &())
        .unwrap_or_else(|err| err.to_string());
    let resp = Response::builder(200)
        .content_type(mime::HTML)
        .body(rendered)
        .build();
    Ok(resp)
}

async fn gen_token_post(mut req: Request<()>) -> tide::Result<Response> {
    // println!("raw body: {}", req.body_string().await?);
    // path=coucou&max-size=10MB&expires=1Day&valid-for=1Day
    let token_form: TokenForm = req.body_form().await?;
    println!("got a token form: {:#?}", token_form);
    Ok("".into())
}

#[derive(Debug, Deserialize)]
struct TokenForm {
    path: String,
    #[serde(rename = "max-size")]
    max_size: MaxSize,
    expires: Expiration,
    #[serde(rename = "valid-for")]
    valid_for: String,
}

#[derive(Debug)]
struct MaxSize(Option<i64>);

struct MaxSizeVisitor;

impl<'de> Visitor<'de> for MaxSizeVisitor {
    type Value = MaxSize;

    fn expecting(&self, formatter: &mut std::fmt::Formatter) -> std::fmt::Result {
        formatter.write_str("a string like 1MB or 5GB or Unlimited")
    }

    fn visit_str<E>(self, value: &str) -> Result<Self::Value, E>
    where
        E: de::Error,
    {
        match value {
            "1MB" => Ok(MaxSize(Some(1))),
            "10MB" => Ok(MaxSize(Some(10))),
            "200MB" => Ok(MaxSize(Some(200))),
            "1GB" => Ok(MaxSize(Some(1024))),
            "5GB" => Ok(MaxSize(Some(5 * 1024))),
            x => Err(E::custom(format!("Unknown max size value: {}", x))),
        }
    }
}

impl<'de> serde::Deserialize<'de> for MaxSize {
    fn deserialize<D>(
        deserializer: D,
    ) -> std::result::Result<Self, <D as serde::Deserializer<'de>>::Error>
    where
        D: serde::Deserializer<'de>,
    {
        deserializer.deserialize_string(MaxSizeVisitor)
    }
}

#[derive(Debug)]
struct VracDuration(chrono::Duration);

#[derive(Debug)]
struct Expiration(Option<VracDuration>);
struct VracDurationVisitor;
struct ExpirationVisitor;

impl<'de> Visitor<'de> for VracDurationVisitor {
    type Value = VracDuration;

    fn expecting(&self, formatter: &mut std::fmt::Formatter) -> std::fmt::Result {
        formatter.write_str("a string representing a duration like 1Hour")
    }

    fn visit_str<E>(self, value: &str) -> Result<Self::Value, E>
    where
        E: de::Error,
    {
        match value {
            "1Hour" => Ok(VracDuration(chrono::Duration::hours(1))),
            "1Day" => Ok(VracDuration(chrono::Duration::days(1))),
            "1Week" => Ok(VracDuration(chrono::Duration::weeks(1))),
            "1Month" => Ok(VracDuration(chrono::Duration::days(31))),
            x => Err(E::custom(format!("Invalid duration: {}", x))),
        }
    }
}

impl<'de> Visitor<'de> for ExpirationVisitor {
    type Value = Expiration;

    fn expecting(&self, formatter: &mut std::fmt::Formatter) -> std::fmt::Result {
        formatter.write_str("a string representing a duration like 1Hour or DoesntExpire")
    }

    fn visit_str<E>(self, value: &str) -> Result<Self::Value, E>
    where
        E: de::Error,
    {
        match value {
            "DoesntExpire" => Ok(Expiration(None)),
            _ => {
                let val = VracDurationVisitor.visit_str(value)?;
                Ok(Expiration(Some(val)))
            }
        }
    }
}

impl<'de> serde::Deserialize<'de> for Expiration {
    fn deserialize<D>(deserializer: D) -> std::result::Result<Self, <D as de::Deserializer<'de>>::Error>
    where
        D: de::Deserializer<'de>,
    {
        deserializer.deserialize_string(ExpirationVisitor)
    }
}
