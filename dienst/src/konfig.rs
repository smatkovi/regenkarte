//! Alles, was an einer Stelle stehen soll: Quellen, Schwellen, Pfade.
//!
//! Die Werte sind eins zu eins aus `regenkarte.py` uebernommen. Wer hier
//! etwas aendert, aendert es fuer die Oberflaeche mit -- `main.qml` und
//! `RadarMap.qml` rechnen mit denselben Endpunkten und demselben
//! Kachelraster.

use std::path::PathBuf;

pub const PORT: u16 = 8642;

pub const WIEN: (f64, f64) = (48.2082, 16.3738);

pub const FCST_STUNDEN: i64 = 48;
/// Chunkgroesse = AROME-Zyklus.
pub const FCST_CHUNK_H: i64 = 3;
pub const FCST_TTL: u64 = 3 * 3600;
/// lat_min, lon_min, lat_max, lon_max
pub const FCST_BBOX: (f64, f64, f64, f64) = (46.3, 12.8, 49.3, 19.0);
pub const RR_PARAM: &str = "rr_acc";

pub const RV_INDEX: &str = "https://api.rainviewer.com/public/weather-maps.json";
pub const RV_FARBE: &str = "2";
pub const RV_OPTS: &str = "1_1";
pub const GS_BASE: &str =
    "https://dataset.api.hub.geosphere.at/v1/grid/forecast/nwp-v1-1h-2500m";
pub const ST_BASE: &str =
    "https://dataset.api.hub.geosphere.at/v1/station/current/tawes-v1-10min";

/// TAWES 2m-Lufttemperatur.
pub const TEMP_OBS_PARAM: &str = "TL";
/// AROME 2m-Temperatur.
pub const TEMP_FCST_PARAM: &str = "t2m";
/// TAWES Windgeschwindigkeit [m/s] und -richtung [Grad].
pub const WIND_OBS_PARAMS: [&str; 2] = ["FF", "DD"];
/// AROME 10m-Windkomponenten [m/s].
pub const WIND_FCST_PARAMS: [&str; 2] = ["u10m", "v10m"];

pub const OBS_TTL: u64 = 600;
pub const STATIONEN_TTL: u64 = 86400;
/// "Oesterreich-Zone" = AROME-Bbox.
pub const AT_BBOX: (f64, f64, f64, f64) = FCST_BBOX;
/// Wien wird nie ausgeduennt.
pub const WIEN_BOX: (f64, f64, f64, f64) = (48.10, 16.18, 48.35, 16.60);

pub const OSM_KACHEL: &str = "https://tile.openstreetmap.org/{z}/{x}/{y}.png";
pub const OM_URL: &str = "https://api.open-meteo.com/v1/forecast";

/// Mindestabstand der Punkte [Bildpunkte].
pub const MIN_PX: f64 = 56.0;
/// Punkte je Antwort.
pub const MAX_PTS: usize = 50;
/// Open-Meteo stuendlich neu.
pub const OM_TTL: u64 = 3600;
pub const OM_BATCH: usize = 50;

pub const OVERLAY_ALPHA: u8 = 170;
/// mm/h -> RGB
pub const RATE_FARBEN: [(f64, (u8, u8, u8)); 8] = [
    (0.1, (110, 170, 255)),
    (0.5, (60, 120, 245)),
    (1.0, (30, 80, 220)),
    (2.0, (0, 160, 60)),
    (4.0, (255, 210, 0)),
    (8.0, (255, 120, 0)),
    (16.0, (230, 0, 0)),
    (32.0, (170, 0, 170)),
];

pub fn ausgabeverzeichnis() -> PathBuf {
    let heim = std::env::var("HOME").unwrap_or_else(|_| "/home/user".into());
    PathBuf::from(heim).join("MyDocs/regenkarte")
}

pub fn cacheverzeichnis() -> PathBuf {
    ausgabeverzeichnis().join("cache")
}

/// Wo `cities.json` liegt: neben dem Programm, sonst am alten Platz.
pub fn staedtedatei() -> PathBuf {
    if let Ok(eigen) = std::env::current_exe() {
        if let Some(verzeichnis) = eigen.parent() {
            let neben = verzeichnis.join("cities.json");
            if neben.exists() {
                return neben;
            }
        }
    }
    PathBuf::from("/opt/regenkarte/cities.json")
}

/// Farbe fuer eine Regenrate, oder nichts unterhalb der ersten Schwelle.
pub fn rate_farbe(mmh: f64) -> Option<(u8, u8, u8)> {
    if mmh < RATE_FARBEN[0].0 {
        return None;
    }
    let mut farbe = RATE_FARBEN[0].1;
    for (schwelle, c) in RATE_FARBEN {
        if mmh >= schwelle {
            farbe = c;
        }
    }
    Some(farbe)
}

pub fn rate_hex(mmh: f64) -> String {
    match rate_farbe(mmh) {
        Some((r, g, b)) => format!("#{:02x}{:02x}{:02x}", r, g, b),
        None => String::new(),
    }
}

pub fn im_kasten(lat: f64, lon: f64, kasten: (f64, f64, f64, f64)) -> bool {
    kasten.0 <= lat && lat <= kasten.2 && kasten.1 <= lon && lon <= kasten.3
}

pub fn kaesten_ueberlappen(a: (f64, f64, f64, f64), b: (f64, f64, f64, f64)) -> bool {
    a.0 <= b.2 && a.2 >= b.0 && a.1 <= b.3 && a.3 >= b.1
}

#[cfg(test)]
mod proben {
    use super::*;

    #[test]
    fn farbskala_faengt_erst_ab_einem_zehntel_an() {
        assert_eq!(rate_farbe(0.05), None);
        assert_eq!(rate_farbe(0.1), Some((110, 170, 255)));
        assert_eq!(rate_farbe(3.0), Some((0, 160, 60)));
        assert_eq!(rate_farbe(999.0), Some((170, 0, 170)));
    }

    #[test]
    fn hex_ist_leer_wenn_es_nicht_regnet() {
        assert_eq!(rate_hex(0.0), "");
        assert_eq!(rate_hex(0.5), "#3c78f5");
    }
}
