# A live pair of series for the pixel plot: one JSON line per tick, drifting waves with a little
# noise, so the plot is re-rasterised continuously.   wave.py [ticks per second]
import json, math, random, sys, time
hz = float(sys.argv[1]) if len(sys.argv) > 1 else 15
t0, n = time.time(), 0
while True:
    n += 1; t = n / hz
    done = [5 + 3 * math.sin(i / 4 + t * 0.9) + 1.2 * math.sin(i / 1.7 + t * 2.1) + random.gauss(0, 0.15) for i in range(48)]
    added = [5.5 + 2.2 * math.sin(i / 5 + t * 0.6 + 1.3) + 0.8 * math.sin(i / 2.3 - t * 1.4) for i in range(48)]
    sys.stdout.write(json.dumps({"done": done, "added": added, "tick": n}) + "\n"); sys.stdout.flush()
    d = t0 + n / hz - time.time()
    if d > 0: time.sleep(d)
