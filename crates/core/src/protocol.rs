//! Client/server protocol: JSON over WebSocket, tagged by `type`.

use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq)]
pub struct Config {
    pub rounds: u32,
    pub round_secs: u32,
}

impl Default for Config {
    fn default() -> Self {
        Config { rounds: 3, round_secs: 90 }
    }
}

impl Config {
    pub fn clamped(self) -> Config {
        Config {
            rounds: self.rounds.clamp(1, 10),
            round_secs: self.round_secs.clamp(10, 300),
        }
    }
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct PlayerInfo {
    pub id: u32,
    pub name: String,
    pub is_host: bool,
    /// False while the player is in their disconnect grace period.
    pub connected: bool,
}

#[derive(Serialize, Deserialize, Debug)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ClientMsg {
    /// Create a lobby and become its host.
    Create { name: String },
    /// Join an existing lobby by code.
    Join { code: String, name: String },
    /// Host only: change game settings while in the lobby.
    Configure { rounds: u32, round_secs: u32 },
    /// Host only: start the game.
    Start,
    /// Resume a session after a dropped connection or page refresh.
    Rejoin { code: String, token: String },
    /// Claim Waldo is at this planet-local position; the server judges.
    Click { pos: [f32; 3] },
}

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ServerMsg {
    Error { message: String },
    /// Sent on join and whenever lobby membership / settings change.
    Lobby { code: String, you: u32, token: String, players: Vec<PlayerInfo>, config: Config },
    RoundStart {
        round: u32,
        total_rounds: u32,
        seed: u32,
        round_secs: u32,
        /// Round modifier: "", "night", "lightning", "crowded" or "tiny".
        mutator: String,
        /// Server timestamp (ms since epoch) when the round ends.
        ends_at_ms: u64,
    },
    /// Personal verdict on your click. `target` is who you hit:
    /// "waldo", "wenda", "woof", "wizard", "odlaw" or "miss".
    ClickResult { target: String, points: i32, round_score: u32, misses: u32 },
    /// Someone found Waldo.
    PlayerFound { id: u32, found: u32, total: u32 },
    RoundResult {
        round: u32,
        waldo: [f32; 3],
        results: Vec<RoundEntry>,
        /// ms until the next round or the win screen.
        next_in_ms: u64,
    },
    GameOver { leaderboard: Vec<Standing> },
    /// Everything a rejoining client needs to rebuild a round in progress.
    Restore {
        round: u32,
        total_rounds: u32,
        seed: u32,
        round_secs: u32,
        mutator: String,
        ends_at_ms: u64,
        /// Your own progress this round.
        found_rank: u32,
        misses: u32,
        wenda: bool,
        woof: bool,
        wizard: bool,
        odlaw: bool,
    },
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct RoundEntry {
    pub id: u32,
    pub name: String,
    pub found: bool,
    /// Finish order for Waldo this round (1-based), 0 if never found.
    pub rank: u32,
    /// ms into the round when Waldo was clicked, -1 if never found.
    pub time_ms: i64,
    pub misses: u32,
    /// Net cast bonus (Wenda/Woof/Wizard minus Odlaw), can be negative.
    pub bonus: i32,
    pub score: u32,
    pub total: u32,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Standing {
    pub id: u32,
    pub name: String,
    pub total: u32,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn client_msg_roundtrip() {
        let m: ClientMsg =
            serde_json::from_str(r#"{"type":"join","code":"ABCD","name":"joey"}"#).unwrap();
        assert!(matches!(m, ClientMsg::Join { .. }));
        let m: ClientMsg =
            serde_json::from_str(r#"{"type":"click","pos":[1.0,2.0,3.0]}"#).unwrap();
        assert!(matches!(m, ClientMsg::Click { pos } if pos == [1.0, 2.0, 3.0]));
    }

    #[test]
    fn server_msg_serializes_with_tag() {
        let s = serde_json::to_string(&ServerMsg::PlayerFound { id: 1, found: 2, total: 4 })
            .unwrap();
        assert!(s.contains(r#""type":"player_found""#));
        let s = serde_json::to_string(&ServerMsg::ClickResult {
            target: "wenda".into(), points: 100, round_score: 1100, misses: 1,
        })
        .unwrap();
        assert!(s.contains(r#""type":"click_result""#));
        assert!(s.contains(r#""target":"wenda""#));
    }

    #[test]
    fn config_clamps() {
        let c = Config { rounds: 99, round_secs: 1 }.clamped();
        assert_eq!(c.rounds, 10);
        assert_eq!(c.round_secs, 10);
    }
}
