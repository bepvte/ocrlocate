#!/usr/bin/env zsh
set -uo pipe_fail
zmodload -F zsh/zutil b:zparseopts

usage() {
	cat <<-EOF
	ocr <query>
	-c	dont use default dir
	-i	do scan
EOF
	exit 0
}
if [[ $# == 0 ]]; then
	usage
fi
typeset -A args
zparseopts -A args -D c i w:
echo $args[*]
if [[ -z ${args[(I)-c]} ]]; then
  cd ~/Pictures # change this to your pictures directory
fi
tempdir=$(mktemp ocr.XXXXX --tmpdir)
set -x
ocrlocate $( [[ -z ${args[(I)-i]} ]] && echo - '-n' ) $@ | cut -f3- | xargs -d'\n' -n1 wslpath -w >> $tempdir
'/mnt/c/Program Files/IrfanView/i_view64.exe' /thumbs /filelist="$(wslpath -w $tempdir)" /title="$1" &
disown
