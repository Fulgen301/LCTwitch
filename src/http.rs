use std::convert::Infallible;
use std::sync::{Arc};

use serde::{Deserialize, Serialize};
use serde_repr::*;
use tokio::sync::oneshot::Receiver;
use warp::{self, hyper::StatusCode, reject, reply, Reply, Filter, Rejection};

use crate::LCTwitch;

#[derive(Clone, Serialize, Deserialize, Debug)]
pub struct Script {
    pub script: String
}

#[derive(Clone, Copy, Serialize, Deserialize, Debug)]
struct EmptyObject {}

#[derive(Clone, Serialize, Deserialize, Debug)]
pub struct Error {
    pub code: ErrorCode,
    pub message: String
}

impl From<ErrorCode> for Error {
    fn from(value: ErrorCode) -> Self {
        Error {
            code: value,
            message: value.to_string()
        }
    }
}

impl From<String> for Error {
    fn from(value: String) -> Self {
        Error {
            code: ErrorCode::InternalServerError,
            message: value
        }
    }
}

impl warp::reject::Reject for Error {}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.message)
    }
}

#[derive(Clone, Serialize, Deserialize, Debug)]
pub struct ScriptReply {
    pub result: String
}


#[repr(u8)]
#[derive(Clone, Copy, Serialize_repr, Deserialize_repr, Debug)]
pub enum ErrorCode {
    NoDebugActive,
    NoScenario,
    NotHost,
    NoScriptingInReplays,
    LeagueActive,
    ScriptParseError,
    InternalServerError
}

impl std::fmt::Display for ErrorCode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NoDebugActive => write!(f, "Debug mode has been disabled"),
            Self::NoScenario => write!(f, "No scenario running"),
            Self::NotHost => write!(f, "Not host"),
            Self::NoScriptingInReplays => write!(f, "Scripting in replays is disabled"),
            Self::LeagueActive => write!(f, "Scripting in league games is not allowed"),
            Self::ScriptParseError => write!(f, "Parse error"),
            Self::InternalServerError => write!(f, "Internal server error")
        }
    }
}

impl From<ErrorCode> for StatusCode {
    fn from(value: ErrorCode) -> Self {
        match value {
            ErrorCode::NoDebugActive | ErrorCode::NotHost | ErrorCode::NoScenario | ErrorCode::NoScriptingInReplays => StatusCode::FORBIDDEN,
            ErrorCode::ScriptParseError => StatusCode::UNPROCESSABLE_ENTITY,
            _ => StatusCode::INTERNAL_SERVER_ERROR
        }
    }
}

impl warp::reject::Reject for ErrorCode {}


async fn post_script(script: Script, instance: Arc<LCTwitch>) -> Result<impl Reply, Rejection> {
    instance.script.run_script(&instance, script.script.as_str())
        .await
        .map_or_else(
            |e| Err(reject::custom(Error::from(e.to_string()))),
            |result| Ok(reply::json(&ScriptReply { result }))
        )
}

async fn handle_rejection(err: Rejection) -> Result<impl Reply, Infallible> {
    let reply_error_from_code = |code: ErrorCode| Ok(reply::with_status(reply::json(&Error::from(code)), code.into()));

    if let Some(code) = err.find::<ErrorCode>() {
        reply_error_from_code(*code)
    }
    else if let Some(error) = err.find::<Error>() {
        Ok(reply::with_status(reply::json(error), error.code.into()))
    }
    else {
        Ok(reply::with_status(reply::json(&EmptyObject{}), StatusCode::INTERNAL_SERVER_ERROR))
    }
}

pub async fn run_server(instance: Arc<LCTwitch>, rx: Receiver<()>) {
    let instance_clone = instance.clone();
    let instance_filter = warp::any().map(move || instance_clone.clone());

    let route = warp::path("v1")
        .and(warp::path("action"))
            .and(warp::path("script"))
                .and(warp::post())
                .and(warp::body::json())
                .and(instance_filter)
                .and_then(post_script)
                .recover(handle_rejection);

                
    let (_, server) = warp::serve(route)
        .bind_with_graceful_shutdown(([127, 0, 0, 1], instance.config().port()), async move {
            rx.await.unwrap();
        });

    server.await
}