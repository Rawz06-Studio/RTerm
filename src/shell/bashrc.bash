# RTerm shell integration for bash.
#
# RTerm starts bash with `--rcfile` pointing at this file, in place of
# ~/.bashrc. It first loads your own configuration the way bash would have,
# then installs two hooks that tell RTerm when each command starts and ends
# (OSC 133 sequences). Nothing else is modified.
#
# Works with bash 3.2 (the one shipped with macOS) and later.

if [[ "$RTERM_BASH_LOGIN" == 1 ]]; then
    # macOS terminals start login shells: read what a login shell reads.
    unset RTERM_BASH_LOGIN
    if [[ -r /etc/profile ]]; then
        builtin source /etc/profile
    fi
    for _rterm_file in ~/.bash_profile ~/.bash_login ~/.profile; do
        if [[ -r "$_rterm_file" ]]; then
            builtin source "$_rterm_file"
            break
        fi
    done
    unset _rterm_file
else
    if [[ -r /etc/bash.bashrc ]]; then
        builtin source /etc/bash.bashrc
    fi
    if [[ -r ~/.bashrc ]]; then
        builtin source ~/.bashrc
    fi
fi

# Set at the end of each prompt, cleared by the first command that runs.
_rterm_at_prompt=

# The command is about to run: its output starts here. Bash has no
# "preexec" hook, but the DEBUG trap runs before every simple command:
# only the first one after a prompt counts.
_rterm_preexec() {
    [[ -n "$_rterm_at_prompt" ]] || return 0
    # Nothing ran (empty line, syntax error): this is the next prompt.
    [[ "$BASH_COMMAND" == _rterm_precmd* ]] && return 0
    _rterm_at_prompt=
    builtin printf '\e]133;C\a'
}

# The command has finished and a new prompt is coming.
_rterm_precmd() {
    local rterm_status=$?
    _rterm_at_prompt=
    builtin printf '\e]133;D;%s\a\e]1337;CurrentDir=%s\a' "$rterm_status" "$PWD"
    # The PATH is used to complete commands; sent whenever it changes.
    # Under WSL, RTerm cannot read it: it gets an empty one, once.
    local rterm_path="$PATH"
    [[ -n "$RTERM_NO_PATH" ]] && rterm_path=""
    if [[ "$rterm_path" != "$_rterm_path" || -z "$_rterm_path_sent" ]]; then
        _rterm_path="$rterm_path"
        _rterm_path_sent=1
        builtin printf '\e]1337;RTermPath=%s\a' "$rterm_path"
    fi
    # Aliases, functions, builtins and keywords: used to highlight the input
    # and to complete commands. Sent whenever the list changes.
    local rterm_list
    rterm_list="$(builtin compgen -a -b -k -A function 2>/dev/null | command grep -v '^_' | command tr '\n' ' ')"
    if [[ "$rterm_list" != "$_rterm_list" ]]; then
        _rterm_list="$rterm_list"
        builtin printf '\e]1337;RTermCommands=%s\a' "$rterm_list"
    fi
    builtin printf '\e]133;A\a'
    return "$rterm_status"
}

trap '_rterm_preexec' DEBUG
# Our hook first, to read the status of the command; yours next; and the
# prompt is only "ready" once everything has run.
PROMPT_COMMAND="_rterm_precmd${PROMPT_COMMAND:+; $PROMPT_COMMAND}
_rterm_at_prompt=1"
