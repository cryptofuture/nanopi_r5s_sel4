MILESTONES = {
    1: "Endpoint detected", 2: "B51 identified", 3: "DMA pool validated", 4: "Inbound aperture accepted",
    5: "MAC reset complete", 6: "PHY configured", 7: "PHY link ready",
    8: "Descriptor rings configured", 9: "PCI bus mastering enabled", 10: "RX frame accepted",
    16: "B51 driver selected", 20: "Unsupported silicon", 21: "TX submitted",
    22: "TX completed", 23: "TX timed out", 24: "MAC error status",
    25: "Root PCI error status", 26: "Endpoint PCI error status", 27: "Status access error",
    28: "Tally DMA write observed", 29: "Tally completed without a memory change",
    30: "Tally timeout or precondition failure", 31: "Tally DMA guard modified",
}
RESULTS = {
    65: "Counter frequency or reference clock", 66: "Clock gate readback", 67: "PIPE power timeout",
    68: "PIPE idle timeout", 69: "Reset or GPIO readback", 70: "PHY readiness", 71: "PCIe link training",
    72: "PCI configuration access", 73: "BAR setup", 74: "Unsupported chip", 75: "MMIO mapping or access",
    160: "DMA pool validation", 161: "Inbound iATU mapping or aperture audit rejected",
    162: "iATU enable or disable timeout", 163: "MAC or PHY initialization", 164: "Firmware or PHY handshake",
    165: "PHY link timeout", 166: "Descriptor or queue setup", 167: "Diagnostic finished",
    168: "RX timeout", 169: "TX timeout", 170: "Shutdown/quiescence failure",
    171: "MAC, DMA or TX error", 172: "No matching ARP", 173: "Bus mastering setup",
}
PHASES = {0: "Waiting", 1: "PCIe shared initialization", 2: "Endpoint discovery", 3: "MAC reset",
          4: "Firmware", 5: "PHY", 8: "Root reset", 9: "Root PHY ready", 10: "Link training",
          11: "BAR identification", 12: "LAN1 setup", 13: "Inbound iATU", 255: "Terminal"}
OPERATIONS = {1: "Validate window", 2: "Check iATU mode", 3: "Disable bank",
              4: "Probe alignment", 5: "Probe low-limit width", 6: "Program bank",
              7: "Enable bank", 8: "Audit window and overlap"}
CATEGORIES = {1: "Software layout", 2: "MMIO read", 3: "MMIO write", 4: "Mode verification",
              5: "Control poll", 6: "Alignment", 7: "Width", 8: "Readback mismatch", 9: "Conflicting bank"}


