#!/usr/bin/env python3
# -*- coding: utf-8 -*-
"""
regenkarte.py v1.2 -- On-Demand-Regenkarte fuer N950 (wunderw / Py 3.11)

Ein einziger Prozess, reine stdlib:

  python3 regenkarte.py serve [port]     Server auf 127.0.0.1:8642
  python3 regenkarte.py seed [bbox...]   Basemap-Cache vorladen

Endpunkte:
  /index.json                 Frame-Liste + Geo-Bounds
  /osm/{z}/{x}/{y}.png        OSM-Basemap (Disk-Cache, TLS-Terminierung)
  /rv/{time}/{z}/{x}/{y}.png  RainViewer-Radar-Tiles (weltweit)
  /fcst/{time}.png            AROME-Regen-Overlay (Alpenraum, on demand)
  /points/{time}.json?bbox=lat0,lon0,lat1,lon1&z=N
                              Temperatur, Wind + Regen an Staedten im
                              Viewport (weltweit, Open-Meteo). Decluttering
                              nach Population: groessere Stadt gewinnt.

Datenquellen: RainViewer.com, GeoSphere Austria (CC BY 4.0),
Open-Meteo.com (CC BY 4.0), (c) OpenStreetMap contributors,
Staedteliste: GeoNames (CC BY 4.0)
"""

import bisect
import json
import math
import os
import re
import struct
import sys
import threading
import time
import urllib.parse
import urllib.request
import zlib
from datetime import datetime, timezone
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

# ---------------------------------------------------------------- Konfig ---

OUTDIR = os.path.expanduser("~/MyDocs/regenkarte")
CACHEDIR = os.path.join(OUTDIR, "cache")
PORT = 8642
BASEDIR = os.path.dirname(os.path.abspath(__file__))

VIENNA = (48.2082, 16.3738)

FCST_HOURS = 48
FCST_CHUNK_H = 3                       # Chunkgroesse = AROME-Zyklus
FCST_TTL = 3 * 3600
FCST_BBOX = (46.3, 12.8, 49.3, 19.0)   # lat_min, lon_min, lat_max, lon_max
RR_PARAM = "rr_acc"

RV_INDEX = "https://api.rainviewer.com/public/weather-maps.json"
RV_COLOR, RV_OPTS = "2", "1_1"
GS_BASE = ("https://dataset.api.hub.geosphere.at/v1/grid/forecast/"
           "nwp-v1-1h-2500m")
ST_BASE = ("https://dataset.api.hub.geosphere.at/v1/station/current/"
           "tawes-v1-10min")
TEMP_OBS_PARAM = "TL"                  # TAWES 2m-Lufttemperatur
TEMP_FCST_PARAM = "t2m"                # AROME 2m-Temperatur
WIND_OBS_PARAMS = ("FF", "DD")         # TAWES Windgeschwindigkeit [m/s], -richtung [Grad]
WIND_FCST_PARAMS = ("u10m", "v10m")    # AROME 10m-Windkomponenten [m/s]
OBS_TTL = 600
STATIONS_TTL = 86400
AT_BBOX = FCST_BBOX                    # "Oesterreich-Zone" = AROME-Bbox
VIENNA_BOX = (48.10, 16.18, 48.35, 16.60)   # nie ausduennen
OSM_TILE = "https://tile.openstreetmap.org/{z}/{x}/{y}.png"
OM_URL = "https://api.open-meteo.com/v1/forecast"
UA = {"User-Agent": "regenkarte-n950/1.2 (personal use)"}

CITIES_FILE = os.path.join(BASEDIR, "cities.json")
MIN_PX = 56                            # Mindestabstand der Punkte [px]
MAX_PTS = 50                           # Punkte pro Viewport-Antwort
OM_TTL = 3600                          # Open-Meteo stuendlich neu
OM_BATCH = 50

OVERLAY_ALPHA = 170
RATE_COLORS = (                        # mm/h -> RGB
    (0.1, (110, 170, 255)), (0.5, (60, 120, 245)), (1.0, (30, 80, 220)),
    (2.0, (0, 160, 60)), (4.0, (255, 210, 0)), (8.0, (255, 120, 0)),
    (16.0, (230, 0, 0)), (32.0, (170, 0, 170)),
)

H = 3600
CHUNK = FCST_CHUNK_H * H

# --------------------------------------------------------- Mini-PNG-Save ---


def png_bytes(w, h, rgba):
    def chunk(tag, data):
        return (struct.pack(">I", len(data)) + tag + data
                + struct.pack(">I", zlib.crc32(tag + data) & 0xFFFFFFFF))
    raw = bytearray()
    stride = 4 * w
    for y in range(h):
        raw.append(0)
        raw += rgba[y * stride:(y + 1) * stride]
    return (b"\x89PNG\r\n\x1a\n"
            + chunk(b"IHDR", struct.pack(">IIBBBBB", w, h, 8, 6, 0, 0, 0))
            + chunk(b"IDAT", zlib.compress(bytes(raw), 6))
            + chunk(b"IEND", b""))


