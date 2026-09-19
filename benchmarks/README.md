# Argus Benchmark Suite

Measures Argus against recorded ground truth instead of judging it by demos.

```bash
argus benchmark run                         # accuracy on the dataset, all modes
argus benchmark run --mode pixels --explain # every missed element and false positive
argus benchmark run --json                  # everything, machine-readable
argus benchmark latency --app Calculator    # live latency and memory
```

## Dataset

`benchmarks/dataset/<case>/<step>/` holds one recorded state of one window:

| File                 | Content                                                        |
| -------------------- | -------------------------------------------------------------- |
| `frame.png`          | The window's pixels, as captured (RGBA).                       |
| `frame.json`         | Where the window was (global points, scale factor).            |
| `accessibility.json` | The raw accessibility tree (optional).                         |
| `expected.json`      | Ground truth: elements, roles, text, bounds, states, parents, relations. |

The steps of a case are successive states of one window and are observed as
one tracking session. Every truth element has an `id` that names the same
element in every step, and is `significant` if an observation must contain it
(visible controls and text; structural containers are not).

| Case         | Steps | What it tests |
| ------------ | ----- | ------------- |
| `calculator` | 4     | A native SwiftUI app: clear, type 12, add 3, equals. Renamed button (AC → C), a line that appears. |
| `form_light` | 4     | A synthetic AppKit form ([`apps/form.swift`](apps/form.swift)): text fields, checkboxes, radio buttons, popup, segmented control, slider, table, and a **canvas of drawn controls the accessibility tree does not describe**. Steps change a checkbox, add a table row, change the canvas. |
| `form_dark`  | 1     | The form in dark mode. |
| `textedit`   | 2     | TextEdit with a synthetic rich-text document: text area, format bar, ruler; step 2 selects a phrase. Recorded as a **held-out** case before the fixes of 2026-09-19 (see below). |

Nothing private is recorded: the form's and the document's content is
synthetic and the Calculator shows only numbers. Windows that reveal
personal settings (e.g. Dictionary's list of the user's dictionaries) are
not recorded.

AppKit applications launched in the background and never activated answer
accessibility requests with the application element instead of their
window; activate such an application once before recording it.

### Recording

```bash
benchmarks/record.sh                     # the synthetic cases (form)
argus benchmark record --app Calculator --out benchmarks/dataset/calculator/1_cleared
argus benchmark review benchmarks/dataset/calculator/1_cleared -o review.png
```

`record` captures the window, stores its accessibility tree and drafts the
truth from it: the elements Argus's accessibility adapter reports, with
identities from names and native identifiers, and label relations the tree
states. `--extra-truth` adds elements the tree does not describe (the form's
canvas). **The draft must be reviewed** (`review` draws it over the frame)
and corrected by hand where the tree is wrong or ambiguous; corrections are
noted in the element's `note` or the case's `case.json`. The Calculator
cases were corrected so that the All Clear button keeps its identity when
it becomes Clear.

## Modes

| Mode            | Sources                        | Answers |
| --------------- | ------------------------------ | ------- |
| `accessibility` | the tree alone                 | What the platform says (an upper bound on the tree-derived truth). |
| `vision`        | visual detection alone         | What the model-free detector finds. |
| `pixels`        | OCR + visual detection         | What Argus recovers **without** the tree (Test B). |
| `full`          | all three, fused               | What Argus reports by default. |

Recorded steps are replayed through the real `Observer`: fusion, the scene
graph, tracking and incremental perception run as they do live; OCR is the
real Apple Vision.

## Metrics

Predicted and true elements are paired one to one by IoU ≥ 0.5 (largest
first); a true control still unpaired then takes a prediction of the same
role lying inside it (a checkbox found by its box while the tree's bounds
include its label — counted as "by containment").

| Metric          | Definition |
| --------------- | ---------- |
| recall          | Significant true elements paired. |
| precision       | Paired predictions / (paired + false positives). A false positive is an unpaired non-structural prediction; a prediction lying inside a true element (the text inside a field) is a *part* and neutral; one overlapping an already paired element is a *duplicate* and false. |
| role            | Paired elements with the true role. |
| name, value     | Exact text (whitespace collapsed); similarity = 1 − edit distance / length. |
| IoU, IoU ≥ .75  | Grounding of the pairs. |
| states          | `enabled`, `checked`, `selected`, `focused`, `expanded`: accuracy of reported values, and how many were not reported. |
| parent          | Paired elements whose true parent was found: predicted parent is its counterpart. |
| row membership  | Pairs of table cells: in the same predicted row exactly when in the same true row. |
| label relations | True `label_for` relations whose ends were found, reported. |
| identity        | Across consecutive steps, true elements found in both: same Argus ID (kept), another element's ID (swapped), or a new ID. |
| ECE             | Expected calibration error of `confidence.element` against being a true positive (10 bins). |
| latency         | Per stage, median over scored observations; by perception mode (`full`, `partial`, `unchanged`). |

`argus benchmark latency` measures the live pipeline on a running
application — capture and accessibility included — and the process's
resident memory: idle (before observing), steady (median of the second half)
and peak.

