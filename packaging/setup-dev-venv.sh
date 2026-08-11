#!/usr/bin/env bash
# Create the development venv used by ./run.sh.
set -euo pipefail
cd "$(dirname "$0")/.."
python3 -m venv --system-site-packages venv
./venv/bin/pip install -q --upgrade pip
./venv/bin/pip install -q -e '.[all]'
echo "Ready — launch with ./run.sh"