EMPTY_PNG = png_bytes(1, 1, bytearray(4))

# ------------------------------------------------------------------ HTTP ---


def fetch(url, retries=3):
    last = None
    for _ in range(retries):
        try:
            req = urllib.request.Request(url, headers=UA)
            with urllib.request.urlopen(req, timeout=45) as r:
                return r.read()
        except Exception as e:            # noqa: BLE001
            last = e
            time.sleep(1.5)
    raise RuntimeError("fetch failed: %s (%s)" % (url, last))


# ------------------------------------------------ RainViewer-Frameindex ---

_rv_lock = threading.Lock()
_rv_paths = {}
_rv_stamp = 0.0


def rv_refresh(force=False):
    global _rv_stamp
    with _rv_lock:
        if not force and time.time() - _rv_stamp < 180:
            return
        idx = json.loads(fetch(RV_INDEX).decode("utf-8"))
        _rv_paths.clear()
        for p in idx.get("radar", {}).get("past", []):
            _rv_paths[p["time"]] = p["path"]
        _rv_stamp = time.time()


def rv_path_for(t):
    try:
        rv_refresh()
    except Exception as e:                # noqa: BLE001
        print("rv index: %s" % e, file=sys.stderr)
    with _rv_lock:
        return _rv_paths.get(t)


# --------------------------------------- Staedte + Open-Meteo-Punktdaten ---

_ct_lock = threading.Lock()
_cities = []                            # [[lat, lon, pop], ...] pop-DESC
_cities_bylat = []                      # lat-sortiert fuer Umkreissuche
_cities_lats = []                       # nur die Latitudes (fuer bisect)
_om = {}                                # (lat,lon) -> Cache-Eintrag


def load_cities():
    with _ct_lock:
        if not _cities:
            with open(CITIES_FILE) as f:
                _cities.extend(json.load(f))
            _cities_bylat.extend(sorted(_cities, key=lambda c: c[0]))
            _cities_lats.extend(c[0] for c in _cities_bylat)
            print("staedte: %d geladen" % len(_cities))
        return _cities


def city_pop_near(lat, lon, dlat=0.18, dlon=0.28):
    """Groesste Stadtbevoelkerung im Umkreis (fuer Stations-Prioritaet)."""
    load_cities()
    i0 = bisect.bisect_left(_cities_lats, lat - dlat)
    i1 = bisect.bisect_right(_cities_lats, lat + dlat)
    best = 1000
    for c in _cities_bylat[i0:i1]:
        if abs(c[1] - lon) <= dlon and c[2] > best:
            best = c[2]
    return best


def in_box(lat, lon, box):
    return box[0] <= lat <= box[2] and box[1] <= lon <= box[3]


# ------------------------------------------ TAWES-Stationen (Oesterreich) --

_st_lock = threading.Lock()
_stations = []              # [{id, name, lat, lon, pri, wien}]
_st_param = TEMP_OBS_PARAM
_st_stamp = 0.0
_obs = {"t": 0.0, "data": {}}


def _celsius(v):
    return round(v - 273.15 if v > 150 else v, 1)


def ensure_stations():
    global _st_stamp, _st_param
    with _st_lock:
        if _stations and time.time() - _st_stamp < STATIONS_TTL:
            return _stations
        meta = json.loads(fetch(ST_BASE + "/metadata").decode("utf-8"))
        names = [p.get("name", "") for p in meta.get("parameters", [])]
        if TEMP_OBS_PARAM not in names:
            for p in names:
                if p.lower() == TEMP_OBS_PARAM.lower():
                    _st_param = p
                    break
        st = []
        for s in meta.get("stations", []):
            sid = str(s.get("id") or s.get("station_id") or "")
            lat = s.get("lat", s.get("latitude"))
            lon = s.get("lon", s.get("longitude"))
            if s.get("is_active") is False:
                continue
            if sid and isinstance(lat, (int, float)) \
                    and isinstance(lon, (int, float)):
                st.append({"id": sid, "name": s.get("name", ""),
                           "lat": lat, "lon": lon,
                           "pri": city_pop_near(lat, lon),
                           "wien": in_box(lat, lon, VIENNA_BOX)})
        _stations[:] = st
        _st_stamp = time.time()
        print("stationen: %d (param %s)" % (len(st), _st_param))
        return _stations


