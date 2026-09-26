//! Die Niederschlagsprognose von GeoSphere, auf Abruf.
//!
//! AROME kommt als Gitter von Punkten mit **aufsummiertem** Niederschlag
//! (`rr_acc`). Die Stundenrate ist die Differenz zweier aufeinander
//! folgender Stunden -- deshalb wird jeder Abschnitt eine Stunde frueher
//! begonnen als er gebraucht wird, sonst fehlt dem ersten Bild sein
//! Vorgaenger.
//!
//! Geholt wird in Abschnitten von drei Stunden (dem AROME-Zyklus) und nur
//! dann, wenn die Oberflaeche ein Bild daraus wirklich anzeigen will. Ein
//! Abschnitt kostet ueber Mobilfunk spuerbar Zeit; im Voraus geholt wird
//! genau einer, wenn die Grenze naht.

use std::collections::{HashMap, HashSet};
use std::sync::{LazyLock, Mutex};

use serde_json::{json, Value};

use crate::bild;
use crate::konfig::*;
use crate::netz;
use crate::orte::{self, Werte};
use crate::zeit::{self, H};

pub const CHUNK: i64 = FCST_CHUNK_H * H;

pub fn abschnittsanfang(t: i64) -> i64 {
    t / CHUNK * CHUNK
}

pub fn fcst_pfad(t: i64) -> std::path::PathBuf {
    cacheverzeichnis().join(format!("fcst_{}.png", t))
}

pub fn temps_pfad(t: i64) -> std::path::PathBuf {
    cacheverzeichnis().join(format!("temps_{}.json", t))
}

fn frisch(pfad: &std::path::Path) -> bool {
    let Ok(daten) = std::fs::metadata(pfad) else {
        return false;
    };
    let Ok(geaendert) = daten.modified() else {
        return false;
    };
    geaendert
        .elapsed()
        .map(|d| d.as_secs() < FCST_TTL)
        .unwrap_or(false)
}

pub fn fcst_frisch(t: i64) -> bool {
    frisch(&fcst_pfad(t))
}

pub fn temps_frisch(t: i64) -> bool {
    frisch(&temps_pfad(t))
}

// --- Gitter --------------------------------------------------------------

#[derive(Default, Clone)]
pub struct Gitter {
    pub lats: Vec<f64>,
    pub lons: Vec<f64>,
    pub grenzen: Value,
}

static GITTER: LazyLock<Mutex<Gitter>> = LazyLock::new(|| Mutex::new(Gitter::default()));

fn gitterdatei() -> std::path::PathBuf {
    cacheverzeichnis().join("grid.json")
}

/// Das gespeicherte Gitter laden -- es ueberlebt einen Neustart, und ohne
/// es muesste `/bounds.json` einen ganzen Abschnitt herunterladen, nur um
/// die Geo-Grenzen zu kennen.
pub fn gitter_laden() -> Gitter {
    {
        let g = GITTER.lock().unwrap();
        if !g.lats.is_empty() {
            return g.clone();
        }
    }
    let Ok(rohdaten) = std::fs::read(gitterdatei()) else {
        return Gitter::default();
    };
    let Ok(gespeichert): Result<Value, _> = serde_json::from_slice(&rohdaten) else {
        return Gitter::default();
    };
    // Nur uebernehmen, wenn die Konfiguration unveraendert ist.
    let erwartet = json!([FCST_BBOX.0, FCST_BBOX.1, FCST_BBOX.2, FCST_BBOX.3]);
    if gespeichert["fbbox"] != erwartet {
        return Gitter::default();
    }
    let hole = |name: &str| -> Vec<f64> {
        gespeichert[name]
            .as_array()
            .map(|a| a.iter().filter_map(|v| v.as_f64()).collect())
            .unwrap_or_default()
    };
    let neu = Gitter {
        lats: hole("lats"),
        lons: hole("lons"),
        grenzen: gespeichert["bounds"].clone(),
    };
    if !neu.lats.is_empty() {
        *GITTER.lock().unwrap() = neu.clone();
    }
    neu
}

