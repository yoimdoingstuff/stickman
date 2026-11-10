"""Train the stickman's brain.

The brain is a tiny neural network (28 inputs -> 20 hidden -> 1 output,
about 600 numbers) that predicts how much the stickman will enjoy an action
in a situation. See src/learn.rs for what the inputs mean.

Two modes:

    python training/train.py pretrain
        Trains the *base* brain every new stickman starts with, from
        "common sense" situations (rest when tired, don't do flips when hurt,
        be nice to friends, show off when you're watching...). Training stops
        at a deliberate cutoff: it only learns the basics, so each stickman
        still has plenty to learn by itself and from you. Writes
        src/base_brain.txt, which gets built into the program. GitHub Actions
        runs this before every build.

    python training/train.py finetune
        With "Keep a training log" on, stickmen log everything they do and
        how it went to %APPDATA%\\stickman\\behaviour_log.jsonl. This starts
        from the base brain and learns from that log, writing
        %APPDATA%\\stickman\\base_brain.txt: new stickmen then start from what
        your stickmen have experienced. (Existing stickmen keep their own
        brains.)

Needs numpy:  pip install numpy
"""

import json
import os
import sys

try:
    import numpy as np
except ImportError:
    sys.exit("This needs numpy: pip install numpy")

CTX = 16
TAGS = 12
INPUTS = CTX + TAGS
HIDDEN = 20

HERE = os.path.dirname(os.path.abspath(__file__))
BASE_OUT = os.path.join(HERE, "..", "src", "base_brain.txt")
APPDATA = os.path.join(os.environ.get("APPDATA", "."), "stickman")

# Situation features (same order as src/learn.rs).
(BOREDOM, ENERGY, HEALTH, CURIOSITY, LAZINESS, MISCHIEF, PLAYFUL, INSIDE, CURSOR, FRIEND, ENEMY, SLEEPER,
 SHAKY, HEIGHT, ATTENTION, CBIAS) = range(CTX)
# Action tags (same order as src/brain.rs `tags`).
(ENERGETIC, RISKY, SOCIAL, AGGRESSIVE, RESTFUL, PLAYFUL_A, EXPLORE, SHOW_OFF, AFFECTION, AVOID, STATIONARY,
 TBIAS) = range(TAGS)

