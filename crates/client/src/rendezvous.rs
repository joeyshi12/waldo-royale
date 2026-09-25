//! Client for the icebreaker rendezvous server: room codes and SDP handover.
//!
//! One WebSocket, held open for as long as we are in a lobby. This module knows nothing
//! about the game; it moves SDP blobs and returns seat numbers. See `peer` for the
//! connections built from what it carries.
//!
//! Holding the connection is also what holds our place: a host hanging up ends its room
//! and a joiner hanging up gives its seat back.

use std::{
    any::Any,
    cell::{Cell, RefCell},
    rc::Rc,
};

use serde::{de::DeserializeOwned, Deserialize, Serialize};
use wasm_bindgen::{closure::Closure, JsCast, JsValue};
use wasm_bindgen_futures::JsFuture;
use web_sys::{MessageEvent, WebSocket};

/// The namespace this game occupies on a shared rendezvous server.
pub const APP: &str = "waldo";

const OPEN: &str = "\0open"; // not a message: the socket opening settles the same way
const HOSTED: &str = "hosted";
const JOINED: &str = "joined";
const SERVERS: &str = "ice_servers";
const ANSWER: &str = "answer";

const NO_ROOM: &str = "no_room";

/// Ours, not the server's, so both kinds of refusal travel through the same JSON.
const TRANSPORT: &str = "\0transport";

/// How many replies to keep that nobody has asked for yet. See `stash`.
const STASHED: usize = 8;

/// Base URL of the rendezvous server, compiled in. Falls back to the origin the
/// page came from, which is what a local static server wants.
pub fn base_url() -> String {
    let compiled = option_env!("RENDEZVOUS_URL").unwrap_or("");
    if !compiled.is_empty() {
        return compiled.trim_end_matches('/').to_string();
    }
    web_sys::window()
        .and_then(|w| w.location().origin().ok())
        .unwrap_or_else(|| "http://localhost:8001".into())
}

