#!/usr/bin/env bash
# Development launcher — runs straight from the source tree using ./venv.
# Installed copies get a `kindle-sender` command instead; see README.
cd "$(dirname "$0")"
if [ -x ./venv/bin/python ]; then
    exec ./venv/bin/python -m kindle_sender "$@"
fi
exec python3 -m kindle_sender "$@"
