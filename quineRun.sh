#!/bin/bash

stty -echo -icanon
trap 'stty sane' EXIT

clear
cat main.c

while true
do
  gcc main.c -o a.out || break

  key=""
  while IFS= read -r -t 0.1 -n 1 ch; do key="$ch"; done

  output=$(./a.out $key) || break
  echo "$output" > main.c

  clear
  echo "$output"
done