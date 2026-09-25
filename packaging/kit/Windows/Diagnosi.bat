@echo off
chcp 65001 >nul
cd /d "%~dp0"
echo Analisi della rete in corso (circa 30 secondi)...
lso.exe diag --seconds 30
echo.
echo Copia il risultato qui sopra e mandalo a chi ti ha dato il programma.
pause
