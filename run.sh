#!/usr/bin/env bash
# Development launcher — runs straight from the source tree.
cd "$(dirname "$0")"

if [ -x target/release/kindle-sender ]; then
    exec target/release/kindle-sender "$@"
elif [ -x target/debug/kindle-sender ]; then
    exec target/debug/kindle-sender "$@"
elif [ -f Cargo.toml ]; then
    exec cargo run --quiet -- "$@"
elif [ -x ./venv/bin/python ]; then
    exec ./venv/bin/python -m kindle_sender "$@"
else
    exec python3 -m kindle_sender "$@"
fi
