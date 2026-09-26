//! Punkte im Bildausschnitt: Staedte, Stationen, Werte.
//!
//! Oesterreich bekommt TAWES-Messwerte beziehungsweise AROME-Prognosen,
//! der Rest der Welt Open-Meteo. Ausgeduennt wird vorher und nach
//! Bevoelkerung -- die groessere Stadt gewinnt --, und die Werte werden
//! **nur fuer die Gewinner** geholt. Das ist kein Feinschliff, sondern
//! der Unterschied zwischen einer und fuenfzig Abfragen ueber eine
//! Mobilfunkverbindung.

use std::collections::HashMap;
use std::sync::{LazyLock, Mutex};

use serde_json::{json, Value};

use crate::arome;
use crate::konfig::*;
use crate::netz;
use crate::zeit::{self, H};

#[derive(Clone)]
pub struct Station {
    pub id: String,
    pub lat: f64,
    pub lon: f64,
    /// Groesste Stadt im Umkreis -- danach wird ausgeduennt.
    pub pri: f64,
    pub wien: bool,
}

#[derive(Clone, Default)]
pub struct Werte {
    pub t: Option<f64>,
    pub ff: Option<f64>,
    pub dd: Option<f64>,
}

// --- Staedte -------------------------------------------------------------

struct Staedte {
    /// [lat, lon, Einwohner], nach Einwohnern absteigend.
    nach_groesse: Vec<(f64, f64, f64)>,
    /// dieselben, nach Breitengrad sortiert -- fuer die Umkreissuche.
    nach_breite: Vec<(f64, f64, f64)>,
}

static STAEDTE: LazyLock<Mutex<Option<Staedte>>> = LazyLock::new(|| Mutex::new(None));

fn mit_staedten<T>(tu: impl FnOnce(&Staedte) -> T) -> T {
    let mut halter = STAEDTE.lock().unwrap();
    if halter.is_none() {
        let mut nach_groesse = Vec::new();
        match std::fs::read(staedtedatei()).ok().and_then(|b| {
            serde_json::from_slice::<Vec<Vec<f64>>>(&b).ok()
        }) {
            Some(liste) => {
                for c in liste {
                    if c.len() >= 3 {
                        nach_groesse.push((c[0], c[1], c[2]));
                    }
                }
                eprintln!("staedte: {} geladen", nach_groesse.len());
            }
            None => eprintln!("staedte: {} nicht lesbar", staedtedatei().display()),
        }
        let mut nach_breite = nach_groesse.clone();
        nach_breite.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));
        *halter = Some(Staedte {
            nach_groesse,
            nach_breite,
        });
    }
    tu(halter.as_ref().unwrap())
}

/// Groesste Stadtbevoelkerung im Umkreis -- die Prioritaet einer Station.
fn einwohner_in_der_naehe(lat: f64, lon: f64) -> f64 {
    const DLAT: f64 = 0.18;
    const DLON: f64 = 0.28;
    mit_staedten(|s| {
        let anfang = s
            .nach_breite
            .partition_point(|c| c.0 < lat - DLAT);
        let mut beste = 1000.0;
        for c in &s.nach_breite[anfang..] {
            if c.0 > lat + DLAT {
                break;
            }
            if (c.1 - lon).abs() <= DLON && c.2 > beste {
                beste = c.2;
            }
        }
        beste
    })
}

// --- TAWES-Stationen -----------------------------------------------------

struct Stationsstand {
    liste: Vec<Station>,
    parameter: String,
    geholt: u64,
}

static STATIONEN: LazyLock<Mutex<Stationsstand>> = LazyLock::new(|| {
    Mutex::new(Stationsstand {
        liste: Vec::new(),
        parameter: TEMP_OBS_PARAM.to_string(),
        geholt: 0,
    })
});

fn alter(seit: u64) -> u64 {
    let jetzt = zeit::jetzt() as u64;
    jetzt.saturating_sub(seit)
}

