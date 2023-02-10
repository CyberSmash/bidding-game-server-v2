pub mod database_ops {
    use sqlx::pool::PoolConnection;
    use sqlx::{Sqlite};
    use crate::error::BidError;
    use log::{error, info};
    use crate::GameResultType;
    use crate::calc_elo;
    use crate::ELO_FLOOR;

    async fn inc_field(db_connection: &mut PoolConnection<Sqlite>, inc_field: String, username: &String) -> Result<(), BidError>
    {
        return match sqlx::query(&*format!("UPDATE players SET {} = {} + 1 where username = ?", inc_field, inc_field))
            .bind(username)
            .execute(db_connection).await
        {
            Ok(_) => { Ok(()) }
            Err(e) => {
                error!("Could not increment field {} for user {} error {:?}", inc_field, username, e);
                return Err(BidError::PlayerAAbandoned);
            }
        };
    }

    pub async fn inc_wins(db_connection: &mut PoolConnection<Sqlite>, username: &String) -> Result<(), BidError> {
        //sqlx::query("UPDATE players SET wins = wins + 1 where username = ?").bind(username).execute(db_connection).await.unwrap();
        inc_field(db_connection, String::from("wins"), &username).await?;
        Ok(())
    }

    pub async fn inc_losses(db_connection: &mut PoolConnection<Sqlite>, username: &String) -> Result<(), BidError>{
        inc_field(db_connection, String::from("losses"), &username).await?;
        Ok(())
    }

    pub async fn inc_draws(db_connection: &mut PoolConnection<Sqlite>, username: &String) -> Result<(), BidError> {
        inc_field(db_connection, String::from("draws"), &username).await?;
        Ok(())
    }



    pub async fn get_player_elo_from_db(db_connection: &mut PoolConnection<Sqlite>, username: &String) -> Result<i32, BidError> {
        // We'll make this more generic if it suits us.
        let q: (i32,) = match sqlx::query_as("SELECT elo FROM players where username = ?")
            .bind(username)
            .fetch_one(db_connection).await {
            Ok(elo) => {
                elo
            }
            Err(e) => {
                error!("Could not find ELO in database for {} {:?}", username, e);
                return Err(BidError::DbCannotExecuteQuery);
            }
        };
        Ok(q.0)
    }


    pub async fn update_elo(db_connection: &mut PoolConnection<Sqlite>, player: &String, new_elo: u32) -> Result<(), BidError> {
        return match sqlx::query("UPDATE players SET elo = ? WHERE username = ?")
            .bind(new_elo)
            .bind(player)
            .execute(db_connection).await {
                Err(e) => {
                    error!("Error excuting set ELO. {}", e);
                    Err(BidError::DbCannotExecuteQuery)
                }
                Ok(_) => {Ok(())}
        }
    }

    pub async fn update_elos(db_connection: &mut PoolConnection<Sqlite>, winner: &String, loser: &String) -> Result<(), BidError> {
        let winner_elo = get_player_elo_from_db(db_connection, &winner).await?;
        let loser_elo = get_player_elo_from_db(db_connection, &loser).await?;

        let mut elos = calc_elo(winner_elo as u32, loser_elo as u32, GameResultType::PlayerAWins);
        info!("Start Elos: {} {} end elos: {} {}", winner_elo, loser_elo, elos.0, elos.1);

        elos.1 = std::cmp::max(elos.1, ELO_FLOOR);
        elos.0 = std::cmp::max(elos.0, ELO_FLOOR);

        update_elo(db_connection, &winner, elos.0).await?;
        update_elo(db_connection, &loser, elos.1).await?;

        Ok(())
    }

}