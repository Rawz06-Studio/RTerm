# An example: shows where you are. Edit it, or add your own scripts here.
#
# Every file in this folder appears in the command palette as "Script: <name>"
# and in "Run Script...". PowerShell runs .ps1, .bat, .cmd and .exe files;
# under WSL, RTerm runs the other ones (.sh, .py...) with the interpreter
# named on their first line (#!). The first comment is the description.

Write-Host "Hello from RTerm, in $(Get-Location)"
