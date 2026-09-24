//! One WebRTC connection to one peer, carrying JSON over a data channel.
//!
//! The awkward parts, both forced by the environment rather than chosen:
//!
//! `web-sys` exposes WebRTC as callbacks, so every handler is a `Closure` that has
//! to outlive the call that installed it. They are kept in [`Peer`] rather than
//! leaked with `forget`, so dropping a `Peer` drops its handlers.
//!
//! The signalling server takes one complete offer and does not trickle candidates,
//! so both sides wait for ICE gathering to finish before handing their description
//! over. That costs a second or so at connect time and is the price of a signalling
//! server that is a plain mailbox.

use std::{cell::RefCell, rc::Rc};

use wasm_bindgen::{closure::Closure, JsCast, JsValue};
use wasm_bindgen_futures::JsFuture;
use web_sys::{
    MessageEvent, RtcConfiguration, RtcDataChannel, RtcDataChannelEvent, RtcDataChannelState,
    RtcIceGatheringState, RtcPeerConnection, RtcPeerConnectionIceEvent, RtcSdpType,
    RtcSessionDescriptionInit,
};

use crate::signal::{Description, IceServer};

/// The channel label both sides must agree on, and a version marker with it: a peer
/// running older code will not match, which is a clearer failure than exchanging
/// messages neither side can parse.
pub const CHANNEL: &str = "waldo-v1";

pub struct Peer {
    conn: RtcPeerConnection,
    channel: RefCell<Option<RtcDataChannel>>,
    /// Handlers, kept alive for as long as the peer is.
    keep: RefCell<Vec<Box<dyn std::any::Any>>>,
    on_message: Rc<RefCell<Option<Box<dyn Fn(String)>>>>,
    on_close: Rc<RefCell<Option<Box<dyn Fn()>>>>,
}

fn err(e: JsValue) -> String {
    e.as_string()
        .or_else(|| e.dyn_ref::<js_sys::Error>().map(|e| String::from(e.message())))
        .unwrap_or_else(|| "webrtc failed".into())
}

fn ice_config(servers: &[IceServer]) -> RtcConfiguration {
    let list = js_sys::Array::new();
    for s in servers {
        let entry = js_sys::Object::new();
        let urls = js_sys::Array::new();
        for u in &s.urls {
            urls.push(&JsValue::from_str(u));
        }
        let _ = js_sys::Reflect::set(&entry, &"urls".into(), &urls);
        if let Some(u) = &s.username {
            let _ = js_sys::Reflect::set(&entry, &"username".into(), &JsValue::from_str(u));
        }
        if let Some(c) = &s.credential {
            let _ = js_sys::Reflect::set(&entry, &"credential".into(), &JsValue::from_str(c));
        }
        list.push(&entry);
    }
    let cfg = RtcConfiguration::new();
    cfg.set_ice_servers(&list);
    cfg
}

impl Peer {
    fn wrap(conn: RtcPeerConnection) -> Rc<Self> {
        Rc::new(Peer {
            conn,
            channel: RefCell::new(None),
            keep: RefCell::new(Vec::new()),
            on_message: Rc::new(RefCell::new(None)),
            on_close: Rc::new(RefCell::new(None)),
        })
    }

    /// Called for every message the peer sends. Replaces any previous handler.
    pub fn on_message(&self, f: impl Fn(String) + 'static) {
        *self.on_message.borrow_mut() = Some(Box::new(f));
    }

    /// Called once when the channel closes or the connection fails.
    pub fn on_close(&self, f: impl Fn() + 'static) {
        *self.on_close.borrow_mut() = Some(Box::new(f));
    }

    pub fn is_open(&self) -> bool {
        self.channel
            .borrow()
            .as_ref()
            .map(|c| c.ready_state() == RtcDataChannelState::Open)
            .unwrap_or(false)
    }

    /// Send one message. Returns false if the channel is not open, which a caller
    /// should treat as the peer being gone rather than retrying.
    pub fn send(&self, text: &str) -> bool {
        match self.channel.borrow().as_ref() {
            Some(c) if c.ready_state() == RtcDataChannelState::Open => {
                c.send_with_str(text).is_ok()
            }
            _ => false,
        }
    }

    pub fn close(&self) {
        if let Some(c) = self.channel.borrow().as_ref() {
            c.close();
        }
        self.conn.close();
    }

    fn attach_channel(self: &Rc<Self>, channel: RtcDataChannel) {
        let on_message = self.on_message.clone();
        let msg_cb = Closure::<dyn FnMut(MessageEvent)>::new(move |ev: MessageEvent| {
            if let Some(text) = ev.data().as_string() {
                if let Some(f) = on_message.borrow().as_ref() {
                    f(text);
                }
            }
        });
        channel.set_onmessage(Some(msg_cb.as_ref().unchecked_ref()));

        let on_close = self.on_close.clone();
        let close_cb = Closure::<dyn FnMut()>::new(move || {
            if let Some(f) = on_close.borrow().as_ref() {
                f();
            }
        });
        channel.set_onclose(Some(close_cb.as_ref().unchecked_ref()));

        self.keep.borrow_mut().push(Box::new(msg_cb));
        self.keep.borrow_mut().push(Box::new(close_cb));
        *self.channel.borrow_mut() = Some(channel);
    }