fn gitter_sichern(g: &Gitter) {
    let inhalt = json!({
        "lats": g.lats,
        "lons": g.lons,
        "bounds": g.grenzen,
        "fbbox": [FCST_BBOX.0, FCST_BBOX.1, FCST_BBOX.2, FCST_BBOX.3],
    });
    if let Err(e) = std::fs::write(gitterdatei(), inhalt.to_string()) {
        eprintln!("gitter sichern: {}", e);
    }
}

pub fn grenzen() -> Value {
    let g = gitter_laden();
    if g.grenzen.is_null() {
        json!({})
    } else {
        g.grenzen
    }
}

/// Index des naechstgelegenen Gitterwerts.
fn naechster_index(achse: &[f64], v: f64) -> usize {
    if achse.is_empty() {
        return 0;
    }
    let i = achse.partition_point(|a| *a < v);
    if i == 0 {
        return 0;
    }
    if i >= achse.len() {
        return achse.len() - 1;
    }
    if achse[i] - v < v - achse[i - 1] {
        i
    } else {
        i - 1
    }
}

/// AROME liefert Windkomponenten, angezeigt wird Betrag und Herkunft.
///
/// Meteorologisch zaehlt die Richtung, **aus der** es weht -- daher die
/// 270 und das Vorzeichen: weht es nach Osten (u > 0), kommt es aus 270
/// Grad, also aus West.
fn wind_aus_uv(u: f32, v: f32) -> Option<(f64, f64)> {
    if u.is_nan() || v.is_nan() {
        return None;
    }
    let (u, v) = (u as f64, v as f64);
    let ff = u.hypot(v);
    let dd = (270.0 - v.atan2(u).to_degrees()).rem_euclid(360.0);
    Some(((ff * 10.0).round() / 10.0, dd.round()))
}

// --- Rueckzug bei Stoerung ----------------------------------------------

static RUECKZUG: LazyLock<Mutex<HashMap<i64, (u32, u64)>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

fn rueckzug_vorbei(abschnitt: i64) -> bool {
    let tabelle = RUECKZUG.lock().unwrap();
    match tabelle.get(&abschnitt) {
        None => true,
        Some((_, wann)) => zeit::jetzt() as u64 >= *wann,
    }
}

/// Nach einem Fehlschlag warten, bevor derselbe Abschnitt wieder versucht
/// wird: erst gleich wieder, bei Dauerstoerung immer laenger.
fn rueckzug_merken(abschnitt: i64, geklappt: bool) {
    let mut tabelle = RUECKZUG.lock().unwrap();
    if geklappt {
        tabelle.remove(&abschnitt);
        return;
    }
    let eintrag = tabelle.entry(abschnitt).or_insert((0, 0));
    eintrag.0 += 1;
    let wartezeit = if eintrag.0 < 6 {
        (30u64 * (1 << (eintrag.0 - 1))).min(1800)
    } else {
        FCST_TTL
    };
    eintrag.1 = zeit::jetzt() as u64 + wartezeit;
    eprintln!(
        "temps-Rueckzug Abschnitt {}: Versuch {}, naechster in {}s",
        abschnitt, eintrag.0, wartezeit
    );
}

// --- Abruf und Bilder ----------------------------------------------------

fn gs_url(anfang: i64, ende: i64, mit_temp: bool, mit_wind: bool) -> String {
    let mut felder = format!("parameters={}", RR_PARAM);
    if mit_temp {
        felder.push_str(&format!("&parameters={}", TEMP_FCST_PARAM));
    }
    if mit_wind {
        for name in WIND_FCST_PARAMS {
            felder.push_str(&format!("&parameters={}", name));
        }
    }
    format!(
        "{}?{}&bbox={:.2},{:.2},{:.2},{:.2}&output_format=geojson&start={}&end={}",
        GS_BASE,
        felder,
        FCST_BBOX.0,
        FCST_BBOX.1,
        FCST_BBOX.2,
        FCST_BBOX.3,
        zeit::utc_minuten(anfang),
        zeit::utc_minuten(ende)
    )
}

static ABRUF: LazyLock<Mutex<()>> = LazyLock::new(|| Mutex::new(()));