/// `RENDEZVOUS_URL` holds the http origin, so the scheme is swapped here.
fn socket_url() -> String {
    let base = base_url();
    let (scheme, host) = match () {
        _ if base.starts_with("https://") => ("wss://", base.trim_start_matches("https://")),
        _ if base.starts_with("http://") => ("ws://", base.trim_start_matches("http://")),
        // already a socket url, so it is left alone
        _ if base.starts_with("wss://") => ("wss://", base.trim_start_matches("wss://")),
        _ if base.starts_with("ws://") => ("ws://", base.trim_start_matches("ws://")),
        _ => ("ws://", base.as_str()),
    };
    format!("{scheme}{host}/ws?app={APP}")
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Description {
    #[serde(rename = "type")]
    pub kind: String,
    pub sdp: String,
}

#[derive(Deserialize, Clone, Debug)]
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
struct Joined {
    seat: u32,
    // The reply also carries ice_servers, but a joiner has already gathered candidates
    // by this point, so it arrives too late to be of use.
}

#[derive(Debug, Clone)]
pub enum Error {
    /// The server refused a message, naming one of its reasons.
    Refused { reason: String, message: String },
    /// The connection failed, or a message was not the shape expected.
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
    /// Whether the room is gone, as opposed to a transient failure. The rendezvous
    /// server answers a wrong app key and an unknown code identically, on purpose.
    pub fn is_missing(&self) -> bool {
        matches!(self, Error::Refused { reason, .. } if reason == NO_ROOM)
    }

    /// A parked caller is woken by a JS promise, which can only carry a `JsValue`, so a
    /// transport failure is dressed as the JSON refusal the server would have sent.
    fn wire(reason: &str, message: &str) -> String {
        serde_json::json!({"reason": reason, "message": message}).to_string()
    }

    fn parse(v: JsValue) -> Error {
        #[derive(Deserialize)]
        struct Refusal {
            reason: String,
            message: String,
        }
        let text = v.as_string().unwrap_or_default();
        match serde_json::from_str::<Refusal>(&text) {
            Ok(r) if r.reason == TRANSPORT => Error::Transport(r.message),
            Ok(r) => Error::Refused { reason: r.reason, message: r.message },
            Err(_) => Error::Transport("the lobby service connection failed".into()),
        }
    }
}

fn js_err(e: JsValue) -> Error {
    Error::Transport(
        e.as_string()
            .or_else(|| e.dyn_ref::<js_sys::Error>().map(|e| String::from(e.message())))
            .unwrap_or_else(|| "could not reach the lobby service".into()),
    )
}

/// One at a time is enough: a peer only waits for the next step of its own sequence.
struct Waiting {
    wants: &'static str,
    resolve: js_sys::Function,
    reject: js_sys::Function,
}

pub struct Rendezvous {
    socket: WebSocket,
    waiting: RefCell<Option<Waiting>>,
    /// Replies that arrived before anybody parked on them. See `stash`.
    arrived: RefCell<Vec<(String, String)>>,
    on_offer: RefCell<Option<Box<dyn Fn(u32, Description)>>>,
    on_lost: RefCell<Option<Box<dyn Fn(String)>>>,
    on_refused: RefCell<Option<Box<dyn Fn(Error)>>>,
    lost: Cell<bool>,
    /// Event handlers, kept alive for as long as this is.
    keep: RefCell<Vec<Box<dyn Any>>>,
}

impl Rendezvous {
    pub async fn connect() -> Result<Rc<Rendezvous>, Error> {
        let socket = WebSocket::new(&socket_url()).map_err(js_err)?;
        let rv = Rc::new(Rendezvous {
            socket,
            waiting: RefCell::new(None),
            arrived: RefCell::new(Vec::new()),
            on_offer: RefCell::new(None),
            on_lost: RefCell::new(None),
            on_refused: RefCell::new(None),
            lost: Cell::new(false),
            keep: RefCell::new(Vec::new()),
        });
        rv.attach();
        // the socket cannot have opened yet: nothing above yielded
        rv.expect_raw(OPEN).await?;
        Ok(rv)
    }

    /// Reserve a room. The code is what a player reads out loud.
    pub async fn host(self: &Rc<Self>) -> Result<Hosted, Error> {
        self.request(serde_json::json!({"type": "host"}), HOSTED).await
    }

    /// An answer names its own seat, so the one returned here is free to ignore.
    pub async fn join(self: &Rc<Self>, code: &str, offer: &Description) -> Result<u32, Error> {
        let joined: Joined = self
            .request(serde_json::json!({"type": "join", "code": code, "offer": offer}), JOINED)
            .await?;
        Ok(joined.seat)
    }

    /// Needed before an offer can be built, so it cannot wait for `join`.
    pub async fn ice(self: &Rc<Self>) -> Result<Vec<IceServer>, Error> {
        #[derive(Deserialize)]
        struct Servers {
            ice_servers: Vec<IceServer>,
        }
        let s: Servers = self.request(serde_json::json!({"type": "ice"}), SERVERS).await?;
        Ok(s.ice_servers)
    }

    /// The deadline is the only way a joiner tells a host that has gone quiet from one
    /// still gathering candidates.
    pub async fn answered(self: &Rc<Self>, within_ms: i32) -> Result<Description, Error> {
        #[derive(Deserialize)]
        struct Answered {
            answer: Description,
        }
        if let Some(text) = self.unstash(ANSWER) {
            return Ok(decode::<Answered>(&text)?.answer);
        }
        let promise = self.park(ANSWER);
        self.deadline(ANSWER, within_ms, "the host did not answer");
        let raw = JsFuture::from(promise).await.map_err(Error::parse)?;
        Ok(decode::<Answered>(&raw.as_string().unwrap_or_default())?.answer)
    }

    /// A refusal arrives at [`Rendezvous::on_refused`] instead, because by then the host is
    /// on to the next offer.
    pub fn answer(&self, seat: u32, answer: &Description) -> Result<(), Error> {
        self.send(&serde_json::json!({"type": "answer", "seat": seat, "answer": answer}))
    }

    pub fn on_offer(&self, f: impl Fn(u32, Description) + 'static) {
        *self.on_offer.borrow_mut() = Some(Box::new(f));
    }

    /// Fires once. Nothing arrives after it.
    pub fn on_lost(&self, f: impl Fn(String) + 'static) {
        *self.on_lost.borrow_mut() = Some(Box::new(f));
    }

    /// Something nobody was waiting on was refused, in practice an answer that did not
    /// land. The connection is still up.
    pub fn on_refused(&self, f: impl Fn(Error) + 'static) {
        *self.on_refused.borrow_mut() = Some(Box::new(f));
    }

    /// Only when the lobby breaks up, not when the match starts: the room has to stay
    /// reachable for a dropped player to signal their way back in.
    pub fn close_room(&self) {
        let _ = self.send(&serde_json::json!({"type": "close"}));
    }

    fn attach(self: &Rc<Self>) {
        let me = Rc::downgrade(self);
        let on_message = Closure::<dyn FnMut(MessageEvent)>::new(move |ev: MessageEvent| {
            if let (Some(rv), Some(text)) = (me.upgrade(), ev.data().as_string()) {
                rv.deliver(&text);
            }
        });
        self.socket.set_onmessage(Some(on_message.as_ref().unchecked_ref()));

        let me = Rc::downgrade(self);
        let on_open = Closure::<dyn FnMut()>::new(move || {
            if let Some(rv) = me.upgrade() {
                rv.settle(OPEN, "{}");
            }
        });
        self.socket.set_onopen(Some(on_open.as_ref().unchecked_ref()));

        // onerror carries nothing a person can use, so both endings say the same thing
        let me = Rc::downgrade(self);
        let on_close = Closure::<dyn FnMut()>::new(move || {
            if let Some(rv) = me.upgrade() {
                rv.lose("the connection to the lobby service dropped");
            }
        });
        self.socket.set_onclose(Some(on_close.as_ref().unchecked_ref()));
        self.socket.set_onerror(Some(on_close.as_ref().unchecked_ref()));

        let mut keep = self.keep.borrow_mut();
        keep.push(Box::new(on_message));
        keep.push(Box::new(on_open));
        keep.push(Box::new(on_close));
    }

    fn deliver(self: &Rc<Self>, text: &str) {
        #[derive(Deserialize)]
        struct Tag {
            #[serde(rename = "type")]
            kind: String,
        }
        let Ok(tag) = serde_json::from_str::<Tag>(text) else {
            return; // not one of ours
        };
        match tag.kind.as_str() {
            "closed" => {
                #[derive(Deserialize)]
                struct Closed {
                    reason: String,
                }
                let why = serde_json::from_str::<Closed>(text)
                    .map(|c| c.reason)
                    .unwrap_or_else(|_| "the lobby closed".into());
                self.lose(&why);
            }
            "error" => self.refuse(text),
            "offer" => {
                #[derive(Deserialize)]
                struct Offered {
                    seat: u32,
                    offer: Description,
                }
                if let Ok(o) = serde_json::from_str::<Offered>(text) {
                    if let Some(f) = self.on_offer.borrow().as_ref() {
                        f(o.seat, o.offer);
                    }
                }
            }
            kind => {
                if !self.settle(kind, text) {
                    self.stash(kind, text);
                }
            }
        }
    }

    /// Send a message and wait for the one reply it has.
    async fn request<T: DeserializeOwned>(
        self: &Rc<Self>,
        message: serde_json::Value,
        wants: &'static str,
    ) -> Result<T, Error> {
        let raw = {
            let promise = self.park(wants);
            if let Err(e) = self.send(&message) {
                self.waiting.borrow_mut().take();
                return Err(e);
            }
            JsFuture::from(promise).await.map_err(Error::parse)?
        };
        decode(&raw.as_string().unwrap_or_default())
    }

    async fn expect_raw(self: &Rc<Self>, wants: &'static str) -> Result<String, Error> {
        if let Some(text) = self.unstash(wants) {
            return Ok(text);
        }
        let promise = self.park(wants);
        let raw = JsFuture::from(promise).await.map_err(Error::parse)?;
        Ok(raw.as_string().unwrap_or_default())
    }

    /// Arrange for a parked reply to fail if it has not arrived in time.
    fn deadline(self: &Rc<Self>, wants: &'static str, within_ms: i32, why: &'static str) {
        let me = Rc::downgrade(self);
        let fire = Closure::once_into_js(move || {
            if let Some(rv) = me.upgrade() {
                // harmless if the reply has since arrived, or if something else is parked
                let waiting = matches!(rv.waiting.borrow().as_ref(), Some(w) if w.wants == wants);
                if waiting {
                    rv.refuse(&Error::wire(TRANSPORT, why));
                }
            }
        });
        if let Some(window) = web_sys::window() {
            let _ = window.set_timeout_with_callback_and_timeout_and_arguments_0(
                fire.unchecked_ref(),
                within_ms,
            );
        }
    }

    /// The executor runs before this returns, so the slot is in place before anything yields.
    fn park(self: &Rc<Self>, wants: &'static str) -> js_sys::Promise {
        let me = self.clone();
        js_sys::Promise::new(&mut |resolve, reject| {
            *me.waiting.borrow_mut() = Some(Waiting { wants, resolve, reject });
        })
    }

    fn settle(&self, kind: &str, text: &str) -> bool {
        let woken = {
            let mut slot = self.waiting.borrow_mut();
            match slot.as_ref() {
                Some(w) if w.wants == kind => slot.take(),
                _ => None,
            }
        };
        match woken {
            Some(w) => {
                let _ = w.resolve.call1(&JsValue::NULL, &JsValue::from_str(text));
                true
            }
            None => false,
        }
    }

    /// The server sends the host's answer as soon as the host does, which can in principle
    /// beat the `joined` a joiner is still waiting for, and a dropped answer is a connection
    /// that silently never happens.
    fn stash(&self, kind: &str, text: &str) {
        let mut arrived = self.arrived.borrow_mut();
        if arrived.len() >= STASHED {
            arrived.remove(0);
        }
        arrived.push((kind.to_string(), text.to_string()));
    }

    fn unstash(&self, kind: &str) -> Option<String> {
        let mut arrived = self.arrived.borrow_mut();
        let at = arrived.iter().position(|(k, _)| k == kind)?;
        Some(arrived.remove(at).1)
    }

    /// Refuse whatever is parked, or report it if nothing is.
    fn refuse(&self, wire: &str) {
        let woken = self.waiting.borrow_mut().take();
        match woken {
            Some(w) => {
                let _ = w.reject.call1(&JsValue::NULL, &JsValue::from_str(wire));
            }
            None => {
                if let Some(f) = self.on_refused.borrow().as_ref() {
                    f(Error::parse(JsValue::from_str(wire)));
                }
            }
        }
    }

    fn lose(&self, why: &str) {
        if self.waiting.borrow().is_some() {
            self.refuse(&Error::wire(TRANSPORT, why));
        }
        if self.lost.replace(true) {
            return;
        }
        if let Some(f) = self.on_lost.borrow().as_ref() {
            f(why.to_string());
        }
    }

    fn send(&self, message: &serde_json::Value) -> Result<(), Error> {
        self.socket.send_with_str(&message.to_string()).map_err(js_err)
    }
}

impl Drop for Rendezvous {
    /// Hanging up is how a peer leaves, so the server does not wait on a closed tab.
    fn drop(&mut self) {
        let _ = self.socket.close();
    }
}

fn decode<T: DeserializeOwned>(text: &str) -> Result<T, Error> {
    serde_json::from_str(text).map_err(|e| Error::Transport(e.to_string()))
}
