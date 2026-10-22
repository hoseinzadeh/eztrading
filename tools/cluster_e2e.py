#!/usr/bin/env python3
"""End-to-end check of a running cluster through Strategy Studio's API (standard library only).

Needs: the cluster up (srvman + datasrv + codesrv + jobsrv), a Studio started with
`--srvman HOST:PORT`, and a data server that streams AAPL (see docs/cluster.md).

    python3 tools/cluster_e2e.py [--studio http://127.0.0.1:8787] [--logs ~/.eztrading]

It deploys examples/pricelog.sm as a bot, waits until the job server logs a real price from the data
server, redeploys it (which replaces the bot), stops it and checks that it is gone.
"""
import argparse, glob, json, os, sys, time, urllib.request

ap = argparse.ArgumentParser()
ap.add_argument("--studio", default="http://127.0.0.1:8787")
ap.add_argument("--logs", default=os.path.expanduser("~/.eztrading"))
ap.add_argument("--user", type=int, default=1)
args = ap.parse_args()
host = args.studio.split("//")[1]


def call(method, path, body=None):
    r = urllib.request.Request(args.studio + path, method=method, data=json.dumps(body).encode() if body is not None else None,
                               headers={"Host": host.split(":")[0], "Content-Type": "application/json"})
    return json.load(urllib.request.urlopen(r, timeout=120))


def deploy(params):
    src = open(os.path.join(os.path.dirname(__file__), "..", "examples", "pricelog.sm")).read()
    t = call("POST", "/api/cluster/deploy", {"user": args.user, "name": "pricelog-e2e", "source": src, "machine": "PriceLog", "params": params})["task"]
    for _ in range(300):
        d = call("GET", f"/api/cluster/tasks/{t}")
        if d["done"]:
            break
        time.sleep(2)
    for s in d["steps"]:
        print(f"  {s['status']:7} {s['label']}: {s['detail']}")
    if d["error"] or not d["bot"]:
        sys.exit(f"deploy failed: {d['error']}")
    return d["bot"]


def prices_logged():
    n = 0
    for f in glob.glob(os.path.join(args.logs, "jobsrv-*.out")):
        for line in open(f, errors="replace"):
            if line.startswith("price ") and float(line.split()[1]) > 0:
                n += 1
    return n


status = call("GET", "/api/cluster")
if not status.get("connected"):
    sys.exit(f"Studio cannot reach srvman: {status.get('error') or status}")
print("servers:", *status["servers"], sep="\n  ")
before = prices_logged()
print("1. deploy the bot")
bot = deploy({"equities": "AAPL"})
for _ in range(60):
    if prices_logged() > before:
        break
    time.sleep(1)
else:
    sys.exit("the bot is running but no price reached it within a minute")
print(f"   prices are reaching bot {bot}: {prices_logged() - before} so far")
print("2. redeploy (replaces the bot)")
bot2 = deploy({"equities": "AAPL,BTC/USD"})
bots = [b["id"] for b in call("GET", "/api/cluster")["bots"] if b["plugin_name"] == "pricelog-e2e"]
assert bots == [bot2], f"expected only bot {bot2}, found {bots}"
print("3. stop it")
call("POST", f"/api/cluster/bots/{bot2}/stop", {})
bots = [b["id"] for b in call("GET", "/api/cluster")["bots"] if b["plugin_name"] == "pricelog-e2e"]
assert not bots, f"still running: {bots}"
print("OK")