def wind_from_uv(u, v):
    """AROME liefert Komponenten, angezeigt wird Betrag und Herkunft.

    Meteorologisch zaehlt die Richtung, aus der es weht -- deshalb die 270
    und das Vorzeichen: weht es nach Osten (u > 0), kommt es aus 270 Grad,
    also aus West.
    """
    if not isinstance(u, (int, float)) or not isinstance(v, (int, float)):
        return None
    ff = math.hypot(u, v)
    dd = (270.0 - math.degrees(math.atan2(v, u))) % 360.0
    return round(ff, 1), int(round(dd))


def obs_values():
    """Temperatur und Wind der TAWES-Stationen, in EINEM Abruf.

    Frueher wurde nur die Temperatur geholt. Zwei getrennte Abrufe waeren
    die naheliegende Erweiterung gewesen und die falsche: Die Schnittstelle
    nimmt mehrere Parameter auf einmal, und das Geraet haengt oft an einer
    langsamen Mobilfunkverbindung.

    Rueckgabe: {sid: {"t": Grad|None, "ff": m/s|None, "dd": Grad|None}}
    """
    with _st_lock:
        if _obs["data"] and time.time() - _obs["t"] < OBS_TTL:
            return _obs["data"]
    st = ensure_stations()
    ids = ",".join(s["id"] for s in st)
    params = "&".join("parameters=%s" % p
                      for p in (_st_param,) + WIND_OBS_PARAMS)
    url = "%s?%s&station_ids=%s&output_format=geojson" % (ST_BASE, params, ids)
    data = json.loads(fetch(url).decode("utf-8"))
    out = {}
    for f in data.get("features", []):
        p = f.get("properties", {})
        sid = str(p.get("station", ""))
        pars = p.get("parameters", {}) or {}

        def letzter(name):
            try:
                v = pars[name]["data"][-1]
            except (KeyError, IndexError, TypeError):
                return None
            return v if isinstance(v, (int, float)) else None

        t = letzter(_st_param)
        ff = letzter(WIND_OBS_PARAMS[0])
        dd = letzter(WIND_OBS_PARAMS[1])
        if t is None and ff is None:
            continue
        out[sid] = {"t": _celsius(t) if t is not None else None,
                    "ff": round(ff, 1) if ff is not None else None,
                    "dd": int(round(dd)) if dd is not None else None}
    with _st_lock:
        _obs.update(t=time.time(), data=out)
    print("messwerte: %d Stationen (Temperatur und Wind)" % len(out))
    return out


def om_fill(coords):
    """Fehlende/veraltete Orte in EINEM Batch-Request nachladen."""
    with _ct_lock:
        need = [k for k in coords
                if k not in _om or time.time() - _om[k]["ts"] > OM_TTL]
    for i in range(0, len(need), OM_BATCH):
        batch = need[i:i + OM_BATCH]
        # wind_speed_unit=ms ist Pflicht: Open-Meteo liefert sonst km/h,
        # und die Anzeige soll m/s zeigen wie die TAWES-Messwerte.
        url = ("%s?latitude=%s&longitude=%s"
               "&hourly=temperature_2m,precipitation,"
               "wind_speed_10m,wind_direction_10m"
               "&current=temperature_2m,precipitation,"
               "wind_speed_10m,wind_direction_10m"
               "&wind_speed_unit=ms"
               "&forecast_days=2&timeformat=unixtime&timezone=UTC") % (
            OM_URL,
            ",".join("%.3f" % k[0] for k in batch),
            ",".join("%.3f" % k[1] for k in batch))
        data = json.loads(fetch(url).decode("utf-8"))
        if isinstance(data, dict):
            data = [data]
        with _ct_lock:
            for k, d in zip(batch, data):
                hr = d.get("hourly", {})
                times = hr.get("time", [])
                tt = hr.get("temperature_2m", []) or []
                pr = hr.get("precipitation", []) or []
                wf = hr.get("wind_speed_10m", []) or []
                wd = hr.get("wind_direction_10m", []) or []
                hh = {}
                for j, ts in enumerate(times):
                    hh[int(ts)] = (tt[j] if j < len(tt) else None,
                                   pr[j] if j < len(pr) else None,
                                   wf[j] if j < len(wf) else None,
                                   wd[j] if j < len(wd) else None)
                cur = d.get("current", {})
                _om[k] = {"ts": time.time(),
                          "cur": (cur.get("temperature_2m"),
                                  cur.get("precipitation"),
                                  cur.get("wind_speed_10m"),
                                  cur.get("wind_direction_10m")),
                          "h": hh}
        print("open-meteo: %d Orte geladen" % len(batch))


def rate_hex(mmh):
    col = None
    for thr, c in RATE_COLORS:
        if mmh >= thr:
            col = c
    return "#%02x%02x%02x" % col if col else ""


