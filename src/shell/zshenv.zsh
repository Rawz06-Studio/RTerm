# RTerm shell integration for zsh.
#
# RTerm launches zsh with ZDOTDIR pointing at this file's directory, which
# makes zsh load it before your own configuration. We immediately restore
# ZDOTDIR to its original value so that your files (.zshenv, .zshrc, …) are
# read as usual, then install three hooks that tell RTerm when each command
# starts and ends (OSC 133 sequences). Nothing else is modified.

if [[ -n "${RTERM_ORIG_ZDOTDIR+X}" ]]; then
    export ZDOTDIR="$RTERM_ORIG_ZDOTDIR"
    unset RTERM_ORIG_ZDOTDIR
else
    unset ZDOTDIR
fi

if [[ -f "${ZDOTDIR:-$HOME}/.zshenv" ]]; then
    builtin source "${ZDOTDIR:-$HOME}/.zshenv"
fi

if [[ -o interactive ]]; then
    # The command line was just accepted (before anything runs).
    _rterm_line_finish() {
        builtin printf '\e]133;L\a'
    }

    # The command is about to run: its output starts here.
    _rterm_preexec() {
        builtin printf '\e]133;C\a'
    }

    # The command has finished and a new prompt is coming.
    _rterm_precmd() {
        local rterm_status=$?
        builtin printf '\e]133;D;%s\a\e]1337;CurrentDir=%s\a' "$rterm_status" "$PWD"
        # The PATH is used to complete commands; sent whenever it changes.
        if [[ "$PATH" != "$_rterm_path" ]]; then
            typeset -g _rterm_path="$PATH"
            builtin printf '\e]1337;RTermPath=%s\a' "$PATH"
        fi
        # Aliases, functions and builtins: used to highlight the input and
        # to complete commands. Sent whenever their number changes.
        local rterm_names="${#aliases}.${#functions}"
        if [[ "$rterm_names" != "$_rterm_names" ]]; then
            typeset -g _rterm_names="$rterm_names"
            local -a rterm_list=(${(k)aliases} ${(k)builtins} ${(k)functions:#_*} $reswords)
            builtin printf '\e]1337;RTermCommands=%s\a' "${(j: :)rterm_list}"
        fi
        builtin printf '\e]133;A\a'

        # Installed on the first prompt, hence after your .zshrc: a theme
        # that defines its own zle-line-finish is thereby preserved.
        if [[ -z "$_rterm_zle_hooked" ]]; then
            typeset -g _rterm_zle_hooked=1
            # zsh flags output without a trailing line feed with a "%"
            # written before the prompt. Since the prompt is hidden, that
            # mark would end up in the command's block: we disable it.
            builtin unsetopt prompt_sp
            builtin autoload -Uz add-zle-hook-widget 2>/dev/null \
                && add-zle-hook-widget line-finish _rterm_line_finish 2>/dev/null
        fi
    }

    builtin autoload -Uz add-zsh-hook
    add-zsh-hook precmd _rterm_precmd
    add-zsh-hook preexec _rterm_preexec
fi
