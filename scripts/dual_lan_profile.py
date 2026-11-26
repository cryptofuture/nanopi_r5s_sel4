import ipaddress
import os

STAGES = {
    "lan1-rx": "lan1",
    "lan1-icmp": "lan1",
    "lan2-tx": "lan2",
    "lan2-ethernet": "lan2",
    "dual-lan-bridge": "both",
    "dual-lan-dhcp": "both",
    "wan-dhcp": "both",
    "dual-lan-router": "both",
}


def services(stage):
    result = ["wan-b51-status", "lan2-tx-diagnostic" if stage == "lan2-tx" else "lan-ethernet"]
    if stage not in ("lan1-rx", "lan2-tx"):
        result.append("lan-arp-icmp")
    if STAGES[stage] == "both":
        result.append("lan-bridge")
    if stage in ("dual-lan-dhcp", "wan-dhcp", "dual-lan-router"):
        result.append("lan-shared-dhcp")
    if stage == "dual-lan-router":
        result += ["ipv4-nat", "lan-front-leds"]
    return result


DEFAULT_LAN_DNS = ("77.88.8.8", "77.88.8.1")


def dns_option():
    value = os.environ.get("R5S_LAN_DNS")
    if value is None:
        parts = DEFAULT_LAN_DNS
    else:
        parts = [part.strip() for part in value.split(",") if part.strip()]
    values = [int(ipaddress.IPv4Address(part)) for part in parts]
    if len(values) > 3 or any(not valid_dns(value) for value in values):
        raise ValueError("LAN DNS requires up to three unicast IPv4 addresses")
    return values


def valid_dns(address):
    if address is None:
        return True
    values = [address] if type(address) is int else address if isinstance(address, list) else None
    return (values is not None and len(values) <= 3 and
            all(type(value) is int and 0 < value < 0xe0000000 and value >> 24 not in (0, 127)
                for value in values))


def fallback_option():
    value = os.environ.get("R5S_WAN_DIAG_FALLBACK")
    if value is None:
        return 0
    address = int(ipaddress.IPv4Address(value))
    if not valid_fallback(address) or address == 0:
        raise ValueError("WAN fallback requires a 169.254.1.1–169.254.254.254 link-local /24 host address")
    return address


def valid_fallback(address):
    return type(address) is int and (address == 0 or
        (address >> 16 == 0xa9fe and 1 <= (address >> 8) & 255 <= 254 and 1 <= address & 255 <= 254))


def valid_tso_request(requested, ipv6, stage):
    return (stage == 'dual-lan-router' and isinstance(requested, list) and bool(requested)
            and requested == [p for p in ('wan', 'lan1', 'lan2') if p in requested]
            and type(ipv6) is bool)


ROUTER_SCHEDULE = {
    "baseline": {"lan1": 50, "lan2": 50, "logical_lan": 100},
    "throughput": {"lan1": 75, "lan2": 75, "logical_lan": 140},
    "quad-core": {"gmac": 400, "wan": 400, "lan1": 800, "lan2": 800, "logical_lan": 600},
}


ROUTER_SCHEDULE["multicore"] = {
    **ROUTER_SCHEDULE["quad-core"], "lan1": 650, "lan2": 650,
    "logical_lan": 440, "forward0": 175, "forward1": 200, "forward2": 300, "forward3": 300,
}

WORKER_SCHEDULES = ("multicore", "high-throughput", "bidirectional", "wan-throughput",
                    "wan-throughput-isolated", "wan-throughput-isolated-upload",
                    "wan-throughput-isolated-workers", "wan-throughput-isolated-workers-max",
                    "wan-throughput-isolated-workers2")
SMP_SCHEDULES = ("quad-core", *WORKER_SCHEDULES)
ROUTER_SCHEDULE["high-throughput"] = {
    **ROUTER_SCHEDULE["multicore"], "gmac": 480, "wan": 250, "forward0": 240,
    "logical_lan": 490, "forward1": 180,
}

ROUTER_SCHEDULE["bidirectional"] = {
    **ROUTER_SCHEDULE["high-throughput"], "gmac": 670, "wan": 150,
    "forward0": 300, "logical_lan": 350,
}