# The same table as `tags()` in src/brain.rs: energetic, risky, social,
# aggressive, restful, playful, explore, show-off, affectionate, avoid,
# stationary.
ACTIONS = {
    "Idle": [0.0, 0.0, 0.0, 0.0, 0.3, 0.0, 0.0, 0.0, 0.0, 0.0, 1.0],
    "Walk": [0.2, 0.0, 0.0, 0.0, 0.0, 0.0, 0.4, 0.0, 0.0, 0.0, 0.0],
    "Run": [0.8, 0.1, 0.0, 0.0, 0.0, 0.2, 0.6, 0.0, 0.0, 0.0, 0.0],
    "Sit": [0.0, 0.0, 0.0, 0.0, 0.8, 0.0, 0.0, 0.0, 0.0, 0.0, 1.0],
    "Wave": [0.1, 0.0, 0.5, 0.0, 0.0, 0.2, 0.0, 0.4, 0.2, 0.0, 0.5],
    "Dance": [0.6, 0.0, 0.1, 0.0, 0.0, 0.9, 0.0, 0.6, 0.0, 0.0, 0.5],
    "Jump": [0.6, 0.4, 0.0, 0.0, 0.0, 0.2, 0.8, 0.0, 0.0, 0.0, 0.0],
    "Enter": [0.3, 0.1, 0.0, 0.0, 0.0, 0.1, 1.0, 0.0, 0.0, 0.0, 0.0],
    "Flip": [0.7, 0.8, 0.0, 0.0, 0.0, 0.6, 0.0, 0.5, 0.0, 0.0, 0.3],
    "Climb": [0.7, 0.3, 0.0, 0.0, 0.0, 0.1, 0.8, 0.0, 0.0, 0.0, 0.0],
    "Drop": [0.2, 0.5, 0.0, 0.0, 0.0, 0.2, 0.5, 0.0, 0.0, 0.0, 0.0],
    "Layer": [0.1, 0.0, 0.0, 0.0, 0.0, 0.2, 0.3, 0.0, 0.0, 0.0, 0.3],
    "Follow": [0.3, 0.0, 0.2, 0.0, 0.0, 0.3, 0.1, 0.7, 0.0, 0.0, 0.0],
    "Cursor": [0.5, 0.4, 0.0, 0.0, 0.0, 0.6, 0.0, 1.0, 0.0, 0.0, 0.0],
    "Approach": [0.3, 0.0, 0.6, 0.0, 0.0, 0.0, 0.2, 0.0, 0.1, 0.0, 0.0],
    "Punch": [0.5, 0.3, 0.2, 1.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.3],
    "Kick": [0.6, 0.3, 0.2, 0.9, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.3],
    "Sleep": [0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 0.0, 1.0],
    "PushUps": [0.9, 0.0, 0.0, 0.0, 0.0, 0.1, 0.0, 0.3, 0.0, 0.0, 0.6],
    "Box": [0.6, 0.0, 0.0, 0.3, 0.0, 0.2, 0.0, 0.3, 0.0, 0.0, 0.5],
    "Meditate": [0.0, 0.0, 0.0, 0.0, 0.9, 0.0, 0.0, 0.1, 0.0, 0.0, 1.0],
    "EdgeSit": [0.0, 0.1, 0.0, 0.0, 0.6, 0.1, 0.0, 0.0, 0.0, 0.0, 0.9],
    "Cartwheel": [0.7, 0.5, 0.0, 0.0, 0.0, 0.7, 0.1, 0.5, 0.0, 0.0, 0.0],
    "Moonwalk": [0.4, 0.0, 0.0, 0.0, 0.0, 0.9, 0.1, 0.7, 0.0, 0.0, 0.0],
    "Sprint": [1.0, 0.1, 0.0, 0.0, 0.0, 0.3, 0.4, 0.0, 0.0, 0.0, 0.0],
    "Trip": [0.2, 0.6, 0.0, 0.0, 0.0, 0.5, 0.0, 0.2, 0.0, 0.0, 0.3],
    "Tantrum": [0.4, 0.2, 0.0, 0.1, 0.0, 0.5, 0.0, 0.3, 0.0, 0.0, 0.6],
    "PlayDead": [0.0, 0.0, 0.0, 0.0, 0.3, 0.6, 0.0, 0.3, 0.0, 0.0, 1.0],
    "Wrap": [0.8, 0.1, 0.0, 0.0, 0.0, 0.6, 0.5, 0.2, 0.0, 0.0, 0.0],
    "Spin": [0.4, 0.1, 0.0, 0.0, 0.0, 0.8, 0.0, 0.4, 0.0, 0.0, 0.8],
    "Panic": [0.9, 0.1, 0.0, 0.0, 0.0, 0.7, 0.0, 0.3, 0.0, 0.3, 0.0],
    "Slide": [0.7, 0.4, 0.0, 0.0, 0.0, 0.7, 0.1, 0.3, 0.0, 0.0, 0.0],
    "Leap": [0.8, 0.6, 0.0, 0.0, 0.0, 0.5, 0.0, 0.5, 0.0, 0.0, 0.2],
    "Wake": [0.2, 0.0, 0.6, 0.0, 0.0, 0.2, 0.0, 0.0, 0.3, 0.0, 0.0],
    "Poke": [0.1, 0.0, 0.7, 0.1, 0.0, 0.3, 0.0, 0.0, 0.2, 0.0, 0.5],
    "Swing": [0.7, 0.5, 0.0, 0.0, 0.0, 0.8, 0.1, 0.4, 0.0, 0.0, 0.0],
    "HighFive": [0.2, 0.0, 1.0, 0.0, 0.0, 0.3, 0.0, 0.1, 1.0, 0.0, 0.4],
    "HangOut": [0.1, 0.0, 0.8, 0.0, 0.3, 0.1, 0.0, 0.0, 0.6, 0.0, 0.0],
    "Avoid": [0.3, 0.0, 0.0, 0.0, 0.0, 0.0, 0.1, 0.0, 0.0, 1.0, 0.0],
}

# The cutoff: training stops once it gets this close to "common sense", or
# after this many passes, whichever comes first. Deliberately loose, so it
# only knows the basics and does the rest of its learning on your desktop.
CUTOFF_LOSS = 0.045
MAX_EPOCHS = 25