pub fn stationen() -> Result<Vec<Station>, String> {
    {
        let stand = STATIONEN.lock().unwrap();
        if !stand.liste.is_empty() && alter(stand.geholt) < STATIONEN_TTL {
            return Ok(stand.liste.clone());
        }
    }
    let meta = netz::holen_json(&format!("{}/metadata", ST_BASE))?;

    // Der Parametername kommt manchmal in anderer Schreibweise zurueck.
    let mut parameter = TEMP_OBS_PARAM.to_string();
    let namen: Vec<String> = meta["parameters"]
        .as_array()
        .map(|a| {
            a.iter()
                .filter_map(|p| p["name"].as_str().map(|s| s.to_string()))
                .collect()
        })
        .unwrap_or_default();
    if !namen.iter().any(|n| n == TEMP_OBS_PARAM) {
        if let Some(n) = namen
            .iter()
            .find(|n| n.eq_ignore_ascii_case(TEMP_OBS_PARAM))
        {
            parameter = n.clone();
        }
    }

    let mut liste = Vec::new();
    if let Some(rohe) = meta["stations"].as_array() {
        for s in rohe {
            if s["is_active"] == Value::Bool(false) {
                continue;
            }
            let id = s["id"]
                .as_str()
                .map(|x| x.to_string())
                .or_else(|| s["id"].as_i64().map(|x| x.to_string()))
                .or_else(|| s["station_id"].as_str().map(|x| x.to_string()))
                .unwrap_or_default();
            let lat = s["lat"].as_f64().or_else(|| s["latitude"].as_f64());
            let lon = s["lon"].as_f64().or_else(|| s["longitude"].as_f64());
            let (Some(lat), Some(lon)) = (lat, lon) else {
                continue;
            };
            if id.is_empty() {
                continue;
            }
            liste.push(Station {
                id,
                lat,
                lon,
                pri: einwohner_in_der_naehe(lat, lon),
                wien: im_kasten(lat, lon, WIEN_BOX),
            });
        }
    }
    eprintln!("stationen: {} (Parameter {})", liste.len(), parameter);
    let mut stand = STATIONEN.lock().unwrap();
    stand.liste = liste.clone();
    stand.parameter = parameter;
    stand.geholt = zeit::jetzt() as u64;
    Ok(liste)
}

pub fn stationsparameter() -> String {
    STATIONEN.lock().unwrap().parameter.clone()
}

fn celsius(v: f64) -> f64 {
    // Manche Reihen kommen in Kelvin; alles ueber 150 kann keine
    // Lufttemperatur in Grad sein.
    let c = if v > 150.0 { v - 273.15 } else { v };
    (c * 10.0).round() / 10.0
}

struct Messstand {
    daten: HashMap<String, Werte>,
    geholt: u64,
}

static MESSWERTE: LazyLock<Mutex<Messstand>> = LazyLock::new(|| {
    Mutex::new(Messstand {
        daten: HashMap::new(),
        geholt: 0,
    })
});

