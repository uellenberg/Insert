#!/bin/bash

while true
do
  gcc main.c
  clear
  ./a.out | tee main.c
  sleep 0.1
done