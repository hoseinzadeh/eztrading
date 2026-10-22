#!/usr/bin/env python3
"""A local stand-in for the parts of the Twelve Data API that datasrv uses, so the cluster can run
without an account or network access. Prices are a deterministic random walk per symbol.

    pip install websockets
    python3 tools/fake_twelvedata.py --port 3030
    TWELVEDATA_API_URL=http://127.0.0.1:3030 TWELVEDATA_WS_URL=ws://127.0.0.1:3030 ./target/debug/datasrv ...

It imitates the shape of the public API; it is not the service.
"""
import argparse, asyncio, hashlib, json, random, time
from datetime import datetime, timedelta, timezone
from http import HTTPStatus
from urllib.parse import urlparse, parse_qs
from websockets.asyncio.server import serve
from websockets.http11 import Response
from websockets.datastructures import Headers

STEP = {"1min": 60, "5min": 300, "15min": 900, "30min": 1800, "45min": 2700, "1h": 3600, "2h": 7200,
        "4h": 14400, "1day": 86400, "1week": 604800, "1month": 2592000}


def base_price(sym):
    return 50 + int(hashlib.md5(sym.encode()).hexdigest()[:4], 16) % 200


def walk(sym, n, step, end):
    rnd = random.Random(sym)
    p = base_price(sym)
    pts = []
    t = end - n * step
    for _ in range(n):
        o = p
        p = max(1.0, p * (1 + rnd.gauss(0, 0.004)))
        pts.append((t, o, max(o, p) * 1.001, min(o, p) * 0.999, p, rnd.randint(1000, 90000)))
        t += step
    return pts


def series(sym, q):
    interval = q.get("interval", ["1day"])[0]
    step = STEP.get(interval, 86400)
    end = int(time.time())
    if "end_date" in q:
        try:
            end = int(datetime.strptime(q["end_date"][0], "%Y-%m-%d %H:%M:%S").replace(tzinfo=timezone.utc).timestamp())
        except ValueError:
            pass
    n = 200
    if "start_date" in q:
        try:
            s = int(datetime.strptime(q["start_date"][0], "%Y-%m-%d %H:%M:%S").replace(tzinfo=timezone.utc).timestamp())
            n = max(1, min(5000, (end - s) // step))
        except ValueError:
            pass
    fmt = "%Y-%m-%d" if step >= 86400 else "%Y-%m-%d %H:%M:%S"
    vals = [{"datetime": datetime.fromtimestamp(t, timezone.utc).strftime(fmt), "open": f"{o:.4f}", "high": f"{h:.4f}",
             "low": f"{l:.4f}", "close": f"{c:.4f}", "volume": str(v)} for t, o, h, l, c, v in reversed(walk(sym, n, step, end))]
    return {"meta": {"symbol": sym, "interval": interval, "currency": "USD", "exchange_timezone": "America/New_York",
                     "exchange": "NASDAQ", "mic_code": "XNGS", "type": "Common Stock"}, "values": vals, "status": "ok"}


def http_reply(connection, request):
    u = urlparse(request.path)
    if u.path == "/v1/quotes/price":
        return None  # websocket upgrade
    q = parse_qs(u.query)
    syms = [s for s in q.get("symbol", [""])[0].split(",") if s]
    if u.path == "/market_state":
        body = [{"name": n, "code": c, "country": "United States", "is_market_open": True,
                 "time_to_open": "00:00:00", "time_to_close": "05:00:00"} for n, c in (("NASDAQ", "XNAS"), ("NYSE", "XNYS"))]
    elif u.path == "/time_series":
        body = series(syms[0], q) if len(syms) == 1 else {s: series(s, q) for s in syms}
    elif u.path == "/earliest_timestamp":
        e = {"datetime": "2000-01-03", "unix_time": 946857600}
        body = e if len(syms) == 1 else {s: e for s in syms}
    else:
        body = {"code": 404, "message": "not found", "status": "error"}
    data = json.dumps(body).encode()
    return Response(HTTPStatus.OK.value, "OK", Headers([("Content-Type", "application/json"), ("Content-Length", str(len(data)))]), data)


async def ws_handler(ws):
    symbols = []
    prices = {}

    async def pump():
        while True:
            await asyncio.sleep(1)
            for s in symbols:
                prices[s] = max(1.0, prices.get(s, base_price(s)) * (1 + random.gauss(0, 0.002)))
                await ws.send(json.dumps({"event": "price", "symbol": s, "currency": "USD", "exchange": "NASDAQ",
                                          "type": "Common Stock", "timestamp": int(time.time()),
                                          "price": round(prices[s], 4), "day_volume": 12345}))
    task = asyncio.create_task(pump())
    try:
        async for msg in ws:
            m = json.loads(msg)
            if m.get("action") == "subscribe":
                symbols[:] = [s for s in m["params"]["symbols"].split(",") if s]
                await ws.send(json.dumps({"event": "subscribe-status", "status": "ok", "success": [{"symbol": s} for s in symbols], "fails": None}))
            elif m.get("action") == "heartbeat":
                await ws.send(json.dumps({"event": "heartbeat", "status": "ok"}))
    finally:
        task.cancel()


async def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--port", type=int, default=3030)
    a = ap.parse_args()
    async with serve(ws_handler, "127.0.0.1", a.port, process_request=http_reply):
        print(f"fake Twelve Data on 127.0.0.1:{a.port}", flush=True)
        await asyncio.Future()

asyncio.run(main())
