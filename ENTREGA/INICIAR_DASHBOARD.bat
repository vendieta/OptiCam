@echo off
chcp 65001 >nul
title Camara - Dashboard
cd /d "%~dp0"
echo Abriendo el dashboard en http://127.0.0.1:8080 ...
start "" http://127.0.0.1:8080
camara_v2.exe serve --port 8080
pause
