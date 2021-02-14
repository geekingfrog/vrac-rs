use std::collections::HashMap;
use warp::{reject, reject::Reject, Filter, Rejection, Reply};
use chrono::Duration;

#[derive(Debug)]
pub struct Token {
    pub path: String,
    pub max_size_in_mb: Quantity<u32>,
    pub content_expires_after: Duration,
    pub token_valid_for: Quantity<Duration>,
}

/// isomorphic to Option<T>, represent a quantity that may not have an upper bound
#[derive(Debug)]
pub enum Quantity<T> {
    Bounded(T),
    Unbounded,
}

impl<T> From<Quantity<T>> for Option<T> {
    fn from(qte: Quantity<T>) -> Self {
        match qte {
            Quantity::Unbounded => None,
            Quantity::Bounded(x) => Some(x),
        }
    }
}

#[derive(Debug)]
pub struct Invalid {
    message: String,
}

impl Reject for Invalid {}

pub fn parse_token() -> impl Filter<Extract = (Token,), Error = Rejection> + Copy {
    warp::body::form().and_then(|form: HashMap<String, String>| async move {
        parse_token_(form).map_err(|err| warp::reject::custom(Invalid { message: err }))
    })
}

fn parse_token_(form: HashMap<String, String>) -> Result<Token, String> {
    let path = form
        .get("path")
        .map(|s| s.to_owned())
        .ok_or("Missing path".to_string())?;

    let max_size_in_mb = form
        .get("max-size")
        .ok_or("Missing max-size".to_string())
        .and_then(parse_size)?;

    let content_expires_after = form
        .get("expires")
        .ok_or("Missing expires".to_string())
        .and_then(parse_duration)?;

    let token_valid_for = form
        .get("valid-for")
        .ok_or("Missing valid-for".to_string())
        .and_then(parse_valid_for)?;

    Ok(Token {
        path,
        max_size_in_mb,
        content_expires_after,
        token_valid_for,
    })
}

fn parse_size(raw: &String) -> Result<Quantity<u32>, String> {
    if raw == "Unlimited" {
        Ok(Quantity::Unbounded)
    } else if raw == "1MB" {
        Ok(Quantity::Bounded(1))
    } else if raw == "10MB" {
        Ok(Quantity::Bounded(10))
    } else if raw == "200MB" {
        Ok(Quantity::Bounded(200))
    } else if raw == "1GB" {
        Ok(Quantity::Bounded(1024))
    } else if raw == "5GB" {
        Ok(Quantity::Bounded(5 * 1024))
    } else {
        Err(format!("cannot parse quantity<u32> from {}", raw))
    }
}

fn parse_duration(raw: &String) -> Result<Duration, String> {
    if raw == "1Hour" {
        Ok(Duration::hours(1))
    } else if raw == "1Day" {
        Ok(Duration::days(1))
    } else if raw == "1Week" {
        Ok(Duration::weeks(1))
    } else if raw == "1Month" {
        Ok(Duration::days(31))
    } else {
        Err(format!("cannot parse Duration from {}", raw))
    }
}

fn parse_valid_for(raw: &String) -> Result<Quantity<Duration>, String> {
    if raw == "DoesntExpire" {
        Ok(Quantity::Unbounded)
    } else {
        parse_duration(raw).map(Quantity::Bounded)
    }
}
