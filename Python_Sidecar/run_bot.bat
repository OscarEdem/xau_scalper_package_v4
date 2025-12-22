@echo off
cd /d "%~dp0"
call venv\Scripts\activate
echo Starting XAU Controller...
python xau_controller.py
pause