## Baseline (2026-09-19, Argus 0.0.1, macOS 27, Apple Silicon)

```text
mode            recall precision    role    name   value    IoU  IoU≥.75 identity     ECE
accessibility    90.0%    100.0%  100.0%  100.0%  100.0%  1.000   100.0%   100.0%   0.000
vision           63.1%     98.2%   83.3%    0.0%    0.0%  0.770    73.3%   100.0%   0.441
pixels           84.6%     96.1%   87.5%   65.3%   37.0%  0.678    57.1%    98.4%   0.341
full            100.0%     96.7%  100.0%  100.0%  100.0%  0.984    98.3%   100.0%   0.054

case          accessibility            vision              pixels                full
calculator   100% / 100% / 100%   92% / 100% / 99%   96% / 100% / 99%   100% / 100% / 100%
form_dark     82% / 100% / 100%   51% / 100% / 80%   85% / 100% / 88%   100% / 100% / 100%
form_light    82% / 100% / 100%   50% / 100% / 80%   85% / 100% / 88%   100% / 100% / 100%
textedit     100% / 100% / 100%   52% /  86% / 33%   57% /  68% / 38%   100% /  79% / 100%
                                                  (recall / precision / role)
```

- **The accessibility tree misses 10%** of what is on screen (the form's
  drawn canvas). **Fused, Argus finds all of it** (100% recall): the MVP
  thesis holds on this dataset.
- **Without the tree** (pixels) Argus finds 85%: on the form and the
  Calculator all text, all fields and 95% of buttons at 100% precision;
  missing are segmented controls' segments, the slider, a disabled checkbox
  and disabled window buttons. Symbol keys are named by their glyph (`×`)
  where the tree says `Multiply`. Controls get no `enabled` state, and
  checkboxes are grounded to their box.
- **TextEdit is the hard case**: its borderless text area is not detected
  from pixels (so the document's text cannot become its value), segmented
  format buttons are seen as one control and their labels as one line
  ("BIUS"), toolbar icons stay unnamed, and the ruler's digits are misread.
  The same three survive fusion as the false positives of `full` (79%
  precision on TextEdit): the misread ruler digits (visible text the tree
  does not describe), "BIUS", and the segmented groups detected as tabs
  next to the tree's checkboxes.
- **Identity**: tree-based tracking kept every element; pixel-only tracking
  swapped one (the Calculator display after "=") and renewed two.
- **Calibration**: visual detections are underconfident (confidence 0.5–0.7,
  97% correct); fused results are well calibrated (ECE 0.05).

Replay latency (recognition and detection real, capture from disk): full
perception 72–96 ms, incremental 20–23 ms, unchanged frames ~1 ms.

Live (`argus benchmark latency`, 20 observations):

| Window                 | accessibility | capture | OCR (steady) | total (median / p95) | first | memory idle / steady / peak |
| ---------------------- | ------------- | ------- | ------------ | -------------------- | ----- | --------------------------- |
| form, 640×472 pt       | 13 ms         | 74 ms   | 0 ms (unchanged) | 90 / 95 ms       | 306 ms | 10 / 157 / 175 MB |
| VS Code, 1470×923 pt   | 7 ms          | 92 ms   | 21 ms (partial)  | 126 / 247 ms     | 0.5 s¹ | 10 / 455 / 519 MB |

¹ 13.7 s when the binary's first OCR also prepares the models. Without OCR
the VS Code window needs 239 MB steady: capture and detection buffers of a
Retina window dominate memory; text recognition adds ~120 MB.

### History

- **2026-09-19, phase 14.1** — two fixes found by the first baseline:
  a text selection with its focus ring was detected as a button (a
  rectangle assembled from lines of two different shapes; such rectangles
  may no longer cross the border of a detected control), and the text in a
  detected field became separate text elements instead of the field's
  value (it now becomes the value, one run of text per line, with at most
  0.7 confidence because pixels cannot tell a placeholder from contents).
  On the cases used to make them: false positives 5 → 0 in every pixel
  mode (precision 97.5–98.4% → 100%), field values 0% → all text fields
  correct. On the held-out TextEdit case: **no change** — its errors are of
  other kinds (above). Detecting disabled controls by faint ink was tried
  and dropped: disabled buttons (contrast 62–63) and enabled Calculator
  operator keys (52–67) overlap.

## Limits

- Four cases, two of them one synthetic app: enough to catch regressions
  and to show where Argus stands, not a statistic of all interfaces.
- TextEdit has now been looked at: new fixes need new held-out cases.
- The truth of native apps is drafted from their accessibility tree, so the
  `accessibility` mode scores 100% on what it describes by construction;
  its value is the gap (the canvas) and the other modes.
- Replay latency excludes capture and accessibility; live latency depends
  on the machine and the window.

`crates/argus-benchmark/tests/dataset.rs` keeps the numbers from regressing
(`accessibility` and `vision` in CI; `pixels` and `full` with
`cargo test -p argus-benchmark -- --ignored`).
