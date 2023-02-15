use async_std::{
    net::{TcpListener, TcpStream, ToSocketAddrs},
    task,
};

use sqlx::sqlite::{SqlitePoolOptions, SqlitePool, SqliteConnectOptions};
use sqlx::ConnectOptions;
use futures::{StreamExt};

use futures::channel::mpsc;
use futures::sink::SinkExt;
use std::{
    collections::hash_map::{HashMap},
};

use std::net::Shutdown;
use chrono::{Utc};
use std::time::Duration;
use rand::{rngs::StdRng, Rng, SeedableRng};
mod protos;
mod player_management;
use clap::{Parser};
use protos::Comms;
use std::path::PathBuf;
use std::str::FromStr;

mod error;
use simple_logger;
use error::BidError;
use crate::Comms::ServerRequest;

use crate::protos::Comms::server_request::MsgType;
use log::{warn, info, trace, error};
mod proto_utils;
mod game_master;
use proto_utils::proto_utils::*;
use player_management::player_management::*;
use crate::game_master::game_master::{game_master};
mod database_ops;
use database_ops::database_ops::*;
type Sender<T> = mpsc::UnboundedSender<T>;
type Receiver<T> = mpsc::UnboundedReceiver<T>;


const SOCKET_READ_TIMEOUT: Duration = Duration::from_millis(500);
const MAX_GAME_ROUNDS: u32 = 10;
const BOTTLE_MIN: u32 = 0;
const BOTTLE_MAX: u32 = 10;
const MAX_BID_ERRORS: u32 = 3;
const ELO_FLOOR: u32 = 100;


const PROTO_VERSION_MAJOR: u32  = 0;
const PROTO_VERSION_MINOR: u32  = 2;

enum GameStatus {
    Completed,
    Draw,
    Abandoned,
}

pub enum Event {
    NewPlayer {
        player: Player
    },
    NewGame {
        name_a: String,
        name_b: String,
        stream_a: TcpStream,
        stream_b: TcpStream,
    },
    GameOver {
        result: GameResult,
    },
    NeedPlayers {
        id: u32
    },
    NoPlayersAvailable
}

struct GameMasterInfo {
    id: u32,
    sender: Sender<Event>,
    receiver: Receiver<Event>,
}

pub enum GameResultType {
    PlayerAWins,
    PlayerBWins,
    Draw,
}

pub struct GameResult
{
    // The ID of the game master.
    id: u32,
    // The winner of the game if there is one
    winner: String,
    // The loser of the game if there is one
    loser: String,
    // The result of the game.
    status: GameStatus,
}

#[derive(sqlx::FromRow)]
struct DatabaseUser {
    id: i32,
    username: String,
    token: i32,
    wins: i32,
    losses: i32,
    elo: i32,
    draws: i32,
}

// The player managers representation of a player. This is the authoritative
// copy of the player information.
pub struct Player {
    id: i32,
    name: String,
    wins: u64,
    losses: u64,
    in_game: bool,
    stream: TcpStream,
    player_cooldown: chrono::DateTime<Utc>,
    player_wait_time: chrono::DateTime<Utc>,
}

// Only the info that the Game Master needs of a player.
pub struct GMPlayer {
    name: String,
    stream: TcpStream,
}

type SomeResult<T> = std::result::Result<T, BidError>;

/**
 * Arguments Parsing.
 */
#[derive(Parser, Debug)]
struct Args {

    #[arg(short, long, value_name = "FILE")]
    database: PathBuf,

    #[arg(short, long, default_value_t = String::from("localhost"))]
    interface: String,

    #[arg(short, long, default_value_t = String::from("8080"))]
    port: String
}

