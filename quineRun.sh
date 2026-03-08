#!/bin/bash

while true
do
  clear
  cat main.c

  gcc main.c
  ./a.out > main.c
  sleep 0.1
done