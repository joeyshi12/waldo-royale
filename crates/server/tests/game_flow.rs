//! End-to-end test: boots the server binary and plays a full two-player
//! game over WebSocket.

use futures_util::{SinkExt, StreamExt};
use serde_json::{json, Value};
use std::process::{Child, Command};
use std::time::Duration;
use tokio::net::TcpStream;
use tokio_tungstenite::{connect_async, tungstenite::Message, MaybeTlsStream, WebSocketStream};

const PORT: u16 = 8991;

struct ServerGuard(Child);
impl Drop for ServerGuard {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

type Ws = WebSocketStream<MaybeTlsStream<TcpStream>>;

async fn connect() -> Ws {
    let (ws, _) = connect_async(format!("ws://127.0.0.1:{PORT}/ws")).await.expect("ws connect");
    ws
}

async fn send(ws: &mut Ws, v: Value) {
    ws.send(Message::Text(v.to_string().into())).await.unwrap();
}

/// Read messages until one with the given type arrives (5 s timeout).
async fn recv_type(ws: &mut Ws, ty: &str) -> Value {
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let msg = ws.next().await.expect("stream ended").expect("ws error");
            if let Message::Text(t) = msg {
                let v: Value = serde_json::from_str(&t).unwrap();
                if v["type"] == ty {
                    return v;
                }
                assert_ne!(v["type"], "error", "server error: {v}");
            }
        }
    })
    .await
    .unwrap_or_else(|_| panic!("timed out waiting for `{ty}`"))
}

