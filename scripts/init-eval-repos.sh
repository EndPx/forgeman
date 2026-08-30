#!/usr/bin/env bash
# Initialize the three evaluation repositories with clean git baselines.
# Used by docs/EVALUATION.md so anyone can reproduce the comparison.
set -euo pipefail

cd "$(dirname "$0")/../examples"

for repo in flawed-api flawed-js flawed-py; do
  cd "$repo"
  rm -rf .git
  git init -b main -q
  git config user.email "eval@forgeman.local"
  git config user.name "ForgeMan Eval"
  git add -A
  git commit -q -m "baseline: intentionally flawed code"
  echo "$repo: baseline $(git rev-parse --short HEAD)"
  cd ..
done

echo
echo "Next: put ZAI_API_KEY in .env, then follow docs/EVALUATION.md"
