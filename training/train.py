"""Train the stickman's "what you like" model.

Turn on "Learn what I like" in the stickman's settings and let it run for a
while (a few hundred decisions is a good start). Then run:

    pip install numpy
    python training/train.py

It reads %APPDATA%\\stickman\\behaviour_log.jsonl and writes
%APPDATA%\\stickman\\brain_model.txt. Restart the stickman to use it.

The model: for each kind of action, a small logistic regression predicting
"did you react to it" (picked it up, moved your mouse over to it) from the
situation it was in (its mood, health, whether it was inside a window, ...).
The stickman then leans towards actions you tend to react to.
"""

import collections
import json
import os
import sys

try:
    import numpy as np
except ImportError:
    sys.exit("This needs numpy: pip install numpy")

FEATURES = 10
MIN_EXAMPLES = 8
APPDATA = os.environ.get("APPDATA", ".")
LOG = os.path.join(APPDATA, "stickman", "behaviour_log.jsonl")
OLD_LOG = os.path.join(APPDATA, "stickman", "behaviour_log.old.jsonl")
OUT = os.path.join(APPDATA, "stickman", "brain_model.txt")


def load(paths):
    rows = []
    for path in paths:
        if not os.path.exists(path):
            continue
        with open(path, encoding="utf-8") as f:
            for line in f:
                line = line.strip()
                if not line:
                    continue
                try:
                    row = json.loads(line)
                except json.JSONDecodeError:
                    continue
                if len(row.get("x", [])) == FEATURES:
                    rows.append(row)
    return rows


def fit(x, y, steps=3000, lr=0.5, l2=0.02):
    """Plain logistic regression by gradient descent (with a little L2 so a
    handful of examples can't produce wild weights)."""
    w = np.zeros(x.shape[1])
    b = 0.0
    for _ in range(steps):
        p = 1.0 / (1.0 + np.exp(-(x @ w + b)))
        g = p - y
        w -= lr * (x.T @ g / len(y) + l2 * w)
        b -= lr * g.mean()
    return b, w


def main():
    paths = sys.argv[1:] or [OLD_LOG, LOG]
    rows = load(paths)
    if not rows:
        sys.exit(f"No log found at {LOG}. Turn on 'Learn what I like' and let it run for a while.")

    by_kind = collections.defaultdict(list)
    for row in rows:
        by_kind[row["kind"]].append((row["x"], 1.0 if row.get("engagement", 0) >= 0.3 else 0.0))

    lines = [f"features {FEATURES}"]
    for kind, data in sorted(by_kind.items()):
        if len(data) < MIN_EXAMPLES:
            continue
        x = np.array([d[0] for d in data], dtype=float)
        y = np.array([d[1] for d in data], dtype=float)
        b, w = fit(x, y)
        lines.append(kind + " " + " ".join(f"{v:.5f}" for v in [b, *w]))
        print(f"{kind:12s} {len(data):5d} examples, reacted to {y.mean():.0%}")

    os.makedirs(os.path.dirname(OUT), exist_ok=True)
    with open(OUT, "w", encoding="utf-8") as f:
        f.write("\n".join(lines) + "\n")
    print(f"\nTrained on {len(rows)} decisions ({len(lines) - 1} kinds of action) -> {OUT}")
    print("Restart the stickman to use it.")


if __name__ == "__main__":
    main()
