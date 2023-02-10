pub mod player_management {
    use crate::Player;
    use crate::Comms::ServerRequest;
    use crate::protos::Comms::server_request::MsgType;
    use chrono::{Utc};
    use std::{
        collections::hash_map::{HashMap, Entry},
    };
    use crate::proto_utils::proto_utils::*;
    use crate::GMPlayer;

    /// Gets the player's info by name.
    ///
    /// This is used to look up a player by name and get a structure that the Game Manager knows
    /// how to read.
    pub fn get_player_info_for_game(players: &mut HashMap<String, Player>, player_name: &String) -> Option<GMPlayer> {
        return match players.entry(player_name.clone())
        {
            Entry::Occupied(mut ent) => {
                let p = ent.get_mut();
                p.in_game = true;
                Some(GMPlayer { name: p.name.clone(), stream: p.stream.clone() })
            }
            Entry::Vacant(_) => { None }
        };
    }

    /// Gets a vector of players who are able to play a game.
    ///
    /// Whether or not a player is able to join a game is determined by several factors:
    /// - They can't currently be in a game
    /// - Their cooldown is over.
    /// - They respond successfully to an ALIVE message.
    ///
    /// This function will also clean up any dead connections before it returns the vector.
    pub async fn get_free_players(players: &mut HashMap<String, Player>) -> Vec<String>
    {
        let mut ready_players: Vec<String> = vec!();
        let mut dead_players: Vec<String> = vec!();
        let current_time = Utc::now();
        let mut alive_msg = ServerRequest::new();
        alive_msg.set_msgType(MsgType::ALIVE);
        for (name, player) in players.into_iter() {
            if !player.in_game && player.player_cooldown < current_time {
                match send_proto_to_client(&mut player.stream, &alive_msg).await {
                    Ok(_) => {}
                    Err(_) => {
                        dead_players.push(player.name.clone());
                        continue;
                    }
                }

                // We don't really care what they respond with as long as they respond with something.
                match read_proto_from_client(&mut player.stream.clone()).await {
                    Ok(r) => { r }
                    Err(_) => {
                        dead_players.push(player.name.clone());
                        continue;
                    }
                };

                // In this case we have found a player who can play the next game.
                ready_players.push(name.clone());
            }
        }

        // Before we go, remove any players who are dead.
        for dead_player in dead_players {
            println!("Pruning {} as they have gone away.", dead_player);
            players.remove(dead_player.as_str());
        }

        return ready_players;
    }
}