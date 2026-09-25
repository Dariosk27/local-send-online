@echo off
chcp 65001 >nul
cd /d "%~dp0"
if "%~1"=="" (
  echo Trascina uno o piu' file SOPRA l'icona di Invia.bat per inviarli.
  pause
  exit /b
)
echo Incolla il ticket ricevuto (lso1...) con tasto destro, poi premi Invio:
set /p TICKET=
lso.exe send "%TICKET%" %*
echo.
echo Se vedi "IMPOSSIBILE", copia tutto il testo qui sopra e mandalo a chi ti ha dato il programma.
pause