def boxes_overlap(b, box):
    return (b[0] <= box[2] and b[2] >= box[0]
            and b[1] <= box[3] and b[3] >= box[1])


def points_for(t, bbox, z):
    """Punkte im Viewport: Oesterreich = TAWES/AROME, sonst Open-Meteo.
    Erst Greedy-Auswahl (Wien ausgenommen, sonst Population gewinnt),
    Werte werden NUR fuer die Gewinner geholt -- spart Bandbreite.

    Je Punkt: v = Temperatur [Grad], w = Windgeschwindigkeit [m/s],
    d = Windrichtung [Grad, woher]. Fehlende Groessen fehlen einfach --
    die Oberflaeche blendet ein, was da ist."""
    lat0, lon0, lat1, lon1 = bbox
    now = int(time.time())
    past = t <= now + 60
    th = t // H * H
    ws = 256.0 * 2 ** z

    def px(lat, lon):
        return ((lon + 180.0) / 360.0 * ws,
                (1.0 - math.asinh(math.tan(math.radians(lat)))
                 / math.pi) / 2.0 * ws)

    # --- Kandidaten (Werte erst spaeter) ------------------------------
    cand = []                 # (pri, wien, lat, lon, sid|None)
    st_vals = {}
    if boxes_overlap(bbox, AT_BBOX):      # nur wenn Oesterreich im Bild
        try:
            if past:
                st_vals = obs_values()
            else:
                cs = chunk_start(th)
                if not temps_fresh(th) and temps_retry_ok(cs):
                    render_chunk(cs)      # leere Daten -> sofort neu
                    temps_backoff(cs, temps_fresh(th))
                if os.path.exists(temps_path(th)):
                    with open(temps_path(th)) as f:
                        roh = json.load(f)
                    # Auf dieselbe Form bringen wie die Messwerte, damit
                    # weiter unten nicht zwei Faelle zu unterscheiden sind.
                    wnd = roh.get("wind", {}) or {}
                    st_vals = {}
                    for sid, tv in (roh.get("temps", {}) or {}).items():
                        w = wnd.get(sid) or (None, None)
                        st_vals[sid] = {"t": tv, "ff": w[0], "dd": w[1]}
                    for sid, w in wnd.items():
                        if sid not in st_vals:
                            st_vals[sid] = {"t": None,
                                            "ff": w[0], "dd": w[1]}
        except Exception as e:            # noqa: BLE001
            print("stationswerte: %s" % e, file=sys.stderr)
            if not past:
                temps_backoff(chunk_start(th), False)
        try:
            for s in ensure_stations():
                if not (lat0 <= s["lat"] <= lat1
                        and lon0 <= s["lon"] <= lon1):
                    continue
                e = st_vals.get(s["id"])
                if not isinstance(e, dict):
                    continue
                if not isinstance(e.get("t"), (int, float)) \
                        and not isinstance(e.get("ff"), (int, float)):
                    continue
                cand.append((s["pri"], s["wien"],
                             s["lat"], s["lon"], s["id"]))
        except Exception as e:            # noqa: BLE001
            print("stationen: %s" % e, file=sys.stderr)

    for lat, lon, pop in load_cities():   # pop-absteigend sortiert
        if not (lat0 <= lat <= lat1 and lon0 <= lon <= lon1):
            continue
        if in_box(lat, lon, AT_BBOX):     # dort gilt TAWES/AROME
            continue
        cand.append((pop, False, lat, lon, None))
        if len(cand) >= 6 * MAX_PTS:
            break

    # --- Wien-Ausnahme + Greedy nach Prioritaet -----------------------
    chosen, taken = [], []
    cand.sort(key=lambda c: (not c[1], -c[0]))    # Wien zuerst, dann pop
    for pri, wien, lat, lon, sid in cand:
        x, y = px(lat, lon)
        if not wien:
            clash = False
            for tx, ty in taken:
                if abs(tx - x) < MIN_PX and abs(ty - y) < MIN_PX:
                    clash = True
                    break
            if clash:
                continue
        taken.append((x, y))
        chosen.append((lat, lon, sid))
        if len(chosen) >= MAX_PTS:
            break

    # --- Werte nur fuer die Gewinner ----------------------------------
    try:
        om_fill([(la, lo) for la, lo, sid in chosen if sid is None])
    except Exception as e:                # noqa: BLE001
        print("open-meteo: %s" % e, file=sys.stderr)
    out = []
    with _ct_lock:
        for lat, lon, sid in chosen:
            if sid is not None:           # TAWES/AROME-Wert liegt vor
                e = st_vals[sid]
                p = {"lat": lat, "lon": lon}
                if isinstance(e.get("t"), (int, float)):
                    p["v"] = round(e["t"], 1)
                if isinstance(e.get("ff"), (int, float)):
                    p["w"] = round(e["ff"], 1)
                    if isinstance(e.get("dd"), (int, float)):
                        p["d"] = int(round(e["dd"]))
                out.append(p)
                continue
            e = _om.get((lat, lon))
            if e is None:
                continue
            werte = e["cur"] if past else e["h"].get(th)
            if not werte:
                continue
            v, r = werte[0], werte[1]
            ff = werte[2] if len(werte) > 2 else None
            dd = werte[3] if len(werte) > 3 else None
            if not isinstance(v, (int, float)) \
                    and not isinstance(ff, (int, float)):
                continue
            p = {"lat": lat, "lon": lon}
            if isinstance(v, (int, float)):
                p["v"] = round(v, 1)
            if isinstance(ff, (int, float)):
                p["w"] = round(ff, 1)
                if isinstance(dd, (int, float)):
                    p["d"] = int(round(dd))
            if not past and isinstance(r, (int, float)) and r >= 0.1:
                c = rate_hex(r)
                if c:
                    p["c"] = c            # Prognose-Regen ohne Overlay
            out.append(p)
    return out