def sample_situations(rng, n):
    """Random but plausible situations."""
    c = np.zeros((n, CTX))
    c[:, BOREDOM] = rng.uniform(0, 1, n)
    c[:, ENERGY] = rng.uniform(0, 1, n)
    hurt = rng.uniform(0, 1, n) < 0.3
    c[:, HEALTH] = np.where(hurt, rng.uniform(0.0, 0.7, n), rng.uniform(0.7, 1.0, n))
    for f in (CURIOSITY, LAZINESS, MISCHIEF, PLAYFUL):
        c[:, f] = rng.uniform(0.1, 0.9, n)
    c[:, INSIDE] = rng.uniform(0, 1, n) < 0.3
    c[:, CURSOR] = rng.uniform(0, 1, n) < 0.25
    c[:, FRIEND] = np.where(rng.uniform(0, 1, n) < 0.4, rng.uniform(0, 1, n), 0)
    c[:, ENEMY] = np.where(rng.uniform(0, 1, n) < 0.25, rng.uniform(0, 1, n), 0)
    c[:, SLEEPER] = rng.uniform(0, 1, n) < 0.1
    c[:, SHAKY] = np.where(rng.uniform(0, 1, n) < 0.15, rng.uniform(0, 1, n), 0)
    c[:, HEIGHT] = rng.uniform(0, 1, n) ** 2
    c[:, ATTENTION] = np.where(rng.uniform(0, 1, n) < 0.3, rng.uniform(0, 1, n), 0)
    c[:, CBIAS] = 1
    return c


def common_sense(c, a):
    """How good an action is in a situation, by the basic rules every
    stickman starts out knowing."""
    hurt = 1 - c[:, HEALTH]
    r = np.zeros(len(c))
    r += a[:, ENERGETIC] * (c[:, BOREDOM] * 0.6 + (c[:, ENERGY] - 0.4) * 0.8 - hurt * 0.4)
    r += a[:, RESTFUL] * ((1 - c[:, ENERGY]) * 0.9 + hurt * 0.6 - c[:, BOREDOM] * 0.4)
    r -= a[:, RISKY] * (hurt * 1.2 + c[:, SHAKY] * 0.8 + c[:, HEIGHT] * 0.2 + 0.1)
    r += a[:, RISKY] * c[:, PLAYFUL] * 0.3
    r += a[:, SOCIAL] * (c[:, FRIEND] * 0.8 - c[:, ENEMY] * 0.3)
    r += a[:, AFFECTION] * (c[:, FRIEND] * 0.9 - c[:, ENEMY] * 0.8)
    r += a[:, AGGRESSIVE] * (c[:, ENEMY] * (0.4 + c[:, MISCHIEF] * 0.5) - c[:, FRIEND] * 0.9 - hurt * 0.5)
    r += a[:, AVOID] * (c[:, ENEMY] * hurt * 0.9 - 0.1)
    r += a[:, PLAYFUL_A] * (c[:, PLAYFUL] * 0.5 + c[:, BOREDOM] * 0.3 - 0.15)
    r += a[:, EXPLORE] * (c[:, CURIOSITY] * 0.6 + c[:, BOREDOM] * 0.3 - c[:, LAZINESS] * 0.3 - 0.1)
    r += a[:, SHOW_OFF] * (c[:, CURSOR] * 0.5 + c[:, ATTENTION] * 0.4 - 0.15)
    r += a[:, STATIONARY] * (c[:, LAZINESS] * 0.3 - c[:, BOREDOM] * 0.3)
    return np.clip(r, -1.5, 1.5)