fn calc_elo(r1: u32, r2: u32, result: GameResultType) -> (u32, u32) {
    let k: f64 = 32.0;
    let R1: f64 = f64::powf(10.0, r1 as f64 / 400.0);
    let R2: f64 = f64::powf(10.0, r2 as f64 / 400.0);

    let E1: f64 = R1 / (R1 + R2);
    let E2: f64 = R2 / (R1 + R2);

    let S1 = if matches!(result, GameResultType::PlayerAWins) {1.0} else if matches!(result, GameResultType::PlayerBWins) {0.0} else {0.5};
    let S2 = if matches!(result, GameResultType::PlayerAWins) {0.0} else if matches!(result, GameResultType::PlayerBWins) {1.0} else {0.5};

    let final_r1 = r1 as f64 + k * (S1 - E1);
    let final_r2 = r2 as f64 + k *(S2 - E2);

    warn!("Calculated Elos: {} {}", final_r1, final_r2);
    return (final_r1 as u32, final_r2 as u32);

}


async fn handle_need_players(players: &mut HashMap<String, Player>, gm_sender: &mut Sender<Event>) -> bool {
    let free_players = get_free_players(players).await;

    if free_players.len() < 2 {
        match gm_sender.send(Event::NoPlayersAvailable).await
        {
            Ok(_) => {}
            Err(e) => {
                error!("Error sending message to game master. {:?}", e);
            }
        }
        return false;
    }
    let player_a = match get_player_info_for_game(players, &free_players[0])
    {
        None => {
            error!("I errored out trying to find a player. The player {} was suddenly not found.", free_players[0]);
            match gm_sender.send(Event::NoPlayersAvailable).await {
                Ok(_) => {}
                Err(e) => {
                    // There's not much we can do about this. Just return and hope for the best.
                    // There may be a case to return an error instead of a boolean.
                    error!("I couldn't talk to the game manager due to error: {}", e);
                }
            };
            return false;
        }
        Some(p) => {p}
    };

    let player_b = match get_player_info_for_game(players, &free_players[1])
    {
        None => {
            error!("I errored out trying to find a player. The player {} was suddenly not found.", free_players[1]);
            return false;
        }
        Some(p) => {p}
    };

    info!("Sending {} and {} to start a game", player_a.name, player_b.name);
    match gm_sender.send(Event::NewGame {
        name_a: player_a.name,
        name_b: player_b.name,
        stream_a: player_a.stream.clone(),
        stream_b: player_b.stream.clone(),
    }).await
    {
        Ok(_) => {}
        Err(e) => {
            println!("Error: Something went wrong sending the new game message to the \
                        game manager. E: {}", e.to_string());
        }
    }

    true
}