    /// Resolves once ICE gathering has finished, because the signalling server takes
    /// one complete description rather than a stream of candidates.
    async fn gathered(&self) -> Result<(), String> {
        if self.conn.ice_gathering_state() == RtcIceGatheringState::Complete {
            return Ok(());
        }
        let conn = self.conn.clone();
        let promise = js_sys::Promise::new(&mut |resolve, _reject| {
            let conn2 = conn.clone();
            let cb = Closure::<dyn FnMut(RtcPeerConnectionIceEvent)>::new(
                move |ev: RtcPeerConnectionIceEvent| {
                    // a null candidate is the end of gathering
                    if ev.candidate().is_none()
                        || conn2.ice_gathering_state() == RtcIceGatheringState::Complete
                    {
                        let _ = resolve.call0(&JsValue::NULL);
                    }
                },
            );
            conn.set_onicecandidate(Some(cb.as_ref().unchecked_ref()));
            cb.forget(); // resolved at most once, and the promise owns the rest
        });
        JsFuture::from(promise).await.map(|_| ()).map_err(err)
    }

    fn local_description(&self) -> Result<Description, String> {
        let d = self.conn.local_description().ok_or("no local description")?;
        Ok(Description { kind: sdp_kind(d.type_()), sdp: d.sdp() })
    }
}

fn sdp_kind(t: RtcSdpType) -> String {
    match t {
        RtcSdpType::Offer => "offer",
        RtcSdpType::Answer => "answer",
        RtcSdpType::Pranswer => "pranswer",
        _ => "rollback",
    }
    .to_string()
}

fn description_init(d: &Description) -> Result<RtcSessionDescriptionInit, String> {
    let kind = match d.kind.as_str() {
        "offer" => RtcSdpType::Offer,
        "answer" => RtcSdpType::Answer,
        "pranswer" => RtcSdpType::Pranswer,
        other => return Err(format!("unknown sdp type {other}")),
    };
    let init = RtcSessionDescriptionInit::new(kind);
    init.set_sdp(&d.sdp);
    Ok(init)
}

/// Build the joining half: create the channel, offer, and wait for gathering.
/// Returns the peer and the offer to hand to the signalling server.
pub async fn offer(servers: &[IceServer]) -> Result<(Rc<Peer>, Description), String> {
    let conn = RtcPeerConnection::new_with_configuration(&ice_config(servers)).map_err(err)?;
    let peer = Peer::wrap(conn);

    let channel = peer.conn.create_data_channel(CHANNEL);
    peer.attach_channel(channel);

    let offer = JsFuture::from(peer.conn.create_offer()).await.map_err(err)?;
    let offer: RtcSessionDescriptionInit = offer.unchecked_into();
    JsFuture::from(peer.conn.set_local_description(&offer)).await.map_err(err)?;
    peer.gathered().await?;

    let description = peer.local_description()?;
    Ok((peer, description))
}

/// Build the hosting half for one joiner: take their offer, answer it, and wait for
/// gathering. The joiner creates the channel, so this side waits for `ondatachannel`.
pub async fn answer(
    servers: &[IceServer],
    their_offer: &Description,
) -> Result<(Rc<Peer>, Description), String> {
    let conn = RtcPeerConnection::new_with_configuration(&ice_config(servers)).map_err(err)?;
    let peer = Peer::wrap(conn);

    let weak = Rc::downgrade(&peer);
    let chan_cb = Closure::<dyn FnMut(RtcDataChannelEvent)>::new(move |ev: RtcDataChannelEvent| {
        if let Some(peer) = weak.upgrade() {
            peer.attach_channel(ev.channel());
        }
    });
    peer.conn.set_ondatachannel(Some(chan_cb.as_ref().unchecked_ref()));
    peer.keep.borrow_mut().push(Box::new(chan_cb));

    let remote = description_init(their_offer)?;
    JsFuture::from(peer.conn.set_remote_description(&remote)).await.map_err(err)?;
    let answer = JsFuture::from(peer.conn.create_answer()).await.map_err(err)?;
    let answer: RtcSessionDescriptionInit = answer.unchecked_into();
    JsFuture::from(peer.conn.set_local_description(&answer)).await.map_err(err)?;
    peer.gathered().await?;

    let description = peer.local_description()?;
    Ok((peer, description))
}

/// Finish the joining half once the host's answer arrives.
pub async fn accept_answer(peer: &Rc<Peer>, their_answer: &Description) -> Result<(), String> {
    let remote = description_init(their_answer)?;
    JsFuture::from(peer.conn.set_remote_description(&remote)).await.map_err(err)?;
    Ok(())
}
