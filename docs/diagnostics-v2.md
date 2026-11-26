# Diagnostics v2

Generated from `protocol/diagnostics-v2.json`. Durations are nominal milliseconds.

| State | ON/OFF sequence |
|---|---|
| WAIT | ON 2000; OFF 2000 |
| READY | ON 200; OFF 3800 |
| IO_VERIFIED | ON 2500; OFF 500; ON 200; OFF 2800 |
| FAULT | ON 125; OFF 125; ON 125; OFF 125; ON 125; OFF 125; ON 125; OFF 125; ON 125; OFF 125; ON 125; OFF 125; ON 125; OFF 125; ON 125; OFF 4125 |

Every state change starts with 500 ms OFF. The FAULT sequence includes the final 125 ms OFF plus 4000 ms darkness.

State selection runs every scheduler opportunity, before pattern progression. Required health expires after 100 ms; proof expires at 5000 ms. Expected scheduling bound is 10 ms, including the delimiter gives at most 510 ms to the first fault indication. A scheduler/kernel failure can freeze the LED. Partial frames, solid ON and solid OFF are inconclusive.

READY means profile checks pass, never packet proof. IO_VERIFIED requires a matching fresh external ACK and is revoked on link loss, health failure or lease expiry. FAULT takes priority. Baseline has no IO_VERIFIED path.

Timeline preview: each row below is one period; `#` is ON and `.` is OFF, one character per 125 ms (rounded for preview only).

    WAIT        ################................
    READY       ##..............................
    IO_VERIFIED ####################....##......................
    FAULT       #.#.#.#.#.#.#.#.................................
