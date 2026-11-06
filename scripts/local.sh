#!/bin/sh
set -eu

cd "$(dirname "$0")/.."

env_file=${ENV_FILE:-.env}
if [ ! -f "$env_file" ]; then
	echo "local: $env_file not found. Copy .env.example to .env and fill it in." >&2
	exit 1
fi

line_number=0
while IFS= read -r line || [ -n "$line" ]; do
	line_number=$((line_number + 1))
	line=${line#"${line%%[![:space:]]*}"}
	case $line in
	'' | '#'*) continue ;;
	*=*) ;;
	*)
		echo "local: $env_file:$line_number is not KEY=value" >&2
		exit 1
		;;
	esac
	key=${line%%=*}
	value=${line#*=}
	case $key in
	'' | [!A-Za-z_]* | *[!A-Za-z0-9_]*)
		echo "local: $env_file:$line_number has an invalid name: $key" >&2
		exit 1
		;;
	esac
	case $value in
	\"*\" | \'*\')
		value=${value#?}
		value=${value%?}
		;;
	esac
	eval "already=\${$key+set}"
	if [ -z "$already" ]; then
		export "$key=$value"
	fi
done <"$env_file"

if [ -z "${DATABASE_URL:-}${DATABASE_URL_FILE:-}" ]; then
	DATABASE_URL=sqlite:///data/sei-monitor.db
fi
case ${DATABASE_URL:-} in
sqlite:///data/*) export DATABASE_URL="sqlite://data/${DATABASE_URL#sqlite:///data/}" ;;
esac

if [ -z "${HTTP_ADDRESS+set}" ]; then
	HTTP_ADDRESS=:8080
fi
case $HTTP_ADDRESS in
:*) export HTTP_ADDRESS="127.0.0.1$HTTP_ADDRESS" ;;
esac

go build -o sei-monitor .
exec ./sei-monitor "$@"