async fn player_manager(sender: Sender<Event>, mut receiver: Receiver<Event>, database_pool: SqlitePool) -> Result<(), BidError> {


    let mut rng = StdRng::seed_from_u64(Utc::now().timestamp() as u64);
    let mut players: HashMap<String, Player> = HashMap::new();
    let mut game_comms: HashMap<u32, Sender<Event>> = HashMap::new();
    let mut stats_update_timer = Utc::now() + chrono::Duration::milliseconds(500);
    let mut db_connection = match database_pool.acquire().await {
        Ok(c) => c,
        Err(e) => {
            println!("Could not acquire a connection to the database from the pool. {}", e);
            return Err(BidError::DbCannotOpenDatabase);
        }
    };
    let max_games = 4;

    for id in 0..max_games {
        let (gm_sender, gm_receiver) : (Sender<Event>, Receiver<Event>) = mpsc::unbounded();
        game_comms.insert(id.clone(), gm_sender);

        let mut gm_db_conn = match database_pool.acquire().await {
            Ok(c) => c,
            Err(e) => {
                println!("Could not acquire a connection to the database from the pool. {}", e);
                break;
            }
        };
        task::spawn(game_master(sender.clone(), gm_receiver, id, gm_db_conn));
    }


    match sqlx::query("UPDATE server_stats SET game_masters = ?")
        .bind(max_games)
        .execute(&mut db_connection).await {
        Err(e) => {
            error!("Error updating games tats game_masters; {} Error {}", max_games, e);
        }
        Ok(_) => {}
    };

    let mut concurrent_games = 0;

    while let Some(event) = receiver.next().await {
        if stats_update_timer < Utc::now() {
            stats_update_timer = Utc::now() + chrono::Duration::milliseconds(500);
            match sqlx::query("UPDATE server_stats SET concurrent_games = ?")
                .bind(concurrent_games)
                .execute(&mut db_connection).await {
                Err(e) => {
                    error!("Could not update game stats SET concurrent_games: Error {}", e);
                }
                Ok(_) => {}
            };
        }

        match event {
            Event::NewPlayer { mut player } => {
                player.player_wait_time = Utc::now();
                trace!("The player manager has recvd. {}", player.name);
                players.insert(player.name.clone(), player);
                info!("Num players: {}", players.len())
            }
            Event::GameOver {
                result
            } => {
                concurrent_games -= 1;
                match result.status {
                    GameStatus::Abandoned => {
                        warn!("The game had to be abandoned due to {}", result.loser);
                        let mut abort_msg = ServerRequest::new();
                        abort_msg.set_msgType(MsgType::GAME_ABORT);

                        info!("The player manager has been told that {} disconnected.", result.loser);
                        players[&result.loser].stream.shutdown(Shutdown::Both).ok();
                        match send_proto_to_client_by_name(&mut players, &result.winner, abort_msg).await
                        {
                            Ok(_) => {}
                            Err(e) => {
                                match e {
                                    BidError::PlayerNotFoundByName => {
                                    }
                                    _ => {}

                                }
                            }
                        };
                        players.remove(&result.loser);
                    }
                    GameStatus::Completed => {
                        info!("The game completed with {} as the winner, and {} as the loser",
                                 result.winner,
                                 result.loser);

                        //update_elos(&mut db_connection, &result.winner, &result.loser).await.ok();
                    }
                    GameStatus::Draw => {
                        // As these database queries are logged already, there's not much else to do but return the error and try
                        // to continue on as if nothing ever happened.
                        inc_draws(&mut db_connection, &result.winner).await.ok();
                        inc_draws(&mut db_connection, &result.loser).await.ok();
                        //update_elos(&mut db_connection, &result.winner, &result.loser).await.ok();
                    }
                }
                if let Some(peer) = players.get_mut(&result.winner) {
                    trace!("Player {} has returned to the pool.", result.winner);
                    peer.player_cooldown = Utc::now() + chrono::Duration::milliseconds(rng.gen_range(20..150));
                    peer.in_game = false;
                    peer.wins += 1;
                    inc_wins(&mut db_connection, &result.winner).await.ok();

                }
                if let Some(peer) = players.get_mut(&result.loser)
                {
                    trace!("Player {} has been returned to the pool.", result.loser);
                    peer.player_cooldown = Utc::now() + chrono::Duration::milliseconds(rng.gen_range(20..150));
                    peer.in_game = false;
                    peer.losses += 1;
                    inc_losses(&mut db_connection, &result.loser).await.ok();
                }
            }
            Event::NeedPlayers {id} => {

                let gm_sender = match game_comms.get_mut(&id)
                {
                    None => {
                        error!("So I shouldn't be here but I lost the game master with the ID of {}", id);
                        continue;
                    }
                    Some(gm) => {gm}
                };
                let game_started = handle_need_players(&mut players, gm_sender).await;
                if game_started {
                    concurrent_games += 1;
                }

            }
            _ => {}
        }
    }

    Ok(())

}

