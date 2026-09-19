#!/bin/bash
# Records the synthetic cases of the benchmark dataset.
#
#     benchmarks/record.sh [path/to/argus]
#
# Runs benchmarks/apps/form.swift (its window appears for a few seconds
# without taking focus), records each step with `argus benchmark record`,
# then adds the label relations a person can see but the accessibility tree
# does not state. Review the result with `argus benchmark review`.
#
# Needs the Accessibility and Screen Recording permissions (see
# `argus doctor`). Native application cases (Calculator) are recorded by
# hand; see benchmarks/README.md.

set -euo pipefail
cd "$(dirname "$0")/.."
ARGUS=${1:-target/release/argus}
DATASET=benchmarks/dataset
WORK=$(mktemp -d)
trap 'rm -rf "$WORK"' EXIT

swiftc -O benchmarks/apps/form.swift -o "$WORK/form"

# record_form <case> <description> <form flags> <step[:command]>...
record_form() {
  local case=$1 description=$2 flags=$3
  shift 3
  rm -rf "${DATASET:?}/$case"
  mkfifo "$WORK/in"
  # shellcheck disable=SC2086
  "$WORK/form" $flags --truth "$WORK/truth.json" <"$WORK/in" >"$WORK/out" 2>&1 &
  exec 3>"$WORK/in"
  wait_for "ready"
  local pid
  pid=$(awk '/^ready/ {print $2}' "$WORK/out")
  for spec in "$@"; do
    local step=${spec%%:*} command=${spec#*:}
    if [ "$command" != "$spec" ] && [ -n "$command" ]; then
      echo "$command" >&3
      wait_for "ok $command"
      sleep 0.3
    fi
    "$ARGUS" benchmark record --pid "$pid" --out "$DATASET/$case/$step" \
      --extra-truth "$WORK/truth.json" --description "$description"
  done
  echo quit >&3
  exec 3>&-
  wait
  rm -f "$WORK/in" "$WORK/out"
}

wait_for() {
  for _ in $(seq 1 100); do
    grep -q "^$1" "$WORK/out" 2>/dev/null && return 0
    sleep 0.1
  done
  echo "form.swift did not answer '$1'" >&2
  exit 1
}

record_form form_light \
  "Synthetic AppKit form (benchmarks/apps/form.swift): standard controls described by the accessibility tree, and a canvas of drawn controls it does not describe. Steps check a checkbox, add a table row, and change the canvas." \
  "" 1_initial 2_checked:check 3_row:row 4_canvas:canvas
record_form form_dark "The synthetic form in dark mode." "--dark" 1_initial

# Labels beside their fields: visible, but not stated by the tree.
python3 - "$DATASET" <<'EOF'
import glob, json, sys
labels = [
    ("window/text:#1", "window/text_box:Name:"),
    ("window/text:#2", "window/text_box:Email:"),
    ("window/text:#3", "window/button:#1"),
    ("canvas/text:Snap to grid", "canvas/checkbox:Snap to grid"),
]
for path in sorted(glob.glob(f"{sys.argv[1]}/form_*/*/expected.json")):
    truth = json.load(open(path))
    ids = {element["id"] for element in truth["elements"]}
    relations = truth.setdefault("relations", [])
    for source, target in labels:
        assert source in ids and target in ids, (path, source, target)
        if not any(r["from"] == source and r["to"] == target for r in relations):
            relations.append({"kind": "label_for", "from": source, "to": target})
    with open(path, "w") as file:
        file.write(json.dumps(truth, indent=2, ensure_ascii=False) + "\n")
EOF
echo "recorded; review with: $ARGUS benchmark review $DATASET/form_light/1_initial -o review.png"
