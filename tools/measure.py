#!/usr/bin/env python3
"""Sample a running app's process-tree RAM and CPU on Linux, without controlling it."""
import argparse
import json
import os
from pathlib import Path
import time


def processes() -> dict[int, tuple[int, int, int, int]]:
    """Read parent PID, start time, CPU ticks and RSS for each readable process."""
    result = {}
    for entry in Path('/proc').iterdir():
        if not entry.name.isdecimal():
            continue
        try:
            # comm may contain spaces and parentheses; numeric fields follow its final ')'.
            fields = (entry / 'stat').read_text().rsplit(')', 1)[1].split()
            result[int(entry.name)] = (
                int(fields[1]), int(fields[19]), int(fields[11]) + int(fields[12]),
                int(fields[21]) * os.sysconf('SC_PAGE_SIZE'),
            )
        except (FileNotFoundError, ProcessLookupError, PermissionError):
            continue
    return result


def tree(root: int, entries: dict[int, tuple[int, int, int, int]]) -> set[int]:
    """Include the root and its currently running descendants."""
    found = {root}
    while True:
        expanded = found | {pid for pid, info in entries.items() if info[0] in found}
        if expanded == found:
            return found & entries.keys()
        found = expanded


def pss_bytes(pids: set[int]) -> int | None:
    """Return summed proportional memory, or None when any process is unreadable."""
    total = 0
    for pid in pids:
        try:
            lines = Path(f'/proc/{pid}/smaps_rollup').read_text().splitlines()
            total += next(int(line.split()[1]) * 1024 for line in lines if line.startswith('Pss:'))
        except (OSError, StopIteration):
            return None
    return total


def main() -> None:
    """Measure observed CPU increments and peak sampled memory for a live process tree."""
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('pid', type=int)
    parser.add_argument('--seconds', type=int, default=60)
    args = parser.parse_args()
    if args.pid <= 0 or args.seconds < 1:
        parser.error('PID and seconds must be positive')
    initial = processes()
    if args.pid not in initial:
        parser.error('PID is not running or cannot be read')
    identity = initial[args.pid][1]
    previous = {(pid, initial[pid][1]): initial[pid][2] for pid in tree(args.pid, initial)}
    ticks = 0
    rss_peak = 0
    pss_peak = None
    started = time.monotonic()
    ended_early = False
    for _ in range(args.seconds):
        time.sleep(1)
        entries = processes()
        if args.pid not in entries or entries[args.pid][1] != identity:
            ended_early = True
            break
        pids = tree(args.pid, entries)
        rss_peak = max(rss_peak, sum(entries[pid][3] for pid in pids))
        pss = pss_bytes(pids)
        if pss is not None:
            pss_peak = max(pss_peak or 0, pss)
        for pid in pids:
            info = entries[pid]
            key = (pid, info[1])
            ticks += max(0, info[2] - previous.get(key, info[2]))
            previous[key] = info[2]
    elapsed = time.monotonic() - started
    print(json.dumps({
        'pid': args.pid,
        'elapsed_seconds': round(elapsed, 2),
        'cpu_percent_one_core': round(100 * ticks / os.sysconf('SC_CLK_TCK') / elapsed, 3),
        'peak_sampled_rss_mb': round(rss_peak / 1_000_000, 2),
        'peak_sampled_pss_mb': None if pss_peak is None else round(pss_peak / 1_000_000, 2),
        'ended_early': ended_early,
        'caveat': 'RSS double-counts shared pages. One-second sampling may miss short-lived children or peaks; no GPU memory measurement.',
    }, indent=2))


if __name__ == '__main__':
    main()
