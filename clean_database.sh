#!/bin/bash

DATABASE=./db.db

if [ $# -gt 2 ]; then
    DATABASE=$1
fi


sqlite3 "${DATABASE}" "UPDATE players SET wins = 0, losses = 0, draws = 0, elo = 400"