ROUTER_SCHEDULE["wan-throughput"] = {
    **ROUTER_SCHEDULE["bidirectional"], "gmac": 710, "forward0": 260,
    "wan": 130, "logical_lan": 330, "forward1": 220,
}

# GMAC contention with forward0 isolated: GMAC keeps its 710 µs budget and
# moves to CPU 1 with only the supervisors (health yields 10 µs to fit the
# 1 ms period); logical_lan, wan and the two upper workers move to CPU 0.
# All router budgets match wan-throughput.
ROUTER_SCHEDULE["wan-throughput-isolated"] = {
    **ROUTER_SCHEDULE["wan-throughput"], "health": 90,
}

# Upload-focused variant of the isolated placement. A single LAN→WAN flow is
# pinned to one worker, so that worker's budget caps upload: the dispatcher
# (logical_lan) and the WAN app sit out the LAN fast path and only need
# control-plane time (DHCP/ARP/learn/fallback), so their CPU 0 slices shrink
# and the two upper workers take the freed time. GMAC and the LAN drivers
# keep their budgets.
# NOTE (tested, regressed download): shrinking logical_lan/wan below their
# wan-throughput values starves the download fallback dispatcher and the WAN
# app. Keep this definition as the record of that experiment; use
# wan-throughput-isolated-workers* instead.
ROUTER_SCHEDULE["wan-throughput-isolated-upload"] = {
    **ROUTER_SCHEDULE["wan-throughput-isolated"],
    "logical_lan": 160, "wan": 80, "forward0": 400, "forward1": 300,
}

# Upload worker budgets without starving the download path: logical_lan (the
# fallback dispatcher) and wan keep their wan-throughput budgets. The upper
# workers take only CPU 0's unused schedulability headroom (940 -> 1000 µs).
ROUTER_SCHEDULE["wan-throughput-isolated-workers"] = {
    **ROUTER_SCHEDULE["wan-throughput-isolated"],
    "forward0": 320,
}

# Max shape of the same idea: the supervisor's spin loop is cosmetic (LED +
# 10 ms-granularity health), so it yields most of its CPU 1 slice to the WAN
# app (which sits off the fast path and keeps its 130 µs), and the freed CPU 0
# slot goes to forward0. Download-path budgets (logical_lan 330, wan 130) and
# the LAN drivers are unchanged.
ROUTER_SCHEDULE["wan-throughput-isolated-workers-max"] = {
    **ROUTER_SCHEDULE["wan-throughput-isolated"],
    "heartbeat": 60, "forward0": 450,
}

# Middle rung of the worker-budget ladder: forward0 takes 120 µs more than
# workers while everything else (including the WAN app and the supervisor)
# keeps its tested placement and budget — none of workers-max's residual
# risks. Forward1 funds it, which only multi-flow/8-stream setups feel.
ROUTER_SCHEDULE["wan-throughput-isolated-workers2"] = {
    **ROUTER_SCHEDULE["wan-throughput-isolated"],
    "forward0": 380, "forward1": 160,
}


def b51_memory(ring="64", cache="nc", count=None):
    """Private/shared DMA placement for one ring+cache configuration.

    Placement follows the buffer count when given: a 2 MiB shared pool area
    at 0x24600000 (lan2 after it) holds 1024 buffers; otherwise the legacy
    cache-keyed choice applies (write-back: 2 MiB area; nc: 1 MiB at
    0x24500000). Cache policy only decides mapping attributes, not placement,
    so a cache A/B can keep identical physical addresses.
    """
    wide = ring in ("256", "512")
    expanded = count == 1024 or (count is None and cache == "writeback")
    if wide and expanded:
        return (
            [0x62000000, 0x24200000, 0x80000000, 0x300000],
            [0x62000000, 0x24800000, 0x81200000, 0x300000],
            [0x24600000, 0x80400000, 0x81000000, 0x24b00000],
        )
    if wide:
        return (
            [0x62000000, 0x24200000, 0x80000000, 0x300000],
            [0x62000000, 0x24600000, 0x81100000, 0x300000],
            [0x24500000, 0x80300000, 0x81000000, 0x24900000],
        )
    return (
        [0x62000000, 0x24200000, 0x80000000, 0x100000],
        [0x62000000, 0x24300000, 0x81000000, 0x100000],
        [0x24400000, 0x82000000, 0x81100000, 0x24500000],
    )