/// Einen Abschnitt holen und daraus Bilder und Stationswerte schreiben.
pub fn abschnitt_rendern(abschnitt: i64) -> Result<(), String> {
    let _nur_einer = ABRUF.lock().unwrap();

    let mut gitter = gitter_laden();
    let alles_da = !gitter.lats.is_empty()
        && (abschnitt..=abschnitt + CHUNK)
            .step_by(H as usize)
            .all(|t| fcst_frisch(t) && temps_frisch(t));
    if alles_da {
        return Ok(());
    }

    // Rueckfallkette: erst alles, dann ohne Wind, zuletzt ohne
    // Temperatur. Faellt ein Parameter bei GeoSphere einmal aus, soll
    // nicht die ganze Regenvorhersage mit ausfallen.
    let versuche = [
        (abschnitt - H, true, true),
        (abschnitt, true, true),
        (abschnitt - H, true, false),
        (abschnitt, true, false),
        (abschnitt - H, false, false),
        (abschnitt, false, false),
    ];
    let mut daten: Option<Value> = None;
    for (anfang, mit_temp, mit_wind) in versuche {
        if let Ok(v) = netz::holen_json(&gs_url(anfang, abschnitt + CHUNK, mit_temp, mit_wind)) {
            daten = Some(v);
            break;
        }
    }
    let Some(daten) = daten else {
        return Err(format!("GeoSphere-Abschnitt {} nicht ladbar", abschnitt));
    };

    let stempel: Vec<String> = daten["timestamps"]
        .as_array()
        .map(|a| {
            a.iter()
                .filter_map(|v| v.as_str().map(|s| s.to_string()))
                .collect()
        })
        .unwrap_or_default();
    let leer = Vec::new();
    let merkmale = daten["features"].as_array().unwrap_or(&leer);
    if stempel.is_empty() || merkmale.is_empty() {
        return Err("GeoSphere-Antwort ohne Daten".into());
    }

    if gitter.lats.is_empty() {
        let mut lats: Vec<f64> = Vec::new();
        let mut lons: Vec<f64> = Vec::new();
        for f in merkmale {
            let k = &f["geometry"]["coordinates"];
            if let (Some(lon), Some(lat)) = (k[0].as_f64(), k[1].as_f64()) {
                lats.push((lat * 10000.0).round() / 10000.0);
                lons.push((lon * 10000.0).round() / 10000.0);
            }
        }
        lats.sort_by(|a, b| a.partial_cmp(b).unwrap());
        lats.dedup();
        lons.sort_by(|a, b| a.partial_cmp(b).unwrap());
        lons.dedup();
        if lats.len() < 2 || lons.len() < 2 {
            return Err("GeoSphere-Gitter unbrauchbar".into());
        }
        let dlat = (lats[lats.len() - 1] - lats[0]) / (lats.len() - 1) as f64;
        let dlon = (lons[lons.len() - 1] - lons[0]) / (lons.len() - 1) as f64;
        gitter = Gitter {
            grenzen: json!({
                "lon0": lons[0] - dlon / 2.0,
                "lon1": lons[lons.len() - 1] + dlon / 2.0,
                "lat_top": lats[lats.len() - 1] + dlat / 2.0,
                "lat_bot": lats[0] - dlat / 2.0,
            }),
            lats,
            lons,
        };
        *GITTER.lock().unwrap() = gitter.clone();
        gitter_sichern(&gitter);
        eprintln!(
            "gitter: {} x {} Punkte, {} Merkmale",
            gitter.lons.len(),
            gitter.lats.len(),
            merkmale.len()
        );
    }

    let nx = gitter.lons.len();
    let ny = gitter.lats.len();
    let nt = stempel.len();

    // Fehlende Werte sind NaN statt Option -- das spart auf einem Geraet
    // mit 1 GB ein paar Megabyte je Abschnitt.
    let mut acc = vec![f32::NAN; nt * ny * nx];
    let mut tmp = vec![f32::NAN; nt * ny * nx];
    let mut uwd = vec![f32::NAN; nt * ny * nx];
    let mut vwd = vec![f32::NAN; nt * ny * nx];
    let stelle = |t: usize, r: usize, c: usize| t * ny * nx + r * nx + c;

    for f in merkmale {
        let k = &f["geometry"]["coordinates"];
        let (Some(lon), Some(lat)) = (k[0].as_f64(), k[1].as_f64()) else {
            continue;
        };
        let r = naechster_index(&gitter.lats, lat);
        let c = naechster_index(&gitter.lons, lon);
        let parameter = &f["properties"]["parameters"];
        let reihe = |name: &str| -> Option<&Vec<Value>> { parameter[name]["data"].as_array() };
        let Some(regen) = reihe(RR_PARAM) else { continue };
        let temperatur = reihe(TEMP_FCST_PARAM);
        let u = reihe(WIND_FCST_PARAMS[0]);
        let v = reihe(WIND_FCST_PARAMS[1]);
        for t in 0..nt.min(regen.len()) {
            let i = stelle(t, r, c);
            if let Some(x) = regen[t].as_f64() {
                acc[i] = x as f32;
            }
            if let Some(tv) = temperatur.and_then(|a| a.get(t)).and_then(|v| v.as_f64()) {
                tmp[i] = tv as f32;
            }
            if let (Some(uv), Some(vv)) = (
                u.and_then(|a| a.get(t)).and_then(|v| v.as_f64()),
                v.and_then(|a| a.get(t)).and_then(|v| v.as_f64()),
            ) {
                uwd[i] = uv as f32;
                vwd[i] = vv as f32;
            }
        }
    }
    drop(daten);

    let stations_index: Vec<(String, usize, usize)> = match orte::stationen() {
        Ok(liste) => liste
            .into_iter()
            .filter(|s| {
                gitter.lats[0] <= s.lat
                    && s.lat <= gitter.lats[ny - 1]
                    && gitter.lons[0] <= s.lon
                    && s.lon <= gitter.lons[nx - 1]
            })
            .map(|s| {
                (
                    s.id,
                    naechster_index(&gitter.lats, s.lat),
                    naechster_index(&gitter.lons, s.lon),
                )
            })
            .collect(),
        Err(e) => {
            eprintln!("stationen: {}", e);
            Vec::new()
        }
    };

    for (t, ts) in stempel.iter().enumerate() {
        let Some(tsec) = zeit::stempel_zu_sekunden(ts) else {
            continue;
        };

        if !temps_frisch(tsec) {
            let mut temps = serde_json::Map::new();
            let mut wind = serde_json::Map::new();
            for (sid, r, c) in &stations_index {
                let i = stelle(t, *r, *c);
                if !tmp[i].is_nan() {
                    let v = tmp[i] as f64;
                    let grad = if v > 150.0 { v - 273.15 } else { v };
                    temps.insert(sid.clone(), json!((grad * 10.0).round() / 10.0));
                }
                if let Some((ff, dd)) = wind_aus_uv(uwd[i], vwd[i]) {
                    wind.insert(sid.clone(), json!([ff, dd]));
                }
            }
            // Leeres nicht speichern -- sonst gilt der Fehlschlag als
            // Ergebnis und wird nie wiederholt.
            if !temps.is_empty() {
                let inhalt = json!({ "temps": temps, "wind": wind });
                if let Err(e) = std::fs::write(temps_pfad(tsec), inhalt.to_string()) {
                    eprintln!("temps schreiben: {}", e);
                }
            }
        }

        // Das erste Bild hat keinen Vorgaenger, aus dem sich die
        // Stundenrate bilden liesse.
        if t == 0 || fcst_frisch(tsec) {
            continue;
        }
        let mut rgba = vec![0u8; 4 * nx * ny];
        for r in 0..ny {
            // Bilder laufen von oben nach unten, das Gitter von Sued nach Nord.
            let zeilenanfang = 4 * nx * (ny - 1 - r);
            for c in 0..nx {
                let a1 = acc[stelle(t, r, c)];
                let a0 = acc[stelle(t - 1, r, c)];
                if a1.is_nan() || a0.is_nan() {
                    continue;
                }
                if let Some((cr, cg, cb)) = rate_farbe(((a1 - a0) as f64).max(0.0)) {
                    let o = zeilenanfang + 4 * c;
                    rgba[o] = cr;
                    rgba[o + 1] = cg;
                    rgba[o + 2] = cb;
                    rgba[o + 3] = OVERLAY_ALPHA;
                }
            }
        }
        if let Err(e) = std::fs::write(fcst_pfad(tsec), bild::png(nx, ny, &rgba)) {
            eprintln!("fcst schreiben: {}", e);
        } else {
            eprintln!("fcst {} gerendert", zeit::beschriftung(tsec));
        }
    }
    Ok(())
}

