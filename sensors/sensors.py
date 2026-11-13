# sensors.py: reads the garden sensors and answers the Rust program.
# Rust sends one JSON command per line on stdin; we answer with one JSON line on stdout.
# Test it: python sensors.py --fake, then type {"cmd":"read"} and press Enter.
import json, math, random, sys

UNITS = {"soil": "%", "temp": "°C"}
FAKE = "--fake" in sys.argv


def read_all() -> dict[str, float | None]:
    # read each sensor; if one fails, return None for it instead of crashing
    if FAKE:
        values = {"soil": round(random.uniform(20, 60), 1), "temp": round(random.uniform(5, 30), 1)}
        if random.random() < 0.1:
            values[random.choice(list(values))] = None  # pretend a sensor failed
        return values
    # TODO: read the real sensors
    return {name: None for name in UNITS}


for line in sys.stdin:
    cmd = json.loads(line)["cmd"]
    if cmd == "describe":
        print(json.dumps(UNITS), flush=True)
    elif cmd == "read":
        # JSON has no NaN or Infinity (Rust would reject the whole line), so send None instead.
        values = {name: v if v is None or math.isfinite(v) else None for name, v in read_all().items()}
        print(json.dumps(values), flush=True)
