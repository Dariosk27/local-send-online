#!/bin/bash
cd "$(dirname "$0")"
echo "=============================================================="
echo " RICEZIONE - lascia aperta questa finestra finche' ricevi."
echo " Copia il ticket (riga che inizia con lso1) e mandalo a chi invia."
echo " I file arrivano in: ~/Downloads/LocalSendOnline"
echo "=============================================================="
./lso receive
