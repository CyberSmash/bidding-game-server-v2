#!/bin/bash

if [ "$#" -lt 1 ]; then
    echo "Usage: $0 username"
    exit 1
fi

sqlite3 ./db.db "INSERT INTO players (username, token, wins, losses, elo, draws) values ('${1}', 0, 0, 0, 400, 0)"
