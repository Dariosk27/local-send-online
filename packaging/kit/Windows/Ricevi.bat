@echo off
chcp 65001 >nul
cd /d "%~dp0"
echo ==============================================================
echo  RICEZIONE - lascia aperta questa finestra finche' ricevi.
echo  Copia il ticket (riga che inizia con lso1) e mandalo a chi invia.
echo  I file arrivano in: %USERPROFILE%\Downloads\LocalSendOnline
echo ==============================================================
lso.exe receive
pause