/// Temperatur und Wind aller Stationen, in **einem** Abruf.
///
/// Zwei getrennte Abrufe waeren die naheliegende Erweiterung gewesen und
/// die falsche: die Schnittstelle nimmt mehrere Parameter auf einmal, und
/// das Geraet haengt oft an einer langsamen Mobilfunkverbindung.
pub fn messwerte() -> Result<HashMap<String, Werte>, String> {
    {
        let stand = MESSWERTE.lock().unwrap();
        if !stand.daten.is_empty() && alter(stand.geholt) < OBS_TTL {
            return Ok(stand.daten.clone());
        }
    }
    let liste = stationen()?;
    let parameter = stationsparameter();
    let kennungen: Vec<&str> = liste.iter().map(|s| s.id.as_str()).collect();
    let mut felder = vec![format!("parameters={}", parameter)];
    for p in WIND_OBS_PARAMS {
        felder.push(format!("parameters={}", p));
    }
    let url = format!(
        "{}?{}&station_ids={}&output_format=geojson",
        ST_BASE,
        felder.join("&"),
        kennungen.join(",")
    );
    let daten = netz::holen_json(&url)?;

    let mut aus: HashMap<String, Werte> = HashMap::new();
    if let Some(merkmale) = daten["features"].as_array() {
        for f in merkmale {
            let eigenschaften = &f["properties"];
            let sid = eigenschaften["station"]
                .as_str()
                .map(|s| s.to_string())
                .or_else(|| eigenschaften["station"].as_i64().map(|s| s.to_string()))
                .unwrap_or_default();
            let parameter_baum = &eigenschaften["parameters"];
            let letzter = |name: &str| -> Option<f64> {
                parameter_baum[name]["data"]
                    .as_array()
                    .and_then(|a| a.last())
                    .and_then(|v| v.as_f64())
            };
            let t = letzter(&parameter);
            let ff = letzter(WIND_OBS_PARAMS[0]);
            let dd = letzter(WIND_OBS_PARAMS[1]);
            if t.is_none() && ff.is_none() {
                continue;
            }
            aus.insert(
                sid,
                Werte {
                    t: t.map(celsius),
                    ff: ff.map(|v| (v * 10.0).round() / 10.0),
                    dd: dd.map(|v| v.round()),
                },
            );
        }
    }
    eprintln!("messwerte: {} Stationen (Temperatur und Wind)", aus.len());
    let mut stand = MESSWERTE.lock().unwrap();
    stand.daten = aus.clone();
    stand.geholt = zeit::jetzt() as u64;
    Ok(aus)
}

// --- Open-Meteo ----------------------------------------------------------

#[derive(Clone)]
struct OmEintrag {
    geholt: u64,
    /// Temperatur, Niederschlag, Windgeschwindigkeit, Windrichtung
    jetzt: (Option<f64>, Option<f64>, Option<f64>, Option<f64>),
    stunden: HashMap<i64, (Option<f64>, Option<f64>, Option<f64>, Option<f64>)>,
}

/// Geschluesselt ueber die Bitmuster der Koordinaten -- dieselbe Stadt
/// liefert dieselben Bits, und Fliesskommazahlen lassen sich sonst nicht
/// als Schluessel benutzen.
type OrtsSchluessel = (u64, u64);

fn schluessel(lat: f64, lon: f64) -> OrtsSchluessel {
    (lat.to_bits(), lon.to_bits())
}

static OM: LazyLock<Mutex<HashMap<OrtsSchluessel, OmEintrag>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

/// Fehlende oder veraltete Orte nachladen, in Stapeln.
fn om_fuellen(orte: &[(f64, f64)]) -> Result<(), String> {
    let noetig: Vec<(f64, f64)> = {
        let zwischenspeicher = OM.lock().unwrap();
        orte.iter()
            .copied()
            .filter(|(la, lo)| match zwischenspeicher.get(&schluessel(*la, *lo)) {
                None => true,
                Some(e) => alter(e.geholt) > OM_TTL,
            })
            .collect()
    };

    for stapel in noetig.chunks(OM_BATCH) {
        // wind_speed_unit=ms ist Pflicht: Open-Meteo liefert sonst km/h,
        // und die Anzeige soll m/s zeigen wie die TAWES-Messwerte.
        let url = format!(
            "{}?latitude={}&longitude={}\
             &hourly=temperature_2m,precipitation,wind_speed_10m,wind_direction_10m\
             &current=temperature_2m,precipitation,wind_speed_10m,wind_direction_10m\
             &wind_speed_unit=ms&forecast_days=2&timeformat=unixtime&timezone=UTC",
            OM_URL,
            stapel
                .iter()
                .map(|k| format!("{:.3}", k.0))
                .collect::<Vec<_>>()
                .join(","),
            stapel
                .iter()
                .map(|k| format!("{:.3}", k.1))
                .collect::<Vec<_>>()
                .join(",")
        );
        let daten = netz::holen_json(&url)?;
        let liste: Vec<Value> = match daten {
            Value::Array(a) => a,
            anderes => vec![anderes],
        };
        let mut zwischenspeicher = OM.lock().unwrap();
        for (ort, d) in stapel.iter().zip(liste.iter()) {
            let stunde = &d["hourly"];
            let zeiten = stunde["time"].as_array().cloned().unwrap_or_default();
            let hole = |name: &str| -> Vec<Value> {
                stunde[name].as_array().cloned().unwrap_or_default()
            };
            let tt = hole("temperature_2m");
            let pr = hole("precipitation");
            let wf = hole("wind_speed_10m");
            let wd = hole("wind_direction_10m");
            let mut stunden = HashMap::new();
            for (j, ts) in zeiten.iter().enumerate() {
                let Some(ts) = ts.as_i64() else { continue };
                stunden.insert(
                    ts,
                    (
                        tt.get(j).and_then(|v| v.as_f64()),
                        pr.get(j).and_then(|v| v.as_f64()),
                        wf.get(j).and_then(|v| v.as_f64()),
                        wd.get(j).and_then(|v| v.as_f64()),
                    ),
                );
            }
            let n = &d["current"];
            zwischenspeicher.insert(
                schluessel(ort.0, ort.1),
                OmEintrag {
                    geholt: zeit::jetzt() as u64,
                    jetzt: (
                        n["temperature_2m"].as_f64(),
                        n["precipitation"].as_f64(),
                        n["wind_speed_10m"].as_f64(),
                        n["wind_direction_10m"].as_f64(),
                    ),
                    stunden,
                },
            );
        }
        eprintln!("open-meteo: {} Orte geladen", stapel.len());
    }
    Ok(())
}

