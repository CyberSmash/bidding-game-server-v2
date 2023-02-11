BEGIN TRANSACTION;
DROP TABLE IF EXISTS "players";
CREATE TABLE IF NOT EXISTS "players" (
	"id"	INTEGER NOT NULL UNIQUE,
	"username"	TEXT NOT NULL UNIQUE,
	"token"	INTEGER NOT NULL,
	"wins"	INTEGER DEFAULT 0,
	"losses"	INTEGER DEFAULT 0,
	"elo"	INTEGER,
	"draws"	INTEGER NOT NULL DEFAULT 0,
	UNIQUE("username"),
	PRIMARY KEY("id" AUTOINCREMENT)
);
DROP TABLE IF EXISTS "server_stats";
CREATE TABLE IF NOT EXISTS "server_stats" (
	"game_masters"	INTEGER NOT NULL DEFAULT 0,
	"concurrent_games"	INTEGER NOT NULL DEFAULT 0,
	"avg_wait_time"	INTEGER NOT NULL DEFAULT 0
);
INSERT INTO "players" ("id","username","token","wins","losses","elo","draws") VALUES (1,'jordan',0,0,0,400,0),
 (2,'bob',0,0,0,400,0),
 (3,'paul',0,0,0,400,0),
 (4,'grace',0,0,0,400,0),
 (6,'alex',0,0,0,400,0),
 (7,'mark',0,0,0,400,0),
 (8,'rose',0,0,0,400,0),
 (9,'dakota',0,0,0,400,0),
 (11,'valarie',0,0,0,400,0);
INSERT INTO "server_stats" ("game_masters","concurrent_games","avg_wait_time") VALUES (4,0,0);
COMMIT;
