#!/usr/bin/env bash
# Development launcher — runs straight from the source tree using cargo.
# Installed copies get a `kindle-sender` command instead; see README.
cd "$(dirname "$0")"
if [ -f Cargo.toml ]; then
    exec cargo run --quiet -- "$@"
elif [ -x ./venv/bin/python ]; then
    exec ./venv/bin/python -m kindle_sender "$@"
else
    exec python3 -m kindle_sender "$@"
fi
