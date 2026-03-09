#!/bin/bash

stty -echo -icanon
trap 'stty sane' EXIT

while true
do
  clear
  cat main.c
  gcc main.c -o a.out || break

  key=""
  while IFS= read -r -t 0.1 -n 1 ch; do key="$ch"; done

  output=$(./a.out $key) || break
  echo "$output" > main.c
done