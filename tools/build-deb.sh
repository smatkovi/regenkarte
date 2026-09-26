#!/bin/sh
# Packt die Regenkarte.
#
# Seit 2.0 steckt der Kachel- und Prognosedienst in einem eigenen Programm
# (dienst/, Rust, statisch gegen musl) -- deshalb ist das Paket "armel" und
# nicht mehr "all". Die Oberflaeche bleibt Python 2.7 mit PySide, und das
# Python 3.11 aus /opt/wunderw wird gar nicht mehr gebraucht: gebraucht
# wurde es nur wegen TLS 1.2, und das bringt der Dienst jetzt selbst mit.
#
#   sh dienst/tools/build.sh      # zuerst: dienst/build/kartendienst
#   tools/build-deb.sh [version]
set -e
cd "$(dirname "$0")/.."

VERSION=${1:-2.0.0}
STAGE=build/stage
rm -rf "$STAGE"
mkdir -p "$STAGE/opt/regenkarte" "$STAGE/usr/share/applications" \
         "$STAGE/usr/share/icons/hicolor/80x80/apps" "$STAGE/DEBIAN"

if [ ! -x dienst/build/kartendienst ]; then
    echo "dienst/build/kartendienst fehlt -- erst dienst/tools/build.sh" >&2
    exit 1
fi
cp src/RadarMap.qml src/main.qml src/launcher.py \
   src/start.sh src/cities.json "$STAGE/opt/regenkarte/"
cp dienst/build/kartendienst "$STAGE/opt/regenkarte/"
cp regenkarte.desktop "$STAGE/usr/share/applications/"
cp icons/regenkarte.png "$STAGE/usr/share/icons/hicolor/80x80/apps/"
chmod 755 "$STAGE/opt/regenkarte/start.sh" \
          "$STAGE/opt/regenkarte/launcher.py" \
          "$STAGE/opt/regenkarte/kartendienst"

python3 - "$VERSION" <<'PY'
import base64, io, sys, textwrap
version = sys.argv[1]
icon = base64.b64encode(open("icons/regenkarte.png", "rb").read()).decode("ascii")
text = io.open("control.in", encoding="utf-8").read()
text = text.replace("@VERSION@", version)
text = text.replace("@ICON@",
                    "\n".join(" " + line for line in textwrap.wrap(icon, 76)))
io.open("build/stage/DEBIAN/control", "w", encoding="utf-8").write(text)
PY

python3 tools/mkdeb.py "$STAGE" "regenkarte_${VERSION}_armel.deb"
