#!/usr/bin/env bash
# Generate the benchmark fixtures: DoenetML text and parsed DAST JSON for each
# spec in the default sweep (or the specs given as arguments).
set -euo pipefail
cd "$(dirname "$0")/.."
cargo build -q --release -p cells-docgen
DOCGEN=target/release/cells-docgen
mkdir -p fixtures
specs=("$@")
if [ ${#specs[@]} -eq 0 ]; then mapfile -t specs < <($DOCGEN --sweep); fi
for spec in "${specs[@]}"; do
  $DOCGEN "$spec" > "fixtures/$spec.doenet"
  start=$(date +%s%N)
  node --max-old-space-size=8192 --stack-size=65500 scripts/parse-dast.mjs -i "fixtures/$spec.doenet" > "fixtures/$spec.json"
  ms=$(( ($(date +%s%N) - start) / 1000000 ))
  printf '%-16s %8d bytes doenet %10d bytes json  parse %6d ms\n' "$spec" "$(stat -c %s fixtures/$spec.doenet)" "$(stat -c %s fixtures/$spec.json)" "$ms"
done
