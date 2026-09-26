//! Der Dienst auf 127.0.0.1:8642.
//!
//! Die Oberflaeche (`main.qml`, `RadarMap.qml`) kennt genau diese
//! Endpunkte und dieses Kachelraster -- hier darf sich nichts verschieben:
//!
//! | Pfad | was |
//! |------|-----|
//! | `/index.json` | Bilderliste und Geo-Grenzen |
//! | `/bounds.json` | nur die Grenzen, ohne einen Abschnitt zu laden |
//! | `/osm/{z}/{x}/{y}.png` | OSM-Grundkarte, dauerhaft zwischengespeichert |
//! | `/rv/{t}/{z}/{x}/{y}.png` | RainViewer-Radar |
//! | `/fcst/{t}.png` | AROME-Regen als Overlay, auf Abruf gerendert |
//! | `/points/{t}.json?bbox=…&z=…` | Temperatur, Wind, Regen an Orten |
//!
//! Ein eigener kleiner HTTP-Server statt einer Bibliothek: gebraucht wird
//! GET, ein Statuscode und drei Kopfzeilen. Je Verbindung ein Faden und
//! danach zu -- genau wie es die Python-Fassung tat.

use std::collections::HashMap;
use std::io::{BufRead, BufReader, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::{LazyLock, Mutex};

use serde_json::json;

use crate::arome;
use crate::bild;
use crate::konfig::*;
use crate::netz;
use crate::zeit::{self, H};

// --- RainViewer-Bilderliste ---------------------------------------------

struct RvStand {
    pfade: HashMap<i64, String>,
    geholt: u64,
}

static RV: LazyLock<Mutex<RvStand>> = LazyLock::new(|| {
    Mutex::new(RvStand {
        pfade: HashMap::new(),
        geholt: 0,
    })
});

fn rv_auffrischen(erzwingen: bool) -> Result<(), String> {
    {
        let stand = RV.lock().unwrap();
        let alter = (zeit::jetzt() as u64).saturating_sub(stand.geholt);
        if !erzwingen && alter < 180 {
            return Ok(());
        }
    }
    let index = netz::holen_json(RV_INDEX)?;
    let mut pfade = HashMap::new();
    if let Some(liste) = index["radar"]["past"].as_array() {
        for p in liste {
            if let (Some(t), Some(pfad)) = (p["time"].as_i64(), p["path"].as_str()) {
                pfade.insert(t, pfad.to_string());
            }
        }
    }
    let mut stand = RV.lock().unwrap();
    stand.pfade = pfade;
    stand.geholt = zeit::jetzt() as u64;
    Ok(())
}

fn rv_pfad_fuer(t: i64) -> Option<String> {
    if let Err(e) = rv_auffrischen(false) {
        eprintln!("rv index: {}", e);
    }
    RV.lock().unwrap().pfade.get(&t).cloned()
}

// --- Bilderliste ---------------------------------------------------------

fn index_bauen() -> Vec<u8> {
    let jetzt = zeit::jetzt();
    let mut bilder = Vec::new();
    if let Err(e) = rv_auffrischen(true) {
        eprintln!("rv index: {}", e);
    }
    {
        let stand = RV.lock().unwrap();
        let mut zeiten: Vec<i64> = stand.pfade.keys().copied().collect();
        zeiten.sort();
        for t in zeiten {
            if t >= jetzt - 3900 {
                bilder.push(json!({
                    "kind": "past", "t": t, "label": zeit::beschriftung(t)
                }));
            }
        }
    }
    let erste = jetzt / H * H + H;
    let mut t = erste;
    while t < jetzt + FCST_STUNDEN * H {
        bilder.push(json!({
            "kind": "fcst", "t": t, "label": zeit::beschriftung(t)
        }));
        t += H;
    }
    // Die Grenzen bleiben absichtlich faul (siehe /bounds.json): sonst
    // laedt der Index einen ganzen AROME-Abschnitt, nur um sie zu kennen.
    json!({
        "generated": jetzt,
        "bounds": arome::grenzen(),
        "vienna": [WIEN.0, WIEN.1],
        "frames": bilder,
    })
    .to_string()
    .into_bytes()
}

// --- Kacheln -------------------------------------------------------------

fn kachel(url: &str, name: &str) -> (u16, &'static str, Vec<u8>) {
    let pfad = cacheverzeichnis().join(name);
    if let Ok(inhalt) = std::fs::read(&pfad) {
        return (200, "image/png", inhalt);
    }
    match netz::holen(url) {
        Ok(daten) => {
            if let Err(e) = std::fs::write(&pfad, &daten) {
                eprintln!("kachel schreiben: {}", e);
            }
            (200, "image/png", daten)
        }
        Err(e) => (502, "text/plain", e.into_bytes()),
    }
}

// --- Abfrageteil zerlegen ------------------------------------------------

fn abfrage(text: &str) -> HashMap<String, String> {
    let mut aus = HashMap::new();
    for paar in text.split('&') {
        if paar.is_empty() {
            continue;
        }
        let (schluessel, wert) = paar.split_once('=').unwrap_or((paar, ""));
        aus.insert(entziffern(schluessel), entziffern(wert));
    }
    aus
}

fn entziffern(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut aus = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'%' if i + 2 < bytes.len() => {
                let hex = std::str::from_utf8(&bytes[i + 1..i + 3]).unwrap_or("");
                match u8::from_str_radix(hex, 16) {
                    Ok(b) => {
                        aus.push(b);
                        i += 3;
                    }
                    Err(_) => {
                        aus.push(bytes[i]);
                        i += 1;
                    }
                }
            }
            b'+' => {
                aus.push(b' ');
                i += 1;
            }
            b => {
                aus.push(b);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&aus).into_owned()
}

/// Zerlegt "/osm/7/68/44.png" in die Zahlen dazwischen.
fn zahlen_aus(pfad: &str, anfang: &str, endung: &str, wieviele: usize) -> Option<Vec<i64>> {
    let rest = pfad.strip_prefix(anfang)?.strip_suffix(endung)?;
    let teile: Vec<&str> = rest.split('/').collect();
    if teile.len() != wieviele {
        return None;
    }
    teile.iter().map(|t| t.parse::<i64>().ok()).collect()
}

// --- Beantworten ---------------------------------------------------------

fn antwort(pfad: &str, frage: &str) -> (u16, &'static str, Vec<u8>) {
    if pfad == "/index.json" {
        return (200, "application/json", index_bauen());
    }

    if pfad == "/bounds.json" {
        // Das gespeicherte Gitter zuerst; einen Abschnitt herunterladen nur
        // dann, wenn die Oberflaeche das Overlay wirklich zeigen will.
        let mut grenzen = arome::grenzen();
        if grenzen.as_object().map(|o| o.is_empty()).unwrap_or(true) {
            if let Some(t) = abfrage(frage).get("t").and_then(|v| v.parse::<i64>().ok()) {
                if let Err(e) = arome::abschnitt_rendern(arome::abschnittsanfang(t)) {
                    eprintln!("bounds: {}", e);
                }
                grenzen = arome::grenzen();
            }
        }
        return (200, "application/json", grenzen.to_string().into_bytes());
    }

    if let Some(zahlen) = zahlen_aus(pfad, "/points/", ".json", 1) {
        let q = abfrage(frage);
        let werte: Option<Vec<f64>> = q.get("bbox").map(|b| {
            b.split(',')
                .filter_map(|v| v.parse::<f64>().ok())
                .collect()
        });
        let body = match werte {
            Some(v) if v.len() == 4 => {
                let z = q.get("z").and_then(|s| s.parse::<i64>().ok()).unwrap_or(6);
                let punkte = crate::orte::punkte_fuer(zahlen[0], (v[0], v[1], v[2], v[3]), z);
                json!({ "points": punkte }).to_string()
            }
            _ => "{\"points\":[]}".to_string(),
        };
        return (200, "application/json", body.into_bytes());
    }

    if let Some(z) = zahlen_aus(pfad, "/osm/", ".png", 3) {
        let url = OSM_KACHEL
            .replace("{z}", &z[0].to_string())
            .replace("{x}", &z[1].to_string())
            .replace("{y}", &z[2].to_string());
        return kachel(&url, &format!("osm_{}_{}_{}.png", z[0], z[1], z[2]));
    }

    if let Some(v) = zahlen_aus(pfad, "/rv/", ".png", 4) {
        let (t, z, x, y) = (v[0], v[1], v[2], v[3]);
        let Some(p) = rv_pfad_fuer(t) else {
            return (200, "image/png", bild::leeres_png());
        };
        let url = format!(
            "https://tilecache.rainviewer.com{}/512/{}/{}/{}/{}/{}.png",
            p, z, x, y, RV_FARBE, RV_OPTS
        );
        return kachel(&url, &format!("rv_{}_{}_{}_{}.png", t, z, x, y));
    }

    if let Some(v) = zahlen_aus(pfad, "/fcst/", ".png", 1) {
        let t = v[0];
        if !arome::fcst_frisch(t) {
            if let Err(e) = arome::abschnitt_rendern(arome::abschnittsanfang(t)) {
                eprintln!("fcst {}: {}", t, e);
            }
        }
        // Naht die Abschnittsgrenze, den naechsten schon einmal holen.
        if t >= arome::abschnittsanfang(t) + arome::CHUNK - H {
            arome::im_voraus_holen(arome::abschnittsanfang(t) + arome::CHUNK);
        }
        return match std::fs::read(arome::fcst_pfad(t)) {
            Ok(inhalt) => (200, "image/png", inhalt),
            Err(_) => (404, "text/plain", b"not ready".to_vec()),
        };
    }

    (404, "text/plain", b"not found".to_vec())
}

fn bedienen(strom: TcpStream) {
    let mut leser = BufReader::new(match strom.try_clone() {
        Ok(s) => s,
        Err(_) => return,
    });
    let mut zeile = String::new();
    if leser.read_line(&mut zeile).is_err() {
        return;
    }
    // Den Rest des Kopfes wegleseen, sonst sieht der Klient einen Abbruch.
    loop {
        let mut weitere = String::new();
        match leser.read_line(&mut weitere) {
            Ok(0) => break,
            Ok(_) => {
                if weitere.trim().is_empty() {
                    break;
                }
            }
            Err(_) => break,
        }
    }

    let mut felder = zeile.split_whitespace();
    let verb = felder.next().unwrap_or("");
    let ziel = felder.next().unwrap_or("/");
    let (pfad, frage) = ziel.split_once('?').unwrap_or((ziel, ""));

    let (code, art, inhalt) = if verb == "GET" {
        antwort(pfad, frage)
    } else {
        (405, "text/plain", b"nur GET".to_vec())
    };

    let mut strom = strom;
    let kopf = format!(
        "HTTP/1.0 {} {}\r\nContent-Type: {}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        code,
        if code == 200 { "OK" } else { "Error" },
        art,
        inhalt.len()
    );
    let _ = strom.write_all(kopf.as_bytes());
    let _ = strom.write_all(&inhalt);
    let _ = strom.flush();
}

/// Alte Kacheln und Bilder wegraeumen. Die Grundkarte bleibt -- sie
/// aendert sich nicht und ist ueber Mobilfunk teuer.
fn aufraeumen() {
    let Ok(eintraege) = std::fs::read_dir(cacheverzeichnis()) else {
        return;
    };
    for eintrag in eintraege.flatten() {
        let name = eintrag.file_name();
        let name = name.to_string_lossy();
        if !(name.starts_with("rv_") || name.starts_with("fcst_") || name.starts_with("temps_")) {
            continue;
        }
        let zu_alt = eintrag
            .metadata()
            .and_then(|m| m.modified())
            .map(|m| m.elapsed().map(|d| d.as_secs() > 4 * 3600).unwrap_or(false))
            .unwrap_or(false);
        if zu_alt {
            let _ = std::fs::remove_file(eintrag.path());
        }
    }
}

pub fn bedienen_auf(port: u16) -> Result<(), String> {
    std::fs::create_dir_all(cacheverzeichnis()).map_err(|e| e.to_string())?;
    aufraeumen();
    let horcher = TcpListener::bind(("127.0.0.1", port)).map_err(|e| e.to_string())?;
    println!("regenkarte: http://127.0.0.1:{}/index.json", port);
    for verbindung in horcher.incoming() {
        match verbindung {
            Ok(strom) => {
                std::thread::spawn(move || bedienen(strom));
            }
            Err(e) => eprintln!("annehmen: {}", e),
        }
    }
    Ok(())
}

#[cfg(test)]
mod proben {
    use super::*;

    #[test]
    fn pfade_werden_richtig_zerlegt() {
        assert_eq!(zahlen_aus("/osm/7/68/44.png", "/osm/", ".png", 3), Some(vec![7, 68, 44]));
        assert_eq!(
            zahlen_aus("/rv/1790431200/7/68/44.png", "/rv/", ".png", 4),
            Some(vec![1790431200, 7, 68, 44])
        );
        assert_eq!(zahlen_aus("/fcst/1790431200.png", "/fcst/", ".png", 1), Some(vec![1790431200]));
        // Eine Kachel zu wenig ist keine Kachel.
        assert_eq!(zahlen_aus("/osm/7/68.png", "/osm/", ".png", 3), None);
        assert_eq!(zahlen_aus("/osm/7/x/44.png", "/osm/", ".png", 3), None);
    }

    #[test]
    fn abfrage_wird_entziffert() {
        let q = abfrage("bbox=46.3%2C12.8%2C49.3%2C19.0&z=7");
        assert_eq!(q.get("bbox").map(|s| s.as_str()), Some("46.3,12.8,49.3,19.0"));
        assert_eq!(q.get("z").map(|s| s.as_str()), Some("7"));
    }

    #[test]
    fn unbekanntes_ist_404() {
        let (code, _, _) = antwort("/gibtsnicht", "");
        assert_eq!(code, 404);
    }
}
