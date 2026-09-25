#!/bin/bash
cd "$(dirname "$0")"
echo "Analisi della rete in corso (circa 30 secondi)..."
./lso diag --seconds 30
echo
echo "Copia il risultato qui sopra e mandalo a chi ti ha dato il programma."
read -r -p "Premi Invio per chiudere."
