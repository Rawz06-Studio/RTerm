# RTerm shell integration for PowerShell (Windows PowerShell 5.1 and
# PowerShell 7).
#
# RTerm starts PowerShell with `-NoExit -Command`, which loads this file after
# your profile. It wraps your prompt function and the Enter key so that they
# tell RTerm when each command starts and ends (OSC 133 sequences). Your
# prompt still runs; nothing else is modified.

$global:__rterm_prompt = $function:prompt
$global:__rterm_path = $null
$global:__rterm_commands_sent = $false

function global:prompt {
    # First, before anything changes them: how the last command went.
    $succeeded = $?
    $code = $global:LASTEXITCODE
    $status = if ($succeeded) { 0 } elseif ($code) { $code } else { 1 }

    $e = [char]27
    $b = [char]7
    $marks = "$e]133;D;$status$b"
    $location = $ExecutionContext.SessionState.Path.CurrentLocation
    if ($location.Provider.Name -eq 'FileSystem') {
        $marks += "$e]1337;CurrentDir=$($location.ProviderPath)$b"
    }
    # The PATH is used to complete commands; sent whenever it changes.
    if ($env:PATH -ne $global:__rterm_path) {
        $global:__rterm_path = $env:PATH
        $marks += "$e]1337;RTermPath=$env:PATH$b"
    }
    # Aliases, functions, cmdlets and keywords: used to highlight the input
    # and to complete commands. Sent once, at the first prompt. Only what is
    # already loaded: listing every installed module can take minutes, and
    # their cmdlets load on first use anyway (RTerm does not flag unknown
    # Verb-Noun names).
    if (-not $global:__rterm_commands_sent) {
        $global:__rterm_commands_sent = $true
        $names = @(Get-Command -CommandType Alias, Function, Cmdlet -ListImported -ErrorAction SilentlyContinue |
            ForEach-Object { $_.Name })
        $names += 'if', 'else', 'elseif', 'switch', 'foreach', 'for', 'while', 'do', 'until',
            'function', 'filter', 'return', 'break', 'continue', 'exit', 'try', 'catch',
            'finally', 'throw', 'trap', 'param', 'begin', 'process', 'end', 'class', 'enum'
        $marks += "$e]1337;RTermCommands=$($names -join ' ')$b"
    }
    $marks += "$e]133;A$b"
    [Console]::Write($marks)

    $global:LASTEXITCODE = $code
    & $global:__rterm_prompt
}

# The command is about to run: its output starts here.
if (Get-Module PSReadLine) {
    Set-PSReadLineKeyHandler -Chord Enter -ScriptBlock {
        [Microsoft.PowerShell.PSConsoleReadLine]::AcceptLine()
        [Console]::Write("$([char]27)]133;C$([char]7)")
    }
}
