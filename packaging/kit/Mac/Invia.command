#!/bin/bash
cd "$(dirname "$0")"
read -r -p "Incolla il ticket ricevuto (lso1...) e premi Invio: " TICKET
echo "Trascina qui dentro il file (o piu' file) da inviare e premi Invio:"
read -r FILES
eval "set -- $FILES"
./lso send "$TICKET" "$@"
echo
echo "Se vedi \"IMPOSSIBILE\", copia tutto il testo qui sopra e mandalo a chi ti ha dato il programma."
read -r -p "Premi Invio per chiudere."