// --- Auswahl und Werte ---------------------------------------------------

/// Web-Mercator: Bildpunkt einer Koordinate bei dieser Zoomstufe.
fn bildpunkt(lat: f64, lon: f64, weltbreite: f64) -> (f64, f64) {
    let x = (lon + 180.0) / 360.0 * weltbreite;
    let y = (1.0 - lat.to_radians().tan().asinh() / std::f64::consts::PI) / 2.0 * weltbreite;
    (x, y)
}

/// Punkte im Bildausschnitt, fertig fuer die Oberflaeche.
///
/// Je Punkt: `v` Temperatur [Grad], `w` Windgeschwindigkeit [m/s],
/// `d` Windrichtung [Grad, woher]. Was fehlt, fehlt einfach -- die
/// Oberflaeche blendet ein, was da ist.
pub fn punkte_fuer(t: i64, bbox: (f64, f64, f64, f64), z: i64) -> Vec<Value> {
    let (lat0, lon0, lat1, lon1) = bbox;
    let jetzt = zeit::jetzt();
    let vergangen = t <= jetzt + 60;
    let th = t / H * H;
    let weltbreite = 256.0 * (2f64).powi(z as i32);

    // --- Kandidaten (Werte erst spaeter) ---------------------------------
    // (Prioritaet, Wien, lat, lon, Stationskennung)
    let mut kandidaten: Vec<(f64, bool, f64, f64, Option<String>)> = Vec::new();
    let mut stationswerte: HashMap<String, Werte> = HashMap::new();

    if kaesten_ueberlappen(bbox, AT_BBOX) {
        if vergangen {
            match messwerte() {
                Ok(w) => stationswerte = w,
                Err(e) => eprintln!("stationswerte: {}", e),
            }
        } else {
            match arome::stationswerte_fuer(th) {
                Ok(w) => stationswerte = w,
                Err(e) => eprintln!("stationswerte: {}", e),
            }
        }
        match stationen() {
            Ok(liste) => {
                for s in liste {
                    if !(lat0 <= s.lat && s.lat <= lat1 && lon0 <= s.lon && s.lon <= lon1) {
                        continue;
                    }
                    let Some(e) = stationswerte.get(&s.id) else {
                        continue;
                    };
                    if e.t.is_none() && e.ff.is_none() {
                        continue;
                    }
                    kandidaten.push((s.pri, s.wien, s.lat, s.lon, Some(s.id.clone())));
                }
            }
            Err(e) => eprintln!("stationen: {}", e),
        }
    }

    mit_staedten(|s| {
        for (lat, lon, pop) in &s.nach_groesse {
            if !(lat0 <= *lat && *lat <= lat1 && lon0 <= *lon && *lon <= lon1) {
                continue;
            }
            if im_kasten(*lat, *lon, AT_BBOX) {
                continue; // dort gilt TAWES/AROME
            }
            kandidaten.push((*pop, false, *lat, *lon, None));
            if kandidaten.len() >= 6 * MAX_PTS {
                break;
            }
        }
    });

    // --- Wien-Ausnahme, dann gierig nach Prioritaet -----------------------
    kandidaten.sort_by(|a, b| {
        (!a.1)
            .cmp(&(!b.1))
            .then(b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal))
    });
    let mut gewaehlt: Vec<(f64, f64, Option<String>)> = Vec::new();
    let mut belegt: Vec<(f64, f64)> = Vec::new();
    for (_, wien, lat, lon, sid) in kandidaten {
        let (x, y) = bildpunkt(lat, lon, weltbreite);
        if !wien
            && belegt
                .iter()
                .any(|(bx, by)| (bx - x).abs() < MIN_PX && (by - y).abs() < MIN_PX)
        {
            continue;
        }
        belegt.push((x, y));
        gewaehlt.push((lat, lon, sid));
        if gewaehlt.len() >= MAX_PTS {
            break;
        }
    }

    // --- Werte nur fuer die Gewinner -------------------------------------
    let offene: Vec<(f64, f64)> = gewaehlt
        .iter()
        .filter(|(_, _, sid)| sid.is_none())
        .map(|(la, lo, _)| (*la, *lo))
        .collect();
    if let Err(e) = om_fuellen(&offene) {
        eprintln!("open-meteo: {}", e);
    }

    let zwischenspeicher = OM.lock().unwrap();
    let mut aus = Vec::new();
    for (lat, lon, sid) in gewaehlt {
        if let Some(sid) = sid {
            let Some(e) = stationswerte.get(&sid) else {
                continue;
            };
            let mut p = json!({ "lat": lat, "lon": lon });
            if let Some(v) = e.t {
                p["v"] = json!((v * 10.0).round() / 10.0);
            }
            if let Some(ff) = e.ff {
                p["w"] = json!((ff * 10.0).round() / 10.0);
                if let Some(dd) = e.dd {
                    p["d"] = json!(dd.round() as i64);
                }
            }
            aus.push(p);
            continue;
        }
        let Some(e) = zwischenspeicher.get(&schluessel(lat, lon)) else {
            continue;
        };
        let werte = if vergangen {
            Some(e.jetzt)
        } else {
            e.stunden.get(&th).copied()
        };
        let Some((v, r, ff, dd)) = werte else { continue };
        if v.is_none() && ff.is_none() {
            continue;
        }
        let mut p = json!({ "lat": lat, "lon": lon });
        if let Some(v) = v {
            p["v"] = json!((v * 10.0).round() / 10.0);
        }
        if let Some(ff) = ff {
            p["w"] = json!((ff * 10.0).round() / 10.0);
            if let Some(dd) = dd {
                p["d"] = json!(dd.round() as i64);
            }
        }
        if !vergangen {
            if let Some(r) = r {
                if r >= 0.1 {
                    let c = rate_hex(r);
                    if !c.is_empty() {
                        // Prognoseregen ohne Overlay: die Farbe klebt am Punkt.
                        p["c"] = json!(c);
                    }
                }
            }
        }
        aus.push(p);
    }
    aus
}

#[cfg(test)]
mod proben {
    use super::*;

    #[test]
    fn celsius_erkennt_kelvin() {
        assert_eq!(celsius(285.15), 12.0);
        assert_eq!(celsius(12.04), 12.0);
        assert_eq!(celsius(-3.26), -3.3);
    }

    #[test]
    fn bildpunkt_am_nullmeridian() {
        let (x, y) = bildpunkt(0.0, 0.0, 256.0);
        assert!((x - 128.0).abs() < 1e-9);
        assert!((y - 128.0).abs() < 1e-9);
    }
}