def zc_pool_manifest(options):
    ring = options.get("R5S_B51_RING", "64")
    cache = options.get("R5S_ZC_CACHE", "nc")
    wide = ring in ("256", "512")
    requested = options.get("R5S_ZC_POOL_BUFFERS")
    legacy_expanded = options.get("R5S_ZC_EXPAND") == "1"
    if requested is not None and legacy_expanded:
        raise ValueError("Conflicting shared DMA pool options")
    count = requested if requested is not None else (
        "1024" if legacy_expanded or not wide else "512"
    )
    if (count not in ("512", "1024") or (not wide and count != "1024")
            or (legacy_expanded and ring != "256")):
        raise ValueError("Shared DMA pool size does not fit the selected ring and cache layout")
    lan1, lan2, (pa, lan1_bus, lan2_bus, _) = b51_memory(ring, cache, int(count))
    size = int(count) * 2048
    if wide and count == "1024" and not (
        pa + size == lan2[1]
        and lan1_bus - lan1[2] == pa - lan1[1]
        and lan2[2] - lan2_bus == size
    ):
        raise ValueError("Expanded shared DMA pool does not fit both B51 inbound apertures")
    return {"va": 0x66000000, "pa": pa, "size": size, "gmac_dma": pa,
            "lan1_dma": lan1_bus, "lan2_dma": lan2_bus}


def b51_rx_fits(ring, queues, tx_queues=1):
    if ring not in ("64", "128", "256", "512") or queues not in ("1", "2", "4"):
        return False
    slots, count = int(ring), int(queues)
    tx_bytes = ((slots + 1) * 32 + 255) & ~255
    rx_bytes = ((slots + 1) * (32 if count > 1 else 16) + 255) & ~255
    payload_offset = (tx_queues * tx_bytes + count * rx_bytes + 4095) & ~4095
    lan1, _, _ = b51_memory(ring)
    return payload_offset + slots * (count + tx_queues) * 2048 <= lan1[3]


def optimization_options():
    options = {}
    for name, values in {
        "R5S_DATAPATH_BATCH": ("8", "16", "32", "64"),
        "R5S_B51_RX_BATCH": ("8", "16", "32", "64", "128"),
        "R5S_B51_TX_BATCH": ("8", "16", "32", "64"),
        "R5S_B51_RING": ("64", "128", "256", "512"),
        "R5S_B51_TX_QUEUES": ("1", "2"),
        "R5S_B51_RX_QUEUES": ("1", "2", "4"),
        "R5S_B51_MPS": ("128", "256", "512", "1024", "2048", "4096"),
        "R5S_B51_MRRS": ("512", "1024", "2048"),
        "R5S_B51_DMA_BARRIERS": ("full", "outer"),
        "R5S_ZC_DMA_BARRIERS": ("full", "outer"),
        "R5S_GMAC_RING": ("64", "256"),
        "R5S_GMAC_DMA_CACHE": ("writeback", "nc"),
        "R5S_GMAC_RX_CHANNELS": ("1", "2"),
        "R5S_GMAC_TX_CHANNELS": ("1", "2"),
        "R5S_GMAC_TX_BATCH": ("0", "1"),
        "R5S_GMAC_TX_EXTRA_BATCH": ("0", "8", "16", "32", "64"),
        "R5S_GMAC_PBL": ("8", "16", "32"),
        "R5S_GMAC_PBLX8": ("0", "1"),
        "R5S_GMAC_EAME": ("0", "1"),
        "R5S_GMAC_RK3568_AXI": ("0", "1"),
        "R5S_GMAC_COPIED_CHECKSUM": ("0", "1"),
        "R5S_GMAC_WORKER_TX": ("0", "1"),
        "R5S_GMAC_HYBRID_IRQ": ("0", "1"),
        "R5S_QUEUE_PAD": ("0", "1"),
        "R5S_IDLE_YIELD": ("0", "1"),
        "R5S_ZC_CACHE": ("nc", "writeback"),
        "R5S_ZC_POOL_BUFFERS": ("512", "1024"),
        "R5S_ZERO_COPY": ("0", "1"),
        "R5S_ZC_TX_CHECKSUM": ("0", "1"),
        "R5S_GRO": ("0", "1"),
        "R5S_WORKER_GRO": ("0", "1"),
        "R5S_DRIVER_GRO": ("0", "1"),
        "R5S_GRO_HOLD_US": ("100", "250", "500", "1000"),
        "R5S_GRO_TSO": ("0", "1"),
        "R5S_TSO_TEST_SOFTWARE": ("0", "1"),
        "R5S_WAN_FAST_PATH": ("0", "1"),
        "R5S_LAN_FAST_PATH": ("0", "1"),
        "R5S_WORKER_WAN_EGRESS": ("0", "1"),
        "R5S_WORKER_LAN_EGRESS": ("0", "1"),
        "R5S_WORKER_LAN_PREP": ("0", "1"),
        "R5S_FIRMWARE_LOG": ("0", "1"),
        "R5S_LED_FAULTS": ("0", "1"),
        "R5S_NAT_HASH": ("0", "1"),
        "R5S_NAT_PAIR_LOOKUP": ("0", "1"),
        "R5S_BRIDGE_LOCAL_HASH": ("0", "1"),
        "R5S_A55_OPT": ("0", "1"),
        "R5S_SDK_CONFIG": ("debug", "release", "benchmark"),
    }.items():
        value = os.environ.get(name)
        if value is not None:
            if value not in values:
                raise ValueError(f"Invalid {name}: {value}")
            options[name] = value
    return options


