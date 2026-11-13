# Started by RTerm on Windows, inside WSL: runs your login shell with RTerm's
# integration when it is zsh or bash, and as it is otherwise.
#
# RTerm passes the folder holding its integration scripts in RTERM_SHELL_DIR,
# translated to a Linux path by WSL (WSLENV).

rterm_dir="$RTERM_SHELL_DIR"
unset RTERM_SHELL_DIR

rterm_shell="$(getent passwd "$(id -un)" 2>/dev/null | cut -d: -f7)"
[ -n "$rterm_shell" ] || rterm_shell="${SHELL:-/bin/sh}"
export SHELL="$rterm_shell"
# Tells the integration to send the distribution and the home directory:
# RTerm, on Windows, reads this shell's files through \\wsl$.
export RTERM_WSL=1

case "${rterm_shell##*/}" in
    zsh)
        if [ -n "${ZDOTDIR+x}" ]; then
            export RTERM_ORIG_ZDOTDIR="$ZDOTDIR"
        fi
        export ZDOTDIR="$rterm_dir/zsh"
        exec "$rterm_shell" -l
        ;;
    bash)
        # Like a login shell: the script reads /etc/profile and ~/.profile.
        export RTERM_BASH_LOGIN=1
        exec "$rterm_shell" --rcfile "$rterm_dir/bash/rterm.bash" -i
        ;;
    *)
        exec "$rterm_shell" -l
        ;;
esac