# ------------------------------------------------- AROME On-Demand-Chunks --

_gs_lock = threading.Lock()
_grid = {}
_prefetching = set()


def grid_file():
    return os.path.join(CACHEDIR, "grid.json")


def load_grid():
    """Persistierte Gitterdaten laden (ueberlebt Server-Neustarts)."""
    if not _grid and os.path.exists(grid_file()):
        try:
            with open(grid_file()) as f:
                g = json.load(f)
            if g.get("fbbox") == list(FCST_BBOX):   # Konfig unveraendert?
                _grid.update(g)
        except Exception:                 # noqa: BLE001
            pass
    return _grid


def save_grid():
    try:
        with open(grid_file(), "w") as f:
            json.dump(dict(_grid, fbbox=list(FCST_BBOX)), f)
    except Exception as e:                # noqa: BLE001
        print("grid save: %s" % e, file=sys.stderr)


def rate_color(mmh):
    if mmh < RATE_COLORS[0][0]:
        return None
    col = RATE_COLORS[0][1]
    for thr, c in RATE_COLORS:
        if mmh >= thr:
            col = c
    return col


def nearest_idx(axis, v):
    i = bisect.bisect_left(axis, v)
    if i == 0:
        return 0
    if i >= len(axis):
        return len(axis) - 1
    return i if axis[i] - v < v - axis[i - 1] else i - 1


def gs_url(s, e, with_temp=True, with_wind=True):
    fmt = "%Y-%m-%dT%H:%M"
    su = datetime.fromtimestamp(s, timezone.utc).strftime(fmt)
    eu = datetime.fromtimestamp(e, timezone.utc).strftime(fmt)
    params = "parameters=%s" % RR_PARAM
    if with_temp:
        params += "&parameters=%s" % TEMP_FCST_PARAM
    if with_wind:
        for name in WIND_FCST_PARAMS:
            params += "&parameters=%s" % name
    return ("%s?%s&bbox=%.2f,%.2f,%.2f,%.2f"
            "&output_format=geojson&start=%s&end=%s") % (
        GS_BASE, params, *FCST_BBOX, su, eu)


def stamp_to_epoch(ts):
    try:
        dt = datetime.fromisoformat(ts)
        if dt.tzinfo is None:
            dt = dt.replace(tzinfo=timezone.utc)
        return int(dt.timestamp())
    except ValueError:
        return int(datetime.strptime(ts[:16], "%Y-%m-%dT%H:%M")
                   .replace(tzinfo=timezone.utc).timestamp())


def fcst_path(t):
    return os.path.join(CACHEDIR, "fcst_%d.png" % t)


def temps_path(t):
    return os.path.join(CACHEDIR, "temps_%d.json" % t)


def _fresh(p):
    return os.path.exists(p) and time.time() - os.path.getmtime(p) < FCST_TTL


def fcst_fresh(t):
    return _fresh(fcst_path(t))


def temps_fresh(t):
    return _fresh(temps_path(t))


# Backoff pro Chunk fuer fehlgeschlagenes Temps-Sampling:
# sofortiger Retry beim naechsten Zugriff, bei Dauerstoerung exponentiell
_temps_retry = {}                       # cs -> [fails, next_ts]


def temps_retry_ok(cs):
    e = _temps_retry.get(cs)
    return e is None or time.time() >= e[1]


def temps_backoff(cs, ok):
    if ok:
        _temps_retry.pop(cs, None)
        return
    e = _temps_retry.get(cs, [0, 0.0])
    e[0] += 1
    delay = min(1800, 30 * 2 ** (e[0] - 1)) if e[0] < 6 else FCST_TTL
    e[1] = time.time() + delay
    _temps_retry[cs] = e
    print("temps-backoff chunk %d: Versuch %d, naechster in %ds"
          % (cs, e[0], delay), file=sys.stderr)