async fn connection_loop(mut stream: TcpStream, mut pm_sender: Sender<Event>, database_connection: SqlitePool) -> std::result::Result<(), BidError>
{

    let mut expected_version = Comms::ServerRequest::new();
    expected_version.set_msgType(MsgType::VERSION);
    expected_version.set_version_major(PROTO_VERSION_MAJOR);
    expected_version.set_version_minor(PROTO_VERSION_MINOR);

    match send_proto_to_client(&mut stream, &expected_version).await {
        Err(e) => {

            error!("Could not send version to client. {}", e);
            return Err(BidError::ErrorOnSend);
        }
        Ok(_) => {}
    };

    let mut auth_request = Comms::ServerRequest::new();
    auth_request.set_msgType(Comms::server_request::MsgType::AUTH_REQUEST);

    match send_proto_to_client(&mut stream, &auth_request).await {
        Ok(_) => {}
        Err(e) => { error!("Error: {}", e) }
    }


    let login_name = match read_proto_from_client(&mut stream).await {
        Ok(resp) => { resp }
        Err(e) => {
            error!("Error: {}", e);
            return Err(e);
        }
    };

    let username = match login_name.msgType() {
        Comms::server_request::MsgType::AUTH_RESPONSE => { login_name.authResponse.player_name().to_string() }
        _ => {
            error!("Invalid response received.");
            String::new()
        }
    };
    let mut conn = match database_connection.acquire().await {
        Ok(c) => {c}
        Err(e) => {
            error!("Could not get a database connection from the pool. {}", e);
            return Err(BidError::DbCannotOpenDatabase);
        }
    };
    let _ = match sqlx::query_as::<_, DatabaseUser>("SELECT * from players WHERE username = ?").bind(username.to_owned()).fetch_one(&mut *conn).await {
        Err(e) => {
            warn!("Rejected authentication from {} -- reason: {}", username, e);
            let mut auth_reject = ServerRequest::new();
            auth_reject.set_msgType(MsgType::AUTH_REJECT);

            send_proto_to_client(&mut stream, &auth_reject).await?;
            stream.shutdown(Shutdown::Both).ok();
            return Err(BidError::PlayerNotFoundByName);
        },
        Ok(result) => {result},
    };


    let p = Player {
        id: 0,
        name: username,
        wins: 0,
        losses: 0,
        in_game: false,
        stream: stream.clone(),
        player_cooldown: Utc::now(),
        player_wait_time: Default::default(),
    };



    let mut ack = ServerRequest::new();
    ack.set_msgType(Comms::server_request::MsgType::ACK);
    send_proto_to_client(&mut stream, &ack).await?;


    match pm_sender.send(Event::NewPlayer {player: p}).await {
        Ok(_) => {}
        Err(e) => {
            error!("Connection loop has lost connection to the player manager with error {:?}", e);
            return Err(BidError::PlayerManagerSendFailed);
        }
    };

    Ok(())
}

async fn accept_loop(addr: impl ToSocketAddrs, database: PathBuf) -> std::result::Result<(), BidError> {

    let listener = match TcpListener::bind(addr).await {
        Ok(l) => {l}
        Err(_) => {return Err(BidError::ErrorBindingToInterface)}
    };
    let mut incoming = listener.incoming();

    let options = SqliteConnectOptions::from_str(database.to_str().unwrap()).unwrap().disable_statement_logging().to_owned();

    let pool = SqlitePoolOptions::new().max_connections(10).connect_with(options).await.map_err(|_| BidError::DbCannotOpenDatabase)?;

    let (pm_sender, pm_receiver) : (Sender<Event>, Receiver<Event>) = mpsc::unbounded();

    let _pm = task::spawn(player_manager(pm_sender.clone(), pm_receiver, pool.clone()));

    while let Some(stream) = incoming.next().await {
        let stream = match stream {
            Ok(s) => {s}
            Err(_) => {continue}
        };

        info!("Accepting connection from {:?}", stream.peer_addr());
        let _handle = task::spawn(connection_loop(stream.clone(), pm_sender.clone(), pool.clone()));
    }

    Ok(())
}

fn main() -> Result<(), BidError> {
    simple_logger::init_with_level(log::Level::Info).unwrap();

    let args = Args::parse();
    trace!("Args: {:?}", args);

    //let (pool_broker_sender, pool_broker_receiver) : (Sender<Event>, Receiver<Event>) = mpsc::unbounded();
    let connection_string = format!("{}:{}", args.interface, args.port);
    let fut = accept_loop(connection_string, args.database);

    task::block_on(fut)?;

    Ok(())
}