def describe_network(words):
    def ipv4(value):
        return ".".join(str(value >> shift & 255) for shift in (24, 16, 8, 0)) if value else None

    def ring_entries(port):
        value = words[6] >> (port * 8) & 255
        return {0: 64, 1: 256, 2: 512}.get(value, value)

    def device(port):
        start = 32 + port * 32
        frame = words[start:start + 32]
        link = bool(frame[6] & 2) if frame[12] in (0, 1) and frame[2] & (1 << 6) else False
        if frame[12] == 0 and frame[1] not in (0, 167):
            link = False
        speed = next((rate for bit, rate in ((0x600, 2500), (0x10, 1000), (8, 100), (4, 10))
                      if link and frame[6] & bit), None)
        counters = ("link", "rx_packets", "rx_bytes", "rx_completed", "rx_errors", "rx_unavailable_events",
                    "tx_submitted", "tx_completed", "tx_timeouts", "rx_queue_drops", "mac_errors", "rx_recycled")
        milestones = [n for n in range(1, 32) if frame[2] & (1 << (n-1))]
        last = next((MILESTONES.get(n) for n in (23, 22, 21, 10, 9, 8, 7, 6, 5, 4, 3, 16, 2, 1)
                     if n in milestones), None)
        first = frame[25] if frame[25] not in (0, 0xffffffff) else frame[3] or frame[1]
        return dict(host_result=frame[0], result=frame[1], result_name=RESULTS.get(frame[1]),
                    first_error=first if first not in (0, 167) else None,
                    phase=frame[7], last_completed_stage=last, milestones=milestones,
                    link_up=link, speed_mbps=speed, phy_status=frame[6],
                    inbound_accepted=frame[24] == 1, ring_entries=ring_entries(port) if 8 in milestones else None,
                    tx_pending_slots=((frame[18] - frame[19]) & 0xffffffff) if 21 not in milestones else None,
                    tail_close=frame[27:29] if 21 in milestones else None,
                    tally_codes=frame[29:31] if 21 in milestones else None,
                    **dict(zip(counters, frame[12:24])))

    progress = words[152]
    wan = dict(app_snapshot_available=bool(words[128]), gmac_snapshot_available=bool(words[160]),
               link_state=words[162], speed_mbps=words[166], mac_version=words[168] or None,
               gmac_clock_hz=words[170] or None, dhcp_bits=progress, dhcp_history_bits=words[154],
               dhcp_stages=[name for bit, name in enumerate(("discover accepted into TX queue",
                   "matching offer accepted", "request accepted into TX queue", "lease installed",
                   "gateway ARP seen", "gateway echo reply", "acquisition timeout", "gateway echo timeout",
                   "matching ACK accepted"))
                   if (progress | (words[154] & (1 << 8))) & (1 << bit)], lease=ipv4(words[138]) if progress & 8 else None,
               lease_remaining_ms=words[145] if progress & 8 else None, fallback=ipv4(words[153]),
               netmask=ipv4(words[139]) if progress & 8 else None,
               gateway=ipv4(words[140]) if progress & 8 else None, gateway_echo_replies=words[147],
               candidate=ipv4(words[138]) if not progress & 8 else None,
               config_rejected=words[150], rx_packets=words[175], tx_packets=words[176],
               wan_app_fault=words[135], gmac_fault=words[163], dma_drops=words[177], tx_queue_drops=words[148], rx_rejected=words[149])
    if words[4] in (3, 4) and (words[5] & 0x7fffffff) in (8, 16, 32, 64, 128, 256):
        hw2 = words[2]
        wan["dma_capabilities"] = {
            "feature_registers": [f"0x{word:08x}" for word in words[:words[4]]],
            "tx_channels": ((hw2 >> 18) & 15) + 1,
            "rx_channels": ((hw2 >> 12) & 15) + 1,
            "tx_queues": ((hw2 >> 6) & 15) + 1,
            "rx_queues": (hw2 & 15) + 1,
            "address_width_bits": (32, 40, 48, None)[(words[1] >> 14) & 3],
            "tx_checksum": bool(words[0] & (1 << 14)),
            "rx_checksum": bool(words[0] & (1 << 16)),
            "tso": bool(words[1] & (1 << 18)),
            "pbl": words[5] & 0x7fffffff,
            "eame": bool(words[5] & (1 << 31)),
        }
        if words[6] & (1 << 31):
            wan["dma_capabilities"]["axi"] = dict(
                read_outstanding_field=(words[6] >> 16) & 15,
                write_outstanding_field=(words[6] >> 20) & 15,
                mixed_burst=bool(words[6] & (1 << 24)),
            )
    if words[6] & (1 << 27):
        wan["rx_interrupt"] = dict(active=bool(words[6] & (1 << 25)), observed=bool(words[6] & (1 << 26)))
    if words[6] & (1 << 28):
        wan["tx_channels"] = 2 if words[6] & (1 << 29) else 1
    if words[6] & (1 << 30):
        wan["rx_channels"] = 2
    if words[163] == 120:
        wan["gmac_fault_name"] = "Supervisor requested WAN shutdown"
    if words[208] == 1:
        result = words[210]
        if result & 0x80000000:
            result -= 1 << 32
        wan["shutdown"] = dict(supervisor_request=words[209], dma_stop_result=result,
                               dma_was_running=bool(words[211]), started_ms=words[212])
    if words[155] == 1:
        scheduled, queued, replies, complete = words[156:160]
        if not progress & 8:
            state = "waiting_for_lease"
        elif not (scheduled or progress & 16):
            state = "waiting_for_gateway_arp"
        else:
            state = "complete" if complete else "running"
        wan["internet_echo"] = dict(target="8.8.8.8", state=state,
                                    socket_queued=scheduled, tx_queued=queued, replies=replies)
    logical_labels = ("lan1_forwarded", "lan1_local_tx", "lan1_egress_drops", "lan1_queue_errors",
                      "lan2_forwarded", "lan2_local_tx", "lan2_egress_drops", "lan2_queue_errors",
                      "arp_tx", "icmp_tx", "offers_generated", "acks_generated", "naks_generated",
                      "other_local_output", "arp_rx", "icmp_echo_rx", "dhcp_rx", "arp_cache_entries", "wan_queued", "wan_to_lan", "wan_queue_drops")
    report = dict(version=words[25], wan=wan,
                **{f"lan{i+1}": device(i) if words[26] & (1 << i) else None for i in range(2)},
                logical_lan=dict(zip(logical_labels, words[96:117])),
                words=[f"{v:08x}" for v in words])
    if words[42] or words[74] or words[75]:
        report["tx_ipv4_checksum"] = {
            name: dict(requests=words[counter], active=bool(words[active]))
            for name,counter,active in (("wan",43,75),("lan1",41,42),("lan2",73,74))
        }
    if words[25] >= 4:
        fields = ("tx_ring_inflight", "tx_ring_peak_inflight", "tx_ring_full", "tx_errors",
                  "tx_reclaim_batches", "tx_reclaim_max_batch")
        for port in range(2):
            device_report = report[f"lan{port + 1}"]
            if device_report is not None:
                start = 8 + 6 * port
                device_report.update(zip(fields, words[start:start + 6]))
                if 21 not in device_report["milestones"]:
                    device_report["tx_pending_slots"] = device_report["tx_ring_inflight"]
                device_report["tx_queue_drops"] = words[98 + 4 * port]
    for port in range(2):
        device_report = report[f"lan{port + 1}"]
        if device_report is None:
            continue
        for index, kind in enumerate(("endpoint", "root_port")):
            packed = words[27 + port * 2 + index]
            if packed & (1 << 31):
                device_report.setdefault("pcie", {})[kind] = dict(
                    generation=packed & 15,
                    lanes=packed >> 4 & 63,
                    max_payload_supported_bytes=128 << (packed >> 16 & 7),
                    max_payload_bytes=128 << (packed >> 19 & 7),
                    max_read_request_bytes=128 << (packed >> 22 & 7),
                )
    if words[25] in (5, 6, 7, 8, 9, 10, 11):
        values = words[20:25] + words[117:128]
        workers = []
        for cpu in range(4):
            packets, drops, occupancy, batches = values[4*cpu:4*cpu+4]
            workers.append(dict(cpu_id=cpu, packets_received=packets, packets_dropped=drops,
                                active_nat_flows=occupancy & 1023, queue_depth=(occupancy >> 10) & 15,
                                queue_peak=occupancy >> 14, processing_batches=batches))
        report["multicore"] = dict(enabled=True, worker_count=4, dispatch_failures=words[247],
                                   dispatch_total=words[31], workers=workers)
    if words[180] == 1:
        reason = words[181]
        reasons = ("None", "Counter frequency", "Supervisor poll deadline", "Health PD deadline",
                   "Driver state or ABI", "PHY sample freshness", "Driver execution freshness",
                   "Application status or freshness", "Invalid timestamp", "Startup deadline")
        supervisor = dict(shutdown_reason=reason, shutdown_reason_name=reasons[reason] if reason < len(reasons) else "Unknown",
                          poll_gap_count=words[182], first_poll_gap=None)
        if words[183]:
            supervisor["first_poll_gap"] = dict(elapsed_ms=words[183] / 24_000,
                driver_phase=words[184], lan1_phase=words[185] & 255, lan2_phase=words[185] >> 8 & 255,
                detail_code=words[186], timestamps=[lo | hi << 32 for lo, hi in zip(words[187:205:2], words[188:205:2])],
                health_counters=words[205:208])
        report["supervisor"] = supervisor
    if words[213]:
        outcomes = {
            1: "accepted", 2: "no output", 3: "waiting for neighbor ARP",
            4: "filtered by LAN bridge", 5: "no LAN egress", 6: "queue rejected",
            7: "no active WAN relay", 8: "queued to NAT worker",
        }
        outcomes.update(enumerate(("malformed", "checksum", "unsupported", "spoof",
                                   "policy", "no WAN lease", "capacity", "TTL",
                                   "MTU", "state", "overlap", "NAT client limit",
                                   "NAT slots exhausted", "NAT IDs exhausted", "neighbor ARP busy"), 16))
        trace = {}
        for i, name in enumerate(("lan_rx", "wan_relay_tx", "wan_relay_rx", "lan_tx")):
            start = 214 + i * 7
            count, src, dst, echo, result, mac_hi, mac_lo = words[start:start + 7]
            trace[name] = dict(count=count, last=None)
            if count:
                trace[name]["last"] = dict(source=ipv4(src), destination=ipv4(dst),
                    id=echo >> 16, sequence=echo & 0xffff, result=outcomes.get(result & 255, "unknown"),
                    lan_ports=[port + 1 for port in range(2) if result & (1 << (port + 8))],
                    destination_mac=(mac_hi.to_bytes(4, "big") + (mac_lo & 0xffff).to_bytes(2, "big")).hex(":"))
        count, echo, queued = words[243:246] if words[242] else (0, 0, 0)
        trace["wan_gmac_queue"] = dict(count=count, last=dict(
            id=echo >> 16, sequence=echo & 0xffff,
            result="accepted" if queued else "queue rejected") if count else None)
        report["icmp_trace"] = trace
    if words[246]:
        capacity, active, retired = words[246] >> 20, words[246] & 1023, (words[246] >> 10) & 1023
        report["nat_table"] = dict(capacity=capacity, active=active, retired=retired,
                                   available=capacity - active - retired)
    if len(words) >= 268 and words[25] in (6, 7, 8, 9, 10, 11):
        (rx, tx, completed, forwarded, dropped, copied, copied_bytes, queue_drops,
         mapping, ownership, recycled, alloc_failed, fb_pool, fb_mapping, fb_layout,
         fb_queue, peak, rx_dropped) = words[248:266]
        report["zc"] = dict(
            enabled=True,
            packets_received=rx, packets_sent=tx, buffers_completed=completed,
            packets_forwarded=forwarded, packets_dropped=dropped,
            copied_packets=copied, copied_bytes=copied_bytes,
            queue_drops=queue_drops, mapping_failures=mapping,
            ownership_errors=ownership, rx_recycled=recycled,
            allocation_failures=alloc_failed, rx_dropped=rx_dropped,
            fallback=dict(pool_exhausted=fb_pool, mapping=fb_mapping,
                          layout=fb_layout, queue_full=fb_queue),
            pool=dict(in_use=words[266], free=words[267], peak=peak, buffers=words[266]+words[267]))
    if words[25] in (6, 7, 8, 9, 10, 11) and any(words[i] for i in (63, 95, 131, 132, 134)):
        groups = words[95]
        report["gro"] = dict(eligible_packets=words[63], aggregated_packets=2 * groups,
                             aggregate_count=groups, average_segments=2 if groups else 0,
                             flush_timeout=words[132] >> 16, flush_incompatible=words[131],
                             fallbacks=words[132] & 65535, flush_batch=words[134] & 65535,
                             gso_packets=groups, gso_software_segments=words[134] >> 16)
    if len(words) >= 336 and (words[25] == 8 or (words[25] in (9, 10, 11)
            and (any(words[304:333]) or words[334] & (1 << 8)))):
        cpu = []
        for index, worker in enumerate(report["multicore"]["workers"]):
            sample = words[304+6*index:310+6*index]
            busy, idle, exhaustion = (lo | hi << 32 for lo, hi in zip(sample[::2], sample[1::2]))
            cpu.append(dict(core_id=index, busy_time=busy if busy or idle else None,
                            idle_time=idle if busy or idle else None,
                            budget_exhaustions=exhaustion if busy or idle else None,
                            worker_packets=worker["packets_received"], worker_drops=worker["packets_dropped"],
                            queue_depth=worker["queue_depth"], queue_peak=worker["queue_peak"]))
        report["cpu"] = dict(time_unit="pmu_cycles", cores=cpu)
        report["platform_clocks"] = dict(armclk_hz=[value or None for value in words[328:332]],
                                          ddr_phy_1x_hz=words[332] or None,
                                          source="RK3568 CRU register snapshot")
        if words[334] & (1 << 8):
            mux = (words[334] >> 9) & 7
            report["platform_clocks"].update(
                ddr_phy_source={0: "dpll", 1: "gpll", 2: "cpll", 4: "dedicated_dpll_ddr"}.get(mux),
                ddr_phy_clock_enabled=bool(words[334] & (1 << 12)))
        if words[334] & 1:
            report.setdefault("gro", {})["worker_pairs_published"] = words[333]
        if words[334] & 2:
            report.setdefault("gro", {})["driver_pairs_published"] = words[335]
    if len(words) >= 304 and (words[25] == 7 or (words[25] in (8, 9, 10, 11) and any(words[268:304]))):
        def tso(block):
            (requests, hw_frames, hw_segments, fb_frames, fb_segments,
             invalid, desc_errors, completion_failures, queue_drops,
             active, capability, flags) = block
            return dict(requests=requests, hw_frames=hw_frames, hw_segments=hw_segments,
                        fallback_frames=fb_frames, fallback_segments=fb_segments,
                        invalid=invalid, desc_errors=desc_errors,
                        completion_failures=completion_failures, queue_drops=queue_drops,
                        active=bool(active), capability=bool(capability),
                        ipv6=bool(flags & 1),
                        **({"hw_completed": flags >> 2} if flags & 2 else {}))
        report["tso"] = dict(
            wan=tso(words[268:280]), lan1=tso(words[280:292]), lan2=tso(words[292:304]))
    if len(words) >= 346 and words[25] in (9, 10, 11):
        for port in range(2):
            lan = report[f"lan{port + 1}"]
            index = 336 + port * 5
            queues = words[index]
            if lan is not None and queues in (1, 2, 4):
                lan["rss"] = dict(active=queues > 1, rx_queues=queues,
                                  packets_per_queue=list(words[index + 1:index + 1 + queues]))
    if len(words) >= 348 and words[25] in (10, 11):
        for port, packed in enumerate(words[346:348], 1):
            lan = report[f"lan{port}"]
            if lan is not None and packed & (1 << 31):
                lan["direct_worker_tx_pending"] = [(packed >> (i * 7)) & 127 for i in range(4)]
    if len(words) == 358 and words[25] == 11:
        for port, index in enumerate((348, 351), 1):
            lan = report[f"lan{port}"]
            count = words[index]
            if lan is not None and count in (1, 2):
                lan["tx_queue_retired_descriptors"] = list(words[index + 1:index + 1 + count])
        wan["tx_channel_retired_descriptors"] = list(words[354:354 + wan.get("tx_channels", 1)])
        wan["rx_channel_retired_descriptors"] = list(words[356:356 + wan.get("rx_channels", 1)])
    return report