def chunk_start(t):
    return t // CHUNK * CHUNK


def render_chunk(cs):
    with _gs_lock:
        load_grid()
        if _grid and all(fcst_fresh(t) and temps_fresh(t)
                         for t in range(cs, cs + CHUNK + H, H)):
            return
        # Rueckfallkette: erst alles, dann ohne Wind, zuletzt ohne
        # Temperatur. Faellt ein Parameter bei GeoSphere einmal aus, soll
        # nicht die ganze Regenvorhersage mit ausfallen.
        data = None
        for s0, wt, ww in ((cs - H, True, True), (cs, True, True),
                           (cs - H, True, False), (cs, True, False),
                           (cs - H, False, False), (cs, False, False)):
            try:
                data = json.loads(
                    fetch(gs_url(s0, cs + CHUNK, wt, ww)).decode("utf-8"))
                break
            except Exception:             # noqa: BLE001
                continue
        if data is None:
            raise RuntimeError("geosphere chunk %d nicht ladbar" % cs)
        stamps, feats = data["timestamps"], data["features"]
        if not _grid:
            lats = sorted({round(f["geometry"]["coordinates"][1], 4)
                           for f in feats})
            lons = sorted({round(f["geometry"]["coordinates"][0], 4)
                           for f in feats})
            dlat = (lats[-1] - lats[0]) / max(1, len(lats) - 1)
            dlon = (lons[-1] - lons[0]) / max(1, len(lons) - 1)
            _grid.update(lats=lats, lons=lons, bounds={
                "lon0": lons[0] - dlon / 2, "lon1": lons[-1] + dlon / 2,
                "lat_top": lats[-1] + dlat / 2,
                "lat_bot": lats[0] - dlat / 2})
            save_grid()
            print("grid: %d x %d Punkte, %d Features"
                  % (len(lons), len(lats), len(feats)))
        lats, lons = _grid["lats"], _grid["lons"]
        nx, ny = len(lons), len(lats)

        try:
            st_idx = [(s["id"], nearest_idx(lats, s["lat"]),
                       nearest_idx(lons, s["lon"]))
                      for s in ensure_stations()
                      if lats[0] <= s["lat"] <= lats[-1]
                      and lons[0] <= s["lon"] <= lons[-1]]
        except Exception as e:            # noqa: BLE001
            print("stationen: %s" % e, file=sys.stderr)
            st_idx = []

        acc = [[[None] * nx for _ in range(ny)] for _ in stamps]
        tmp = [[[None] * nx for _ in range(ny)] for _ in stamps]
        uwd = [[[None] * nx for _ in range(ny)] for _ in stamps]
        vwd = [[[None] * nx for _ in range(ny)] for _ in stamps]
        for f in feats:
            lon, lat = f["geometry"]["coordinates"]
            r, c = nearest_idx(lats, lat), nearest_idx(lons, lon)
            pars = f["properties"]["parameters"]
            vals = pars[RR_PARAM]["data"]
            tv = pars.get(TEMP_FCST_PARAM, {}).get("data")
            uv = pars.get(WIND_FCST_PARAMS[0], {}).get("data")
            vv = pars.get(WIND_FCST_PARAMS[1], {}).get("data")
            for t, v in enumerate(vals):
                acc[t][r][c] = v
                if tv is not None:
                    tmp[t][r][c] = tv[t]
                if uv is not None and vv is not None:
                    uwd[t][r][c] = uv[t]
                    vwd[t][r][c] = vv[t]
        del feats, data

        for t, ts in enumerate(stamps):
            tsec = stamp_to_epoch(ts)
            if not temps_fresh(tsec):
                temps, wind = {}, {}
                for sid, r, c in st_idx:
                    v = tmp[t][r][c]
                    if isinstance(v, (int, float)):
                        temps[sid] = _celsius(v)
                    w = wind_from_uv(uwd[t][r][c], vwd[t][r][c])
                    if w:
                        wind[sid] = list(w)
                if temps:                 # leere NICHT cachen -> Retry
                    # "wind" darf fehlen: aeltere Dateien aus dem Cache
                    # haben den Schluessel nicht, und das Lesen vertraegt es.
                    with open(temps_path(tsec), "w") as f:
                        json.dump({"temps": temps, "wind": wind}, f)
            if t == 0 or fcst_fresh(tsec):
                continue
            rgba = bytearray(4 * nx * ny)
            for r in range(ny):
                row1, row0 = acc[t][r], acc[t - 1][r]
                base = 4 * nx * (ny - 1 - r)
                for c in range(nx):
                    a1, a0 = row1[c], row0[c]
                    if a1 is None or a0 is None:
                        continue
                    col = rate_color(max(0.0, a1 - a0))
                    if col:
                        o = base + 4 * c
                        rgba[o:o + 4] = bytes(
                            (col[0], col[1], col[2], OVERLAY_ALPHA))
            with open(fcst_path(tsec), "wb") as f:
                f.write(png_bytes(nx, ny, rgba))
            print("fcst %s gerendert"
                  % datetime.fromtimestamp(tsec).strftime("%a %H:%M"))


