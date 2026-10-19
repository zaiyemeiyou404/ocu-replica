param([string]$Python='python.exe')
$ErrorActionPreference='Stop'
$env:PYTHONDONTWRITEBYTECODE='1'
& $Python (Join-Path $PSScriptRoot 'runtime\supervisor.py')
exit $LASTEXITCODE
