//! Client for the icebreaker signalling server: room codes and SDP handover.
//!
//! Eight endpoints, all JSON over HTTP, no WebSocket. This module knows nothing
//! about the game; it moves SDP blobs and returns seat numbers. See `peer` for the
//! connections built from what it fetches.
//!
//! Nothing calls this yet: the host and joiner drivers that consume it are the next
//! layer, and landing the transport on its own keeps that diff readable.
#![allow(dead_code)]

use serde::{Deserialize, Serialize};
use wasm_bindgen::{JsCast, JsValue};
use wasm_bindgen_futures::JsFuture;
use web_sys::{Request, RequestInit, Response};

/// The namespace this game occupies on a shared signalling server.
pub const APP: &str = "waldo";

/// Base URL of the signalling server, compiled in. Falls back to the origin the
/// page came from, which is what a local `cargo run` wants.
pub fn base_url() -> String {
    let compiled = option_env!("SIGNAL_URL").unwrap_or("");
    if !compiled.is_empty() {
        return compiled.trim_end_matches('/').to_string();
    }
    web_sys::window()
        .and_then(|w| w.location().origin().ok())
        .unwrap_or_else(|| "http://localhost:8001".into())
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Description {
    #[serde(rename = "type")]
    pub kind: String,
    pub sdp: String,
}

#[derive(Deserialize, Debug)]
pub struct IceServer {
    pub urls: Vec<String>,
    #[serde(default)]
    pub username: Option<String>,
    #[serde(default)]
    pub credential: Option<String>,
}

#[derive(Deserialize, Debug)]
pub struct Hosted {
    pub code: String,
    pub max_joiners: u32,
    pub ice_servers: Vec<IceServer>,
}

#[derive(Deserialize, Debug)]
pub struct Joined {
    pub seat: u32,
    pub ice_servers: Vec<IceServer>,
}

#[derive(Deserialize, Debug)]
pub struct Offer {
    pub seat: u32,
    pub offer: Description,
}

#[derive(Debug)]
pub enum Error {
    /// The server answered, with this status and message.
    Refused { status: u16, message: String },
    /// The request never completed, or the answer was not the shape expected.
    Transport(String),
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Error::Refused { message, .. } => write!(f, "{message}"),
            Error::Transport(e) => write!(f, "{e}"),
        }
    }
}

impl Error {
    /// Whether the room is gone, as opposed to a transient failure. The signalling
    /// server answers a wrong app key and an unknown code identically, on purpose.
    pub fn is_missing(&self) -> bool {
        matches!(self, Error::Refused { status: 404, .. })
    }
}

fn js_err(e: JsValue) -> Error {
    Error::Transport(
        e.as_string()
            .or_else(|| e.dyn_ref::<js_sys::Error>().map(|e| String::from(e.message())))
            .unwrap_or_else(|| "request failed".into()),
    )
}

/// One request. `None` body means GET. Returns the parsed body, or `None` for a
/// 204, which is how the server says "nothing new yet".
async fn call(path: &str, body: Option<String>) -> Result<Option<String>, Error> {
    let sep = if path.contains('?') { '&' } else { '?' };
    let url = format!("{}{}{}app={}", base_url(), path, sep, APP);

    let opts = RequestInit::new();
    opts.set_method(if body.is_some() { "POST" } else { "GET" });
    if let Some(b) = &body {
        opts.set_body(&JsValue::from_str(b));
    }
    let request = Request::new_with_str_and_init(&url, &opts).map_err(js_err)?;
    if body.is_some() {
        request.headers().set("content-type", "application/json").map_err(js_err)?;
    }

    let window = web_sys::window().ok_or_else(|| Error::Transport("no window".into()))?;
    let res = JsFuture::from(window.fetch_with_request(&request)).await.map_err(js_err)?;
    let res: Response = res.dyn_into().map_err(js_err)?;
    let status = res.status();

    if status == 204 {
        return Ok(None);
    }
    let text = JsFuture::from(res.text().map_err(js_err)?).await.map_err(js_err)?;
    let text = text.as_string().unwrap_or_default();
    if !res.ok() {
        let message = serde_json::from_str::<serde_json::Value>(&text)
            .ok()
            .and_then(|v| v["error"].as_str().map(String::from))
            .unwrap_or_else(|| format!("signalling server returned {status}"));
        return Err(Error::Refused { status, message });
    }
    Ok(Some(text))
}

fn parse<T: for<'de> Deserialize<'de>>(body: Option<String>) -> Result<T, Error> {
    let body = body.ok_or_else(|| Error::Transport("expected a body, got 204".into()))?;
    serde_json::from_str(&body).map_err(|e| Error::Transport(e.to_string()))
}

/// Reserve a room. The code is what a player reads out loud.
pub async fn host() -> Result<Hosted, Error> {
    parse(call("/host", Some("{}".into())).await?)
}

/// Leave an offer for the host of `code` and take the seat it hands back.
pub async fn join(code: &str, offer: &Description) -> Result<Joined, Error> {
    let body = serde_json::json!({ "code": code, "offer": offer }).to_string();
    parse(call("/join", Some(body)).await?)
}

/// Collect offers left since the last call. Each is handed over exactly once, so a
/// dropped response loses that joiner and they have to retry.
pub async fn offers(code: &str) -> Result<Vec<Offer>, Error> {
    match call(&format!("/offers/{code}"), None).await? {
        None => Ok(Vec::new()),
        Some(body) => {
            #[derive(Deserialize)]
            struct Batch {
                offers: Vec<Offer>,
            }
            let batch: Batch =
                serde_json::from_str(&body).map_err(|e| Error::Transport(e.to_string()))?;
            Ok(batch.offers)
        }
    }
}

pub async fn answer(code: &str, seat: u32, answer: &Description) -> Result<(), Error> {
    let body = serde_json::json!({ "code": code, "seat": seat, "answer": answer }).to_string();
    call("/answer", Some(body)).await.map(|_| ())
}

/// Collect the host's answer for our seat. `None` means it has not arrived yet.
pub async fn take_answer(code: &str, seat: u32) -> Result<Option<Description>, Error> {
    match call(&format!("/answer/{code}/{seat}"), None).await? {
        None => Ok(None),
        Some(body) => {
            #[derive(Deserialize)]
            struct Wrapper {
                answer: Description,
            }
            let w: Wrapper =
                serde_json::from_str(&body).map_err(|e| Error::Transport(e.to_string()))?;
            Ok(Some(w.answer))
        }
    }
}

/// Drop the room. Only when the lobby breaks up, not when the match starts: the
/// room has to stay reachable so a dropped player can signal their way back in.
pub async fn close(code: &str) -> Result<(), Error> {
    let body = serde_json::json!({ "code": code }).to_string();
    call("/close", Some(body)).await.map(|_| ())
}