static IM_VORAUS: LazyLock<Mutex<HashSet<i64>>> = LazyLock::new(|| Mutex::new(HashSet::new()));

/// Den naechsten Abschnitt im Hintergrund holen, wenn die Grenze naht.
pub fn im_voraus_holen(abschnitt: i64) {
    if abschnitt > zeit::jetzt() + FCST_STUNDEN * H {
        return;
    }
    {
        let mut laufend = IM_VORAUS.lock().unwrap();
        if !laufend.insert(abschnitt) {
            return;
        }
    }
    std::thread::spawn(move || {
        if let Err(e) = abschnitt_rendern(abschnitt) {
            eprintln!("Vorausholen {}: {}", abschnitt, e);
        }
        IM_VORAUS.lock().unwrap().remove(&abschnitt);
    });
}

/// Die aus AROME abgeleiteten Stationswerte einer Prognosestunde.
///
/// Auf dieselbe Form gebracht wie die Messwerte, damit weiter oben nicht
/// zwei Faelle zu unterscheiden sind.
pub fn stationswerte_fuer(th: i64) -> Result<HashMap<String, Werte>, String> {
    let abschnitt = abschnittsanfang(th);
    if !temps_frisch(th) && rueckzug_vorbei(abschnitt) {
        let ergebnis = abschnitt_rendern(abschnitt);
        rueckzug_merken(abschnitt, ergebnis.is_ok() && temps_frisch(th));
        ergebnis?;
    }
    let Ok(rohdaten) = std::fs::read(temps_pfad(th)) else {
        return Ok(HashMap::new());
    };
    let gelesen: Value = serde_json::from_slice(&rohdaten).map_err(|e| e.to_string())?;
    let mut aus: HashMap<String, Werte> = HashMap::new();
    if let Some(temps) = gelesen["temps"].as_object() {
        for (sid, v) in temps {
            aus.entry(sid.clone()).or_default().t = v.as_f64();
        }
    }
    // "wind" darf fehlen: aeltere Dateien aus dem Zwischenspeicher haben
    // den Schluessel nicht, und das Lesen vertraegt es.
    if let Some(wind) = gelesen["wind"].as_object() {
        for (sid, v) in wind {
            let eintrag = aus.entry(sid.clone()).or_default();
            eintrag.ff = v[0].as_f64();
            eintrag.dd = v[1].as_f64();
        }
    }
    Ok(aus)
}

#[cfg(test)]
mod proben {
    use super::*;

    #[test]
    fn naechster_index_findet_den_naeheren() {
        let achse = [1.0, 2.0, 3.0];
        assert_eq!(naechster_index(&achse, 0.0), 0);
        assert_eq!(naechster_index(&achse, 2.4), 1);
        assert_eq!(naechster_index(&achse, 2.6), 2);
        assert_eq!(naechster_index(&achse, 9.0), 2);
    }

    #[test]
    fn wind_kommt_aus_west_wenn_er_nach_osten_weht() {
        let (ff, dd) = wind_aus_uv(5.0, 0.0).unwrap();
        assert!((ff - 5.0).abs() < 1e-9);
        assert_eq!(dd, 270.0);
        let (_, dd) = wind_aus_uv(0.0, 5.0).unwrap();
        assert_eq!(dd, 180.0); // weht nach Norden = kommt aus Sued
    }

    #[test]
    fn abschnitt_rastet_auf_drei_stunden() {
        assert_eq!(abschnittsanfang(0), 0);
        assert_eq!(abschnittsanfang(CHUNK - 1), 0);
        assert_eq!(abschnittsanfang(CHUNK), CHUNK);
    }
}
