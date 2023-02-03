BEGIN TRANSACTION;
CREATE TABLE IF NOT EXISTS "players" (
	"id"	INTEGER NOT NULL,
	"username"	TEXT NOT NULL UNIQUE,
	"token"	INTEGER NOT NULL DEFAULT 0,
	"wins"	INTEGER NOT NULL DEFAULT 0,
	"losses"	INTEGER NOT NULL DEFAULT 0,
	"elo"	INTEGER NOT NULL DEFAULT 400,
	"draws"	INTEGER NOT NULL DEFAULT 0,
	PRIMARY KEY("id" AUTOINCREMENT)
);
INSERT INTO "players" ("id","username","token","wins","losses","elo","draws") VALUES (1,'jordan',0,2,0,400,0),
 (2,'bob',0,1,0,400,0),
 (3,'grace',0,1,0,400,0),
 (4,'paul',0,3,0,400,0);
COMMIT;