def prefetch_chunk(cs):
    if cs in _prefetching or cs > int(time.time()) + FCST_HOURS * H:
        return
    _prefetching.add(cs)

    def run():
        try:
            render_chunk(cs)
        except Exception as e:            # noqa: BLE001
            print("prefetch %d: %s" % (cs, e), file=sys.stderr)
        finally:
            _prefetching.discard(cs)
    threading.Thread(target=run, daemon=True).start()


# ------------------------------------------------------- Basemap-Seeding ---


def tile_x(lon, z):
    return max(0, min(2 ** z - 1, int((lon + 180.0) / 360.0 * 2 ** z)))


def tile_y(lat, z):
    lat = max(-85.05, min(85.05, lat))
    y = (1.0 - math.asinh(math.tan(math.radians(lat))) / math.pi) / 2.0
    return max(0, min(2 ** z - 1, int(y * 2 ** z)))


SEED_PLAN = (
    (-85.0, -180.0, 85.0, 180.0, 3, 5),
    (34.0, -12.0, 62.0, 30.0, 6, 6),
    (45.0, 11.2, 50.8, 19.7, 7, 9),
)


def seed(plan=SEED_PLAN):
    os.makedirs(CACHEDIR, exist_ok=True)
    jobs = []
    for lat0, lon0, lat1, lon1, zmin, zmax in plan:
        for z in range(zmin, zmax + 1):
            x0, x1 = tile_x(lon0, z), tile_x(lon1, z)
            y0, y1 = tile_y(lat1, z), tile_y(lat0, z)
            for ty in range(y0, y1 + 1):
                for tx in range(x0, x1 + 1):
                    jobs.append((z, tx, ty))
    jobs = sorted(set(jobs))
    todo = [(z, x, y) for z, x, y in jobs if not os.path.exists(
        os.path.join(CACHEDIR, "osm_%d_%d_%d.png" % (z, x, y)))]
    print("seed: %d Tiles gesamt, %d fehlen" % (len(jobs), len(todo)))
    ok = err = 0
    for i, (z, x, y) in enumerate(todo, 1):
        try:
            data = fetch(OSM_TILE.format(z=z, x=x, y=y))
            with open(os.path.join(
                    CACHEDIR, "osm_%d_%d_%d.png" % (z, x, y)), "wb") as f:
                f.write(data)
            ok += 1
        except Exception as e:            # noqa: BLE001
            err += 1
            print("  z%d/%d/%d: %s" % (z, x, y, e), file=sys.stderr)
        if i % 25 == 0 or i == len(todo):
            print("  %d/%d" % (i, len(todo)))
        time.sleep(0.25)
    print("seed fertig: %d geladen, %d Fehler" % (ok, err))


# --------------------------------------------------------------- Server ----

WOTAG = ("Mo", "Di", "Mi", "Do", "Fr", "Sa", "So")


def label_for(tsec):
    lt = datetime.fromtimestamp(tsec)
    return "%s %02d:%02d" % (WOTAG[lt.weekday()], lt.hour, lt.minute)


def build_index():
    now = int(time.time())
    frames = []
    try:
        rv_refresh(force=True)
    except Exception as e:                # noqa: BLE001
        print("rv index: %s" % e, file=sys.stderr)
    with _rv_lock:
        for t in sorted(_rv_paths):
            if t >= now - 3900:
                frames.append({"kind": "past", "t": t,
                               "label": label_for(t)})
    h0 = now // H * H + H
    for t in range(h0, now + FCST_HOURS * H, H):
        frames.append({"kind": "fcst", "t": t, "label": label_for(t)})
    # bounds absichtlich lazy (/bounds.json), sonst laedt der Index
    # einen kompletten AROME-Chunk nur fuer die Geo-Bounds
    return json.dumps({"generated": now,
                       "bounds": load_grid().get("bounds", {}),
                       "vienna": VIENNA, "frames": frames}).encode()


RE_OSM = re.compile(r"^/osm/(\d+)/(\d+)/(\d+)\.png$")
RE_RV = re.compile(r"^/rv/(\d+)/(\d+)/(\d+)/(\d+)\.png$")
RE_FC = re.compile(r"^/fcst/(\d+)\.png$")
RE_PT = re.compile(r"^/points/(\d+)\.json$")