class Net:
    def __init__(self, rng):
        self.w1 = rng.normal(0, 0.3, (HIDDEN, INPUTS))
        self.b1 = np.zeros(HIDDEN)
        self.w2 = rng.normal(0, 0.1, HIDDEN)
        self.b2 = 0.0

    def predict(self, x):
        h = np.tanh(x @ self.w1.T + self.b1)
        return h @ self.w2 + self.b2, h

    def train_epoch(self, x, y, rng, lr=0.03, batch=64, weight=None):
        order = rng.permutation(len(x))
        for i in range(0, len(x), batch):
            idx = order[i:i + batch]
            xb, yb = x[idx], y[idx]
            wb = np.ones(len(idx)) if weight is None else weight[idx]
            pred, h = self.predict(xb)
            err = np.clip(pred - yb, -1, 1) * wb
            n = len(idx)
            dw2 = h.T @ err / n
            db2 = err.mean()
            dh = np.outer(err, self.w2) * (1 - h * h)
            dw1 = dh.T @ xb / n
            db1 = dh.mean(axis=0)
            self.w2 -= lr * (dw2 + 1e-4 * self.w2)
            self.b2 -= lr * db2
            self.w1 -= lr * (dw1 + 1e-4 * self.w1)
            self.b1 -= lr * db1

    def loss(self, x, y):
        pred, _ = self.predict(x)
        return float(np.mean((pred - y) ** 2))

    def save(self, path, note):
        def line(name, v):
            return name + " " + " ".join(f"{float(x):.5f}" for x in np.ravel(v)) + "\n"

        os.makedirs(os.path.dirname(os.path.abspath(path)), exist_ok=True)
        with open(path, "w", encoding="utf-8") as f:
            f.write(f"# {note}\n")
            f.write(f"stickman-net {INPUTS} {HIDDEN}\n")
            # Row-major: hidden neuron j's weights are w1[j*INPUTS .. (j+1)*INPUTS].
            f.write(line("w1", self.w1))
            f.write(line("b1", self.b1))
            f.write(line("w2", self.w2))
            f.write(line("b2", [self.b2]))

    @staticmethod
    def load(path, rng):
        net = Net(rng)
        nums = {}
        with open(path, encoding="utf-8") as f:
            for raw in f:
                parts = raw.split()
                if parts and parts[0] in ("w1", "b1", "w2", "b2"):
                    nums[parts[0]] = np.array([float(v) for v in parts[1:]])
        net.w1 = nums["w1"].reshape(HIDDEN, INPUTS)
        net.b1 = nums["b1"]
        net.w2 = nums["w2"]
        net.b2 = float(nums["b2"][0])
        return net


def pretrain():
    rng = np.random.default_rng(1234)
    tags = np.array([v + [1.0] for v in ACTIONS.values()])

    def dataset(n):
        c = sample_situations(rng, n)
        a = tags[rng.integers(0, len(tags), n)]
        y = common_sense(c, a) + rng.normal(0, 0.08, n)
        return np.hstack([c, a]), y

    x, y = dataset(40000)
    xv, yv = dataset(5000)
    net = Net(rng)
    for epoch in range(1, MAX_EPOCHS + 1):
        net.train_epoch(x, y, rng)
        val = net.loss(xv, yv)
        print(f"epoch {epoch:2d}  validation loss {val:.4f}")
        if val <= CUTOFF_LOSS:
            print(f"Reached the cutoff ({CUTOFF_LOSS}): it knows the basics. Stopping here.")
            break
    net.save(BASE_OUT, f"Base brain: common sense up to the cutoff (validation loss {val:.4f}).")
    print(f"Wrote {os.path.normpath(BASE_OUT)}")


def finetune():
    rng = np.random.default_rng()
    rows = []
    for name in ("behaviour_log.old.jsonl", "behaviour_log.jsonl"):
        path = os.path.join(APPDATA, name)
        if not os.path.exists(path):
            continue
        with open(path, encoding="utf-8") as f:
            for line in f:
                try:
                    row = json.loads(line)
                except json.JSONDecodeError:
                    continue
                if len(row.get("x", [])) == CTX and len(row.get("a", [])) == TAGS:
                    rows.append(row)
    if len(rows) < 50:
        sys.exit(f"Only {len(rows)} logged experiences. Turn on 'Keep a training log' and let them play for a while.")
    x = np.array([r["x"] + r["a"] for r in rows])
    y = np.array([r["r"] for r in rows])
    # Your praise and scolding count for more.
    w = np.array([3.0 if r.get("taught") else 1.0 for r in rows])
    custom = os.path.join(APPDATA, "base_brain.txt")
    start = custom if os.path.exists(custom) else BASE_OUT
    try:
        net = Net.load(start, rng)
    except (OSError, KeyError, ValueError):
        net = Net(rng)
    for epoch in range(1, 9):
        net.train_epoch(x, y, rng, lr=0.01, weight=w)
        print(f"epoch {epoch}  loss {net.loss(x, y):.4f}")
    net.save(custom, f"Fine-tuned on {len(rows)} experiences from your stickmen.")
    print(f"Wrote {custom}. New stickmen will start from it.")


if __name__ == "__main__":
    mode = sys.argv[1] if len(sys.argv) > 1 else ""
    if mode == "pretrain":
        pretrain()
    elif mode == "finetune":
        finetune()
    else:
        sys.exit("Usage: python training/train.py pretrain | finetune")