def scheduling_profile(stage):
    profile = os.environ.get("R5S_ROUTER_SCHEDULE", "baseline")
    if profile not in ROUTER_SCHEDULE or (profile != "baseline" and stage != "dual-lan-router"):
        raise ValueError("Invalid R5S_ROUTER_SCHEDULE for checkpoint")
    return profile


def configure_schedule(system, profile):
    domains = {pd.get("name"): pd for pd in system.findall("protection_domain")}
    for name, budget in ROUTER_SCHEDULE[profile].items():
        domains[name].set("budget", str(budget))
    placement = {"heartbeat": 1, "health": 1, "logical_lan": 1, "lan1": 2, "lan2": 3}
    placement.update({f"forward{i}": i for i in range(4)})
    if profile in ("bidirectional", "wan-throughput", "wan-throughput-isolated",
                   "wan-throughput-isolated-upload", "wan-throughput-isolated-workers",
                   "wan-throughput-isolated-workers-max", "wan-throughput-isolated-workers2"):
        placement["wan"] = 1
    if profile in ("wan-throughput-isolated", "wan-throughput-isolated-upload",
                   "wan-throughput-isolated-workers", "wan-throughput-isolated-workers-max",
                   "wan-throughput-isolated-workers2"):
        # GMAC alone with the supervisors on CPU 1; CPU 0 hosts the
        # dispatcher, WAN app and the two upper workers.
        placement["gmac"] = 1
        placement["wan"] = 0
        placement["logical_lan"] = 0
        placement["forward0"] = 0
        placement["forward1"] = 0
    if profile == "wan-throughput-isolated-workers-max":
        # The supervisor spin loop yields its slice; the WAN app keeps its
        # budget on CPU 1 so CPU 0 can give forward0 the whole remainder.
        placement["wan"] = 1
    smp = profile in SMP_SCHEDULES
    expected = {name: placement.get(name, 0) if smp else 0 for name in domains}
    if smp:
        for name, core in expected.items():
            domains[name].set("cpu", str(core))
    totals = [0] * (4 if smp else 1)
    for name, pd in domains.items():
        cpu, budget = int(pd.get("cpu", "0")), int(pd.get("budget"))
        if cpu != expected[name] or pd.get("period") != "1000" or budget <= 0:
            raise ValueError("Unexpected CPU placement, period or budget")
        totals[cpu] += budget
    if any(total > 1000 for total in totals):
        raise ValueError("Router schedule exceeds one CPU")