def describe(words):
    if len(words) == 352 and words[25] == 14:
        legacy = list(words[:304])
        legacy[25] = 7
        report = describe_network(legacy)
        report["version"] = 14
        report["words"] = [f"{v:08x}" for v in words]
        fields = ("upload_received", "download_received", "upload_dropped", "download_dropped",
                  "invalid_packet", "nat_rejected", "stale_revision", "route_unavailable",
                  "hold_full", "hold_buffer_unavailable")
        report["worker_datapath"] = [dict(worker=i, **dict(zip(fields, words[304+i*10:314+i*10])))
                                     for i in range(4)]
        if words[344] == 0x5a435231:
            report.setdefault("zc", {}).update(cpu_released=words[345],
                tx_cancelled_on_stop=words[346], cpu_rejected=words[347])
        return report
    if len(words) == 364 and words[25] == 13:
        legacy = list(words[:360])
        legacy[25] = 12
        report = describe(legacy)
        report["version"] = 13
        report["words"] = [f"{v:08x}" for v in words]
        for port, count in zip(("wan", "lan1", "lan2"), words[360:363]):
            if report[port] is not None:
                report[port]["rx_shared_reserve_allocations"] = count
        report.setdefault("gro", {})["grouped_segments"] = words[363]
        if report.get("wan") is not None and words[358] & (1 << 31):
            report["wan"].update(
                tx_route_blocked=words[325],
                tx_stale_revision=words[326],
                tx_tso_deferred=words[327],
                tx_descriptor_peak_by_channel=[word & 0x7fffffff
                    for word in words[358:358 + report["wan"].get("tx_channels", 1)]],
            )
        return report
    if len(words) == 368 and words[25] == 15:
        legacy = list(words[:364])
        legacy[25] = 13
        report = describe(legacy)
        report["version"] = 15
        report["words"] = [f"{v:08x}" for v in words]
        # Schema 15: payload cache-maintenance calls (NC payload policy never
        # issues them) and the requested GMAC PBL beside the effective value.
        report.setdefault("zc", {})["cache_maintenance"] = words[364]
        if report.get("wan") is not None:
            report["wan"].setdefault("dma_capabilities", {})["pbl_requested"] = words[365]
        return report
    if len(words) == 360 and words[25] == 12:
        legacy = list(words[:358])
        legacy[25] = 11
        legacy[304:328] = [0] * 24
        report = describe_network(legacy)
        report["words"] = [f"{v:08x}" for v in words]
        report["version"] = 12
        if words[322] == 0x4c524e31:
            report["wan"]["learning_submitted"] = words[323]
            report["wan"]["learning_upload_drops"] = words[324]
        for port in ("lan1", "lan2"):
            lan = report[port]
            if lan is not None:
                lan["mac_error_bits"] = f"0x{lan['mac_errors']:08x}"
                lan["rx_descriptor_unavailable_seen"] = bool(lan["mac_errors"] & 0x10)
        if words[320] == 0x43534d31:
            report["wan"]["nat_copy_checksum_bypass"] = words[321]
        if words[313] == 0x57545831:
            fields = ("notifications", "dequeued", "queue_full", "budget_hits", "ring_stalls", "wake_passes")
            report["wan"]["worker_tx"] = dict(zip(fields, words[314:320]))
        report.pop("cpu", None)
        for i, port in enumerate(("wan", "lan1", "lan2")):
            if report[port] is not None:
                for j, field in enumerate(("rx_allocation_failures", "rx_copied_packets", "rx_copied_bytes")):
                    report[port][field] = words[304 + 3 * j + i]
        return report
    if len(words) in (248, 268, 304, 336, 346, 348, 358) and words[25] in (3, 4, 5, 6, 7, 8, 9, 10, 11):
        return describe_network(words)
    if len(words) != 248 or words[25] not in (1, 2):
        raise ValueError("Incomplete or unsupported B51 report")

    def available(value):
        return None if value == 0xffffffff else value

    def tally(index, outcome, chip):
        return dict(chip_cmd=available(words[chip]) if words[index] else None, sys=words[outcome],
                    outcome=MILESTONES.get(words[outcome], "No outcome published"), stage=words[index],
                    changed_words=available(words[index + 6]), first_bad_guard_plus_one=available(words[index + 5]))

    milestones = [i + 1 for i in range(31) if words[2] & (1 << i)]
    early = words[25] == 2 and words[247] in (1, 2)
    submitted = 21 in milestones
    first = available(words[135]) if words[25] == 2 else (words[3] or (words[1] if words[1] != 167 else 0))
    last = next((n for n in (23, 22, 21, 9, 8, 7, 6, 5, 4, 3, 16, 2, 1) if n in milestones), None)
    result = dict(version=words[25], terminal=words[1], terminal_name=RESULTS.get(words[1]),
                  milestones=milestones, milestone_names=[MILESTONES.get(n, str(n)) for n in milestones],
                  aperture_checked=words[27], aperture_state=("accepted" if words[27] == 1 else
                      {1: "mapping attempted", 2: "mapping programmed, audit rejected"}.get(words[247], "not accepted")),
                  phase_name=PHASES.get(words[7]), last_completed_stage=MILESTONES.get(last),
                  first_error_code=first or None, first_error_name=RESULTS.get(first),
                  first_failed_stage=(PHASES.get(words[143]) if words[25] == 2 and first else
                      PHASES[13] if words[1] in (161, 162) else None),
                  lan1_tx_attempted=submitted, dma_diagnostic_attempted=words[128] >= 3 or words[136] >= 3,
                  tail_close_initial=[available(v) for v in words[13:15]] if 9 in milestones else [None, None],
                  tail_close_after_kick=[available(v) for v in words[62:64]] if submitted else [None, None],
                  tail_close_final=[available(v) for v in words[15:17]] if submitted else [None, None],
                  rx_off=tally(128, 21, 19), rx_on=tally(136, 22, 20),
                  words=[f"{v:08x}" for v in words])
    if submitted:
        result["pci_error_status"] = {
            name: {label: bool(words[i] & (1 << bit)) for bit, label in
                   [(31, "parity"), (30, "system_error"), (29, "master_abort"),
                    (28, "received_target_abort"), (27, "signalled_target_abort"), (24, "master_data_parity")]}
            for name, i in (("root", 11), ("endpoint", 12)) if available(words[i]) is not None}
    if early:
        result["pre_mapping_pcie"] = {
            "root": dict(zip(("command_status", "device_control_status", "link_control_status",
                             "aer_uncorrectable", "aer_correctable"), map(available, words[48:53]))),
            "endpoint": dict(zip(("command_status", "bar2_low", "bar2_high", "bar4_low", "bar4_high",
                                 "pm_control_status", "device_control_status", "link_control_status",
                                 "aer_uncorrectable", "aer_correctable"), map(available, words[53:63]))),
        }
        result["inbound_error"] = dict(
            operation=OPERATIONS.get(words[32]), category=CATEGORIES.get(words[33]),
            register=f"0x{words[35]:x}{words[34]:08x}" if words[32] in OPERATIONS else None,
            expected=available(words[36]) if words[32] != 2 else words[36],
            observed=(available(words[37]) if words[33] in (2, 3) else words[37]),
            controller_dbi=f"0x{words[39]:x}{words[38]:08x}",
            bus_base=words[40], inclusive_limit=words[41], cpu_target=words[42],
            bank=available(words[44]) if words[33] == 9 else 0, direction="inbound",
            unrolled=(words[45] == 0xffffffff if words[32] > 2 or words[33] == 4 else None),
            limit_probe_zero=available(words[46]), limit_probe_ones=(
                words[47] if words[32] > 5 or (words[32] == 5 and words[33] == 7) else None),
            post_audit_disable_verified=bool(words[63]) if words[247] == 2 else None)
    return result


if __name__ == "__main__":
    import json
    import sys
    with open(sys.argv[1]) as source:
        report = json.load(source)
    print(json.dumps(describe([int(word, 16) for word in report["words"]]), indent=2))