async fn start_server() -> ServerGuard {
    let bin = env!("CARGO_BIN_EXE_waldo-server");
    let child = Command::new(bin)
        .env("PORT", PORT.to_string())
        .env("WALDO_INTERMISSION_MS", "200")
        .spawn()
        .expect("spawn server");
    for _ in 0..50 {
        if TcpStream::connect(("127.0.0.1", PORT)).await.is_ok() {
            return ServerGuard(child);
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    panic!("server did not start");
}

#[tokio::test]
async fn two_players_full_game() {
    let _guard = start_server().await;

    // --- lobby ---
    let mut alice = connect().await;
    send(&mut alice, json!({"type": "create", "name": "Alice"})).await;
    let lobby = recv_type(&mut alice, "lobby").await;
    let code = lobby["code"].as_str().unwrap().to_string();
    assert_eq!(code.len(), 4);
    assert_eq!(lobby["players"].as_array().unwrap().len(), 1);
    assert!(lobby["players"][0]["is_host"].as_bool().unwrap());

    let mut bob = connect().await;
    send(&mut bob, json!({"type": "join", "code": code, "name": "Bob"})).await;
    let lobby_b = recv_type(&mut bob, "lobby").await;
    assert_eq!(lobby_b["players"].as_array().unwrap().len(), 2);
    let lobby_a = recv_type(&mut alice, "lobby").await; // membership update
    assert_eq!(lobby_a["players"].as_array().unwrap().len(), 2);

    send(&mut alice, json!({"type": "configure", "rounds": 2, "round_secs": 10})).await;
    let cfg = recv_type(&mut alice, "lobby").await;
    assert_eq!(cfg["config"]["rounds"], 2);
    assert_eq!(cfg["config"]["round_secs"], 10);

    send(&mut bob, json!({"type": "start"})).await;
    let err = recv_type(&mut bob, "error").await;
    assert!(err["message"].as_str().unwrap().contains("host"));

    // --- play both rounds ---
    send(&mut alice, json!({"type": "start"})).await;
    let mut last_results: Option<Value> = None;

    for round in 1..=2u32 {
        let rs_a = recv_type(&mut alice, "round_start").await;
        let rs_b = recv_type(&mut bob, "round_start").await;
        assert_eq!(rs_a["round"], round);
        assert_eq!(rs_a["seed"], rs_b["seed"], "players must share the seed");
        let seed = rs_a["seed"].as_u64().unwrap() as u32;

        let world = waldo_core::generate_world(seed);
        let w = world.waldo.pos;

        send(&mut alice, json!({"type": "click", "pos": [w[0], w[1], w[2]]})).await;
        let cr = recv_type(&mut alice, "click_result").await;
        assert_eq!(cr["hit"], true);
        assert!(cr["score"].as_u64().unwrap() > 4000, "instant find ≈ max score");
        let pf = recv_type(&mut alice, "player_found").await;
        assert_eq!(pf["found"], 1);

        send(&mut bob, json!({"type": "click", "pos": [-w[0], -w[1], -w[2]]})).await;
        let miss = recv_type(&mut bob, "click_result").await;
        assert_eq!(miss["hit"], false);
        assert_eq!(miss["misses"], 1);

        send(&mut bob, json!({"type": "click", "pos": [w[0], w[1], w[2]]})).await;
        let hit = recv_type(&mut bob, "click_result").await;
        assert_eq!(hit["hit"], true);

        let res_a = recv_type(&mut alice, "round_result").await;
        let _res_b = recv_type(&mut bob, "round_result").await;
        assert_eq!(res_a["round"], round);

        let results = res_a["results"].as_array().unwrap();
        assert_eq!(results.len(), 2);
        let alice_row = results.iter().find(|r| r["name"] == "Alice").unwrap();
        let bob_row = results.iter().find(|r| r["name"] == "Bob").unwrap();
        assert_eq!(alice_row["found"], true);
        assert_eq!(bob_row["found"], true);
        assert_eq!(alice_row["misses"], 0);
        assert_eq!(bob_row["misses"], 1);
        assert!(alice_row["time_ms"].as_i64().unwrap() >= 0);
        assert!(bob_row["score"].as_u64().unwrap() < alice_row["score"].as_u64().unwrap());

        let rw = res_a["waldo"].as_array().unwrap();
        for k in 0..3 {
            assert!((rw[k].as_f64().unwrap() as f32 - w[k]).abs() < 1e-4);
        }
        last_results = Some(res_a);
    }

    // --- game over ---
    let over_a = recv_type(&mut alice, "game_over").await;
    let over_b = recv_type(&mut bob, "game_over").await;
    let lb = over_a["leaderboard"].as_array().unwrap();
    assert_eq!(lb.len(), 2);
    assert_eq!(lb[0]["name"], "Alice", "Alice should win");
    assert!(lb[0]["total"].as_u64().unwrap() > lb[1]["total"].as_u64().unwrap());
    assert_eq!(over_a.to_string(), over_b.to_string());

    let final_total = lb[0]["total"].as_u64().unwrap();
    let last_alice_total = last_results.unwrap()["results"]
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["name"] == "Alice")
        .unwrap()["total"]
        .as_u64()
        .unwrap();
    assert_eq!(final_total, last_alice_total);
}

#[tokio::test]
async fn join_unknown_lobby_fails() {
    let _guard = start_server_on(8992).await;
    let (mut ws, _) = connect_async("ws://127.0.0.1:8992/ws").await.expect("connect");
    ws.send(Message::Text(
        json!({"type": "join", "code": "ZZZZ", "name": "Nobody"}).to_string().into(),
    ))
    .await
    .unwrap();
    let v = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if let Message::Text(t) = ws.next().await.unwrap().unwrap() {
                break serde_json::from_str::<Value>(&t).unwrap();
            }
        }
    })
    .await
    .unwrap();
    assert_eq!(v["type"], "error");
}

async fn start_server_on(port: u16) -> ServerGuard {
    let bin = env!("CARGO_BIN_EXE_waldo-server");
    let child = Command::new(bin)
        .env("PORT", port.to_string())
        .env("WALDO_INTERMISSION_MS", "200")
        .spawn()
        .expect("spawn server");
    for _ in 0..50 {
        if TcpStream::connect(("127.0.0.1", port)).await.is_ok() {
            return ServerGuard(child);
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    panic!("server did not start");
}