class Handler(BaseHTTPRequestHandler):
    def log_message(self, *a):
        pass

    def _send(self, data, code=200, ctype="image/png"):
        self.send_response(code)
        self.send_header("Content-Type", ctype)
        self.send_header("Content-Length", str(len(data)))
        self.end_headers()
        self.wfile.write(data)

    def do_GET(self):
        path, _, query = self.path.partition("?")
        if path == "/index.json":
            try:
                return self._send(build_index(), 200, "application/json")
            except Exception as e:        # noqa: BLE001
                return self._send(str(e).encode(), 502, "text/plain")
        if path == "/bounds.json":
            # persistiertes Grid zuerst; Chunk-Download nur wenn die UI
            # das Overlay wirklich anzeigen will (t=...) und nichts da ist
            load_grid()
            q = urllib.parse.parse_qs(query)
            if not _grid and "t" in q:
                try:
                    render_chunk(chunk_start(int(q["t"][0])))
                except Exception as e:    # noqa: BLE001
                    print("bounds: %s" % e, file=sys.stderr)
            return self._send(json.dumps(
                _grid.get("bounds", {})).encode(), 200,
                "application/json")
        m = RE_PT.match(path)
        if m:
            t = int(m.group(1))
            q = urllib.parse.parse_qs(query)
            try:
                bbox = [float(v) for v in q["bbox"][0].split(",")]
                z = int(q.get("z", ["6"])[0])
                pts = points_for(t, bbox, z)
                body = json.dumps({"points": pts})
            except Exception as e:        # noqa: BLE001
                print("points: %s" % e, file=sys.stderr)
                body = "{\"points\":[]}"
            return self._send(body.encode(), 200, "application/json")
        m = RE_OSM.match(path)
        if m:
            z, x, y = m.groups()
            return self._tile(OSM_TILE.format(z=z, x=x, y=y),
                              "osm_%s_%s_%s.png" % (z, x, y))
        m = RE_RV.match(path)
        if m:
            t, z, x, y = (int(v) for v in m.groups())
            p = rv_path_for(t)
            if not p:
                return self._send(EMPTY_PNG)
            url = "https://tilecache.rainviewer.com%s/512/%d/%d/%d/%s/%s.png" \
                % (p, z, x, y, RV_COLOR, RV_OPTS)
            return self._tile(url, "rv_%d_%d_%d_%d.png" % (t, z, x, y))
        m = RE_FC.match(path)
        if m:
            t = int(m.group(1))
            if not fcst_fresh(t):
                try:
                    render_chunk(chunk_start(t))
                except Exception as e:    # noqa: BLE001
                    print("fcst %d: %s" % (t, e), file=sys.stderr)
            if t >= chunk_start(t) + CHUNK - H:   # Chunk-Grenze naht
                prefetch_chunk(chunk_start(t) + CHUNK)
            if os.path.exists(fcst_path(t)):
                with open(fcst_path(t), "rb") as f:
                    return self._send(f.read())
            return self._send(b"not ready", 404, "text/plain")
        self._send(b"not found", 404, "text/plain")

    def _tile(self, url, name):
        p = os.path.join(CACHEDIR, name)
        if os.path.exists(p):
            with open(p, "rb") as f:
                return self._send(f.read())
        try:
            data = fetch(url)
        except Exception as e:            # noqa: BLE001
            return self._send(str(e).encode(), 502, "text/plain")
        with open(p, "wb") as f:
            f.write(data)
        self._send(data)


def serve(port=PORT):
    os.makedirs(CACHEDIR, exist_ok=True)
    cut = time.time() - 4 * 3600
    for fn in os.listdir(CACHEDIR):
        if (fn.startswith("rv_") or fn.startswith("fcst_")
                or fn.startswith("temps_")) and \
                os.path.getmtime(os.path.join(CACHEDIR, fn)) < cut:
            os.unlink(os.path.join(CACHEDIR, fn))
    print("regenkarte: http://127.0.0.1:%d/index.json" % port)
    ThreadingHTTPServer(("127.0.0.1", port), Handler).serve_forever()


if __name__ == "__main__":
    os.environ.setdefault("TZ", "Europe/Vienna")
    try:
        time.tzset()
    except AttributeError:
        pass
    args = sys.argv[1:]
    if args and args[0] == "seed":
        if len(args) >= 7:
            seed(((float(args[1]), float(args[2]), float(args[3]),
                   float(args[4]), int(args[5]), int(args[6])),))
        else:
            seed()
    elif args and args[0] == "serve":
        serve(int(args[1]) if len(args) > 1 else PORT)
    else:
        serve(PORT)
