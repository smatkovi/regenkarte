# Regenkarte

Eine Regenradar-Karte für Nokia N9 und N950 (MeeGo Harmattan): weltweit
schwenk- und zoombar, mit RainViewer-Radar in 10-Minuten-Schritten und
AROME-Niederschlagsprognose von GeoSphere Austria (stündlich, +48 h).
Temperatur und Wind lassen sich einblenden.

## Aufbau

| Teil | was |
|------|-----|
| `dienst/` | **Kartendienst** — der Kachel- und Prognoseteil, Rust, statisch gegen musl. Läuft auf `127.0.0.1:8642`. |
| `src/main.qml`, `src/RadarMap.qml` | Die Oberfläche (Qt Quick 1.1). |
| `src/launcher.py` | qmlviewer-Ersatz für Harmattan, PySide unter Python 2.7. |
| `src/start.sh` | Zieht den Dienst hoch, falls er nicht läuft, und startet dann die Oberfläche. |
| `src/cities.json` | Städteliste aus GeoNames, nach Einwohnern sortiert. |
| `alt/regenkarte.py` | Die Python-Vorlage des Dienstes bis 1.3.1 — die Spezifikation, nach der `dienst/` gebaut ist. |

Die Oberfläche spricht den Dienst über feste Pfade an; wer dort etwas
verschiebt, muss beide Seiten anfassen:

    /index.json                 Bilderliste und Geo-Grenzen
    /bounds.json[?t=…]          nur die Grenzen
    /osm/{z}/{x}/{y}.png        OSM-Grundkarte, dauerhaft zwischengespeichert
    /rv/{t}/{z}/{x}/{y}.png     RainViewer-Radar
    /fcst/{t}.png               AROME-Regen als Overlay, auf Abruf gerendert
    /points/{t}.json?bbox=…&z=…  Temperatur, Wind, Regen an Orten

## Warum der Dienst seit 2.0 in Rust ist

Er lag den ganzen Tag im Speicher und belegte dabei **10,3 MB** — auf
einem Gerät mit 1 GB, auf dem der Kern sonst den Seitencache wegwirft,
ist das spürbar. Die Rust-Fassung belegt **0,5 MB** im Leerlauf und
**2,9 MB**, sobald die Städteliste geladen ist (32 401 Orte; die ist
der Grund, dass es nicht weniger wird).

Nebenbei fällt eine Abhängigkeit weg: gebraucht wurde das
nachinstallierte `/opt/wunderw/bin/python3.11` nur, weil Harmattans
eigenes Python kein TLS 1.2 kann und keine der Datenquellen mehr
erreicht. Der Dienst bringt rustls mit und braucht davon nichts.

Was *nicht* anders ist: dieselben Endpunkte, dasselbe Kachelraster,
dasselbe Format im Zwischenspeicher (`osm_z_x_y.png`, `rv_*`, `fcst_*`,
`temps_*`, `grid.json`). Ein vorhandener Cache unter
`~/MyDocs/regenkarte/cache` wird unverändert weiterbenutzt.

## Die zwei Stellen, an denen es hakt

**AROME liefert aufsummierten Niederschlag** (`rr_acc`), nicht die
Stundenrate. Die Rate ist die Differenz zweier Stunden, und deshalb wird
jeder Abschnitt eine Stunde früher begonnen, als er gebraucht wird —
sonst hat das erste Bild keinen Vorgänger.

**Die Punkte im Bildausschnitt werden vor dem Abruf ausgedünnt**, nicht
danach. Ausgewählt wird nach Einwohnerzahl (Wien ist ausgenommen und
bleibt immer stehen), und die Werte werden nur für die Gewinner geholt.
Über Mobilfunk ist das der Unterschied zwischen einer und fünfzig
Abfragen.

## Bauen

    sh dienst/tools/build.sh         # -> dienst/build/kartendienst (armel)
    tools/build-deb.sh 2.0.0         # -> regenkarte_2.0.0_armel.deb

Aufs Gerät mit **`aegis-dpkg -i`**, nicht `dpkg -i`. Lehnt aegis das
Ersetzen ab („not replacing it from less trusted source"), vorher
`dpkg -r regenkarte`.

Die Grundkarte einmal am WLAN vorladen (dauert, schont unterwegs das
Datenvolumen):

    /opt/regenkarte/kartendienst seed

## Datenquellen

RainViewer.com, GeoSphere Austria (CC BY 4.0), Open-Meteo.com (CC BY 4.0),
© OpenStreetMap contributors, Städteliste aus GeoNames (CC BY 4.0).

Lizenz: GPL-3.0-or-later.
