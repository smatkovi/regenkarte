// RadarMap.qml -- Harmattan (com.nokia.meego 1.1)
// Weltweite Pan/Zoom-Regenkarte, on demand vom lokalen Proxy.
// Temperatur/Regen-Punkte: Oesterreich = TAWES/AROME, sonst Open-Meteo.
import QtQuick 1.1
import com.nokia.meego 1.1

Page {
    id: page
    orientationLock: PageOrientation.LockLandscape

    property string proxy: "http://127.0.0.1:8642"
    property variant frames: []
    property variant bounds: ({})
    property real vLat: 48.2082
    property real vLon: 16.3738
    property int zl: 6                   // Zoomstufe 3..9
    property int zMin: 3
    property int zMax: 9
    property bool firstLoad: true

    property int fi: 0
    property variant cur: frames.length ? frames[fi] : undefined
    property bool showTemps: false       // Standard: aus
    property bool showWind: false        // Windpfeile, ebenfalls aus
    property variant points: []          // [{lat, lon, v, c?}]
    property string pointsFor: ""        // frameKey|bbox der Punkte
    property real ptsRetryAt: 0          // Re-Poll bei leerem Ergebnis
    property bool ovVisible: false       // AROME-Overlay im Sichtfeld?
    property bool boundsPending: false
    property real boundsRetryAt: 0

    // grober Oesterreich-Schnitttest (Konstanten statt variant-Literal --
    // QtQuick-1.1-Falle bei Objekt-Defaults in variant-Properties)
    function atVisible() {
        var x0 = geoX(12.8), x1 = geoX(19.0)
        var y0 = geoY(49.3), y1 = geoY(46.3)
        return !(x1 < flick.contentX
              || x0 > flick.contentX + flick.width
              || y1 < flick.contentY
              || y0 > flick.contentY + flick.height)
    }
    function rectVisible(b) {
        if (b.lon0 === undefined) return false
        var x0 = geoX(b.lon0), x1 = geoX(b.lon1)
        var y0 = geoY(b.lat_top), y1 = geoY(b.lat_bot)
        return !(x1 < flick.contentX || x0 > flick.contentX + flick.width
              || y1 < flick.contentY || y0 > flick.contentY + flick.height)
    }
    // Bounds erst holen, wenn das Overlay wirklich gebraucht wird --
    // der Server laedt dafuer den ersten AROME-Chunk
    function loadBounds() {
        if (boundsPending || cur === undefined) return
        if (Date.now() < boundsRetryAt) return
        boundsPending = true
        var xhr = new XMLHttpRequest()
        xhr.onreadystatechange = function () {
            if (xhr.readyState !== 4) return
            boundsPending = false
            if (xhr.status !== 200) { boundsRetryAt = Date.now() + 15000
                                      return }
            try {
                var j = JSON.parse(xhr.responseText)
                if (j.lon0 !== undefined) { bounds = j; upd.restart() }
                else boundsRetryAt = Date.now() + 15000
            } catch (e) { boundsRetryAt = Date.now() + 15000 }
        }
        xhr.open("GET", proxy + "/bounds.json?t=" + cur.t)
        xhr.send()
    }

    function frameKey() {
        if (cur === undefined) return ""
        return cur.kind === "past" ? "past" : "" + cur.t
    }

    function ws() { return 256 * Math.pow(2, zl) }
    function mercY(lat) {
        return Math.log(Math.tan(Math.PI / 4 + lat * Math.PI / 360))
    }
    function geoX(lon) { return (lon + 180) / 360 * ws() }
    function geoY(lat) { return (1 - mercY(lat) / Math.PI) / 2 * ws() }
    function lonAt(x) { return x / ws() * 360 - 180 }
    function latAt(y) {
        var a = Math.PI * (1 - 2 * y / ws())
        var sinh = (Math.exp(a) - Math.exp(-a)) / 2   // ES5: kein Math.sinh
        return Math.atan(sinh) * 180 / Math.PI
    }
    // Sichtbarer Ausschnitt als "lat0,lon0,lat1,lon1" (mit Rand)
    function bboxStr() {
        var m = 30
        var lat0 = latAt(flick.contentY + flick.height + m)
        var lat1 = latAt(flick.contentY - m)
        var lon0 = lonAt(flick.contentX - m)
        var lon1 = lonAt(flick.contentX + flick.width + m)
        return lat0.toFixed(2) + "," + lon0.toFixed(2) + ","
             + lat1.toFixed(2) + "," + lon1.toFixed(2)
    }

    function load() {
        var keepT = (frames.length && !firstLoad) ? frames[fi].t : -1
        var xhr = new XMLHttpRequest()
        xhr.onreadystatechange = function () {
            if (xhr.readyState !== 4) return
            if (xhr.status !== 200 || xhr.responseText.length === 0) return
            var j = JSON.parse(xhr.responseText)
            frames = j.frames
            bounds = j.bounds
            if (j.vienna) { vLat = j.vienna[0]; vLon = j.vienna[1] }
            var i = 0
            if (keepT > 0) {
                var best = 1e15
                for (var k = 0; k < frames.length; k++) {
                    var d = Math.abs(frames[k].t - keepT)
                    if (d < best) { best = d; i = k }
                }
            } else {
                for (var k2 = 0; k2 < frames.length; k2++)
                    if (frames[k2].kind === "past") i = k2
            }
            fi = i
            slider.value = i
            if (firstLoad) { firstLoad = false; centerOn(vLat, vLon) }
            pointsFor = ""               // Refresh: Punkte neu holen
            upd.restart()
        }
        xhr.open("GET", proxy + "/index.json")
        xhr.send()
    }
    // Punkte fuer aktuellen Frame + Viewport laden; verspaetete oder
    // nicht mehr passende Antworten werden verworfen
    function loadPoints() {
        if ((!showTemps && !showWind) || cur === undefined) return
        var key = frameKey() + "|" + bboxStr()
        if (key === pointsFor) {
            // leeres Ergebnis (z.B. Temps-Sampling schlug fehl):
            // nach 10 s erneut versuchen statt fuer immer aufgeben
            if (points.length > 0 || Date.now() < ptsRetryAt) return
            pointsFor = ""
            ptsRetryAt = Date.now() + 10000
        }
        var t = cur.t
        var xhr = new XMLHttpRequest()
        xhr.onreadystatechange = function () {
            if (xhr.readyState !== 4 || xhr.status !== 200) return
            if (frameKey() + "|" + bboxStr() !== key) return
            try {
                var j = JSON.parse(xhr.responseText)
                points = j.points || []
                pointsFor = key
            } catch (e) { points = []; pointsFor = "" }
            upd.restart()
        }
        xhr.open("GET", proxy + "/points/" + t + ".json?bbox="
                 + bboxStr() + "&z=" + zl)
        xhr.send()
    }
    function centerOn(lat, lon) {
        flick.contentX = geoX(lon) - flick.width / 2
        flick.contentY = geoY(lat) - flick.height / 2
        upd.restart()
    }
    function setZoom(nz) {
        nz = Math.max(zMin, Math.min(zMax, nz))
        if (nz === zl) return
        var f = Math.pow(2, nz - zl)
        var cx = (flick.contentX + flick.width / 2) * f - flick.width / 2
        var cy = (flick.contentY + flick.height / 2) * f - flick.height / 2
        zl = nz
        flick.contentX = Math.max(0, Math.min(cx, ws() - flick.width))
        flick.contentY = Math.max(0, Math.min(cy, ws() - flick.height))
        upd.restart()
    }
    Component.onCompleted: load()
    Timer {                              // Index alle 10 min auffrischen
        interval: 600000; repeat: true; running: true
        onTriggered: load()
    }

    // ---------------------------------------------------- Tile-Engine ---
    Component {
        id: tileComp
        Image {
            property string key
            property bool dead: false
            asynchronous: true
        }
    }

    function updTiles(layer, tz, urlFn) {
        var n = Math.pow(2, tz)
        var px = ws() / n
        var x0 = Math.max(0, Math.floor(flick.contentX / px) - 1)
        var y0 = Math.max(0, Math.floor(flick.contentY / px) - 1)
        var x1 = Math.min(n - 1,
                 Math.floor((flick.contentX + flick.width) / px) + 1)
        var y1 = Math.min(n - 1,
                 Math.floor((flick.contentY + flick.height) / px) + 1)

        var need = {}
        for (var ty = y0; ty <= y1; ty++)
            for (var tx = x0; tx <= x1; tx++)
                need[tz + "/" + tx + "/" + ty + "|" + urlFn(tz, tx, ty)] =
                    [tx, ty]
        for (var i = 0; i < layer.children.length; i++) {
            var it = layer.children[i]
            if (it.dead) continue
            if (need[it.key] !== undefined) {
                // Geometrie nachziehen: bei Overzoom (Radar > z7)
                // bleibt der Key ueber Zoomstufen gleich, die
                // Pixelgroesse aber nicht
                var keep = need[it.key]
                it.x = keep[0] * px; it.y = keep[1] * px
                it.width = px + 1; it.height = px + 1
                delete need[it.key]
            } else {
                it.dead = true; it.visible = false; it.destroy()
            }
        }
        for (var k in need) {
            var txy = need[k]
            tileComp.createObject(layer, {
                key: k,
                x: txy[0] * px, y: txy[1] * px,
                width: px + 1, height: px + 1,
                source: k.substring(k.indexOf("|") + 1)
            })
        }
    }
    function osmUrl(tz, tx, ty) {
        return proxy + "/osm/" + tz + "/" + tx + "/" + ty + ".png"
    }
    function rvUrl(tz, tx, ty) {
        return proxy + "/rv/" + cur.t + "/" + tz + "/" + tx + "/"
               + ty + ".png"
    }

    // ------------------------------------------------ Temperatur-Chips ---
    Component {
        id: ptComp
        Rectangle {
            property string key
            property bool dead: false
            property alias txt: lbl.text
            property string halo: ""
            color: halo !== "" ? halo : "transparent"
            opacity: halo !== "" ? 0.92 : 1.0
            radius: 10
            width: lbl.width + 12; height: lbl.height + 6
            transform: Translate { x: -width / 2; y: -height / 2 }
            Text {
                id: lbl
                anchors.centerIn: parent
                color: "white"
                style: Text.Outline; styleColor: "black"
                font.pixelSize: 20; font.bold: true
            }
        }
    }

    // ----------------------------------------------------- Windfahnen ---
    // Der Pfeil zeigt, WOHIN der Wind weht; die Richtungsangabe sagt, WOHER
    // er kommt -- daher die 180 Grad. Der Zahlenwert dreht sich nicht mit,
    // sonst steht er auf dem Kopf.
    Component {
        id: wndComp
        Item {
            property string key
            property bool dead: false
            property real dir: 0
            property alias txt: wlbl.text
            width: 54; height: 30
            transform: Translate { x: -width / 2; y: -height / 2 }

            Text {
                id: pfeil
                anchors { left: parent.left; verticalCenter: parent.verticalCenter }
                text: "\u2191"
                color: "#cfe8ff"
                style: Text.Outline; styleColor: "black"
                font.pixelSize: 26; font.bold: true
                rotation: dir + 180
            }
            Text {
                id: wlbl
                anchors { left: pfeil.right; leftMargin: 2
                          verticalCenter: parent.verticalCenter }
                color: "#cfe8ff"
                style: Text.Outline; styleColor: "black"
                font.pixelSize: 18; font.bold: true
            }
        }
    }

    function updWind() {
        var need = {}
        if (showWind && cur !== undefined
                && pointsFor.split("|")[0] === frameKey()) {
            for (var i = 0; i < points.length; i++) {
                var p = points[i]
                if (p.w === undefined) continue
                need["W" + p.lat + "," + p.lon + "|" + p.w + "|"
                     + (p.d === undefined ? "-" : p.d) + "|" + zl] = p
            }
        }
        for (var j = 0; j < wndLayer.children.length; j++) {
            var it = wndLayer.children[j]
            if (it.dead) continue
            if (need[it.key] !== undefined) {
                delete need[it.key]
            } else {
                it.dead = true; it.visible = false; it.destroy()
            }
        }
        for (var k in need) {
            var q = need[k]
            wndComp.createObject(wndLayer, {
                key: k, x: geoX(q.lon),
                // etwas tiefer als die Temperatur, damit sich beide
                // Anzeigen am selben Ort nicht ueberdecken
                y: geoY(q.lat) + (showTemps ? 26 : 0),
                dir: q.d === undefined ? 0 : q.d,
                txt: q.w.toFixed(1)
            })
        }
    }

    function updPoints() {
        var need = {}
        if (showTemps && cur !== undefined
                && pointsFor.split("|")[0] === frameKey()) {
            for (var i = 0; i < points.length; i++) {
                var p = points[i]
                if (p.v === undefined) continue
                var halo = p.c !== undefined ? p.c : ""
                need["P" + p.lat + "," + p.lon + "|" + p.v + "|" + halo
                     + "|" + zl] = p
            }
        }
        for (var j = 0; j < ptLayer.children.length; j++) {
            var it = ptLayer.children[j]
            if (it.dead) continue
            if (need[it.key] !== undefined) {
                delete need[it.key]
            } else {
                it.dead = true; it.visible = false; it.destroy()
            }
        }
        for (var k in need) {
            var q = need[k]
            ptComp.createObject(ptLayer, {
                key: k, x: geoX(q.lon), y: geoY(q.lat),
                txt: "" + Math.round(q.v) + "\u00b0",
                halo: q.c !== undefined ? q.c : ""
            })
        }
        updWind()
    }

    function refresh() {
        updTiles(osmLayer, zl, osmUrl)
        if (cur !== undefined && cur.kind === "past")
            updTiles(rvLayer, Math.min(zl, 7), rvUrl)
        else
            updTiles(rvLayer, 0, function () { return "" })
        // Overlay nur laden/zeigen, wenn der Alpenraum im Bild ist
        if (cur !== undefined && cur.kind === "fcst") {
            if (bounds.lon0 !== undefined) {
                ovVisible = rectVisible(bounds)
            } else {
                ovVisible = false
                if (atVisible()) loadBounds()
            }
        } else {
            ovVisible = false
        }
        updPoints()
        pts.restart()                    // Punkte fuer neue Lage nachladen
    }
    Timer { id: upd; interval: 60; onTriggered: refresh() }
    Timer { id: pts; interval: 350; onTriggered: loadPoints() }

    Rectangle { anchors.fill: parent; color: "#0e0e12" }

    Flickable {
        id: flick
        anchors.fill: parent
        contentWidth: ws()
        contentHeight: ws()
        boundsBehavior: Flickable.StopAtBounds
        pressDelay: 0
        onContentXChanged: upd.restart()
        onContentYChanged: upd.restart()

        Item { id: osmLayer; opacity: 0.62 }
        Item { id: rvLayer }

        Image {                                  // AROME-Overlay
            id: ovImg
            property int nonce: 0
            visible: ovVisible && status === Image.Ready
            x: ovVisible ? geoX(bounds.lon0) : 0
            y: ovVisible ? geoY(bounds.lat_top) : 0
            width: ovVisible ? geoX(bounds.lon1) - geoX(bounds.lon0) : 1
            height: ovVisible
                    ? geoY(bounds.lat_bot) - geoY(bounds.lat_top) : 1
            smooth: true
            cache: false
            asynchronous: true
            source: ovVisible
                    ? proxy + "/fcst/" + cur.t + ".png?n=" + nonce : ""
            onStatusChanged: if (status === Image.Error) ovRetry.restart()
        }
        Timer {                                  // Overlay-Reload nach 404
            id: ovRetry
            interval: 4000
            onTriggered: if (ovVisible) ovImg.nonce++
        }
        Item {                                   // Wien-Marker
            x: geoX(vLon); y: geoY(vLat)
            Rectangle { x: -6; y: -6; width: 12; height: 12; radius: 6
                        color: "transparent"
                        border { color: "white"; width: 2 } }
            Rectangle { x: -3; y: -3; width: 6; height: 6; radius: 3
                        color: "#ff3c3c" }
        }
        Item { id: ptLayer }                     // Temperatur-Chips
        Item { id: wndLayer }                    // Windfahnen
        MouseArea {
            anchors.fill: parent
            onClicked: player.running = !player.running
        }
    }

    Timer {
        id: player
        interval: 350
        repeat: true
        onTriggered: slider.value = (fi + 1) % frames.length
    }

    Text {
        anchors { top: parent.top; left: parent.left; margins: 8 }
        color: "white"
        style: Text.Outline; styleColor: "black"
        font.pixelSize: 28
        text: cur !== undefined
              ? cur.label + (cur.kind === "fcst" ? "  (Prognose)"
                                                 : "  (Radar)")
              : "lade Index \u2026"
    }
    Text {
        anchors { top: parent.top; right: parent.right; margins: 8 }
        color: "#bbbbbb"; font.pixelSize: 20
        text: "z" + zl
    }

    Column {
        anchors { right: parent.right
                  verticalCenter: parent.verticalCenter; margins: 8 }
        spacing: 10
        Button { width: 64; height: 64; text: "+"
                 enabled: zl < zMax; onClicked: setZoom(zl + 1) }
        Button { width: 64; height: 64; text: "\u2212"
                 enabled: zl > zMin; onClicked: setZoom(zl - 1) }
        Button { width: 64; height: 64; text: "\u2302"
                 onClicked: centerOn(vLat, vLon) }
        Button { width: 64; height: 64; text: "\u00b0C"
                 opacity: showTemps ? 1.0 : 0.4
                 onClicked: {
                     showTemps = !showTemps
                     pointsFor = ""
                     if (showTemps || showWind) loadPoints()
                     upd.restart()
                 } }
        Button { width: 64; height: 64; text: "m/s"
                 opacity: showWind ? 1.0 : 0.4
                 onClicked: {
                     showWind = !showWind
                     pointsFor = ""
                     if (showTemps || showWind) loadPoints()
                     upd.restart()
                 } }
    }

    Slider {
        id: slider
        anchors { left: parent.left; right: parent.right
                  bottom: parent.bottom; margins: 6 }
        minimumValue: 0
        maximumValue: Math.max(0, frames.length - 1)
        stepSize: 1
        valueIndicatorVisible: false
        onValueChanged: {
            var v = Math.round(value)
            if (v !== fi) { fi = v; upd.restart() }
        }
        onPressedChanged: if (pressed) player.running = false
    }
}
