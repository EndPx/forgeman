#!/usr/bin/env bash
# ForgeMan killer-demo setup (spec §42).
# Prepares examples/flawed-api as its own git repository with a baseline
# commit so iteration checkpoints and `forgeman diff` work.
set -euo pipefail

cd "$(dirname "$0")/../examples/flawed-api"

if [ ! -d .git ]; then
  git init -b main
  git config user.email "demo@forgeman.local"
  git config user.name "ForgeMan Demo"
  git add -A
  git commit -m "baseline: intentionally flawed user API"
  echo "Demo repository initialized."
else
  echo "Demo repository already initialized."
fi

echo
echo "Run the demo:"
echo "  cd examples/flawed-api"
echo "  forgeman run \"Fix the API performance issue and make the failing tests pass\""
echo "  forgeman report"
echo "  forgeman diff"
