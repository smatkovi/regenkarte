//! Kartendienst -- der Kachel- und Prognoseteil der Regenkarte.
//!
//! Nachbau von `regenkarte.py serve` in Rust. Die Oberflaeche bleibt, wie
//! sie ist (PySide/QML); hier wird nur das ausgetauscht, was den ganzen
//! Tag im Speicher liegt: der Python-Dienst belegte 10,3 MB, dieser rund
//! 1 MB.
//!
//!     kartendienst serve [port]     Dienst auf 127.0.0.1:8642
//!     kartendienst probe            laeuft schon einer? (fuer start.sh)
//!     kartendienst seed [lat0 lon0 lat1 lon1 zmin zmax]
//!                                   Grundkarte vorladen
//!
//! Nebenbei faellt die Abhaengigkeit von `/opt/wunderw/bin/python3.11`
//! weg: gebraucht wurde das nachinstallierte Python nur, weil Harmattans
//! eigenes kein TLS 1.2 kann. rustls bringt das mit.

mod arome;
mod bild;
mod dienst;
mod konfig;
mod netz;
mod orte;
mod zeit;

use konfig::*;

fn kachel_x(lon: f64, z: u32) -> i64 {
    let n = (1i64 << z) as f64;
    (((lon + 180.0) / 360.0 * n) as i64).clamp(0, (1i64 << z) - 1)
}

fn kachel_y(lat: f64, z: u32) -> i64 {
    let lat = lat.clamp(-85.05, 85.05);
    let n = (1i64 << z) as f64;
    let y = (1.0 - lat.to_radians().tan().asinh() / std::f64::consts::PI) / 2.0;
    ((y * n) as i64).clamp(0, (1i64 << z) - 1)
}

/// lat0, lon0, lat1, lon1, zmin, zmax
const SAEPLAN: [(f64, f64, f64, f64, u32, u32); 3] = [
    (-85.0, -180.0, 85.0, 180.0, 3, 5),
    (34.0, -12.0, 62.0, 30.0, 6, 6),
    (45.0, 11.2, 50.8, 19.7, 7, 9),
];

/// Die Grundkarte vorladen. Einmal am WLAN, dann ist sie unterwegs da --
/// OSM-Kacheln aendern sich kaum, und der Zwischenspeicher wird nie
/// aufgeraeumt.
fn saeen(plan: &[(f64, f64, f64, f64, u32, u32)]) {
    let _ = std::fs::create_dir_all(cacheverzeichnis());
    let mut auftraege = Vec::new();
    for (lat0, lon0, lat1, lon1, zmin, zmax) in plan {
        for z in *zmin..=*zmax {
            let (x0, x1) = (kachel_x(*lon0, z), kachel_x(*lon1, z));
            let (y0, y1) = (kachel_y(*lat1, z), kachel_y(*lat0, z));
            for y in y0..=y1 {
                for x in x0..=x1 {
                    auftraege.push((z, x, y));
                }
            }
        }
    }
    auftraege.sort();
    auftraege.dedup();
    let offen: Vec<(u32, i64, i64)> = auftraege
        .iter()
        .copied()
        .filter(|(z, x, y)| !cacheverzeichnis().join(format!("osm_{}_{}_{}.png", z, x, y)).exists())
        .collect();
    println!(
        "saeen: {} Kacheln gesamt, {} fehlen",
        auftraege.len(),
        offen.len()
    );
    let (mut gut, mut schlecht) = (0, 0);
    for (i, (z, x, y)) in offen.iter().enumerate() {
        let url = OSM_KACHEL
            .replace("{z}", &z.to_string())
            .replace("{x}", &x.to_string())
            .replace("{y}", &y.to_string());
        match netz::holen(&url) {
            Ok(daten) => {
                let ziel = cacheverzeichnis().join(format!("osm_{}_{}_{}.png", z, x, y));
                if std::fs::write(ziel, daten).is_ok() {
                    gut += 1;
                } else {
                    schlecht += 1;
                }
            }
            Err(e) => {
                schlecht += 1;
                eprintln!("  z{}/{}/{}: {}", z, x, y, e);
            }
        }
        if (i + 1) % 25 == 0 || i + 1 == offen.len() {
            println!("  {}/{}", i + 1, offen.len());
        }
        // Die Kachelserver von OSM sind eine Spende; nicht draufhauen.
        std::thread::sleep(std::time::Duration::from_millis(250));
    }
    println!("saeen fertig: {} geladen, {} Fehler", gut, schlecht);
}

fn main() {
    // Ortszeit fuer die Beschriftung; die Vorlage setzte dieselbe Zone.
    // musl liest TZ bei jedem localtime_r neu; ein tzset() gibt es dort
    // gar nicht, und gebraucht wird es damit auch nicht.
    if std::env::var_os("TZ").is_none() {
        std::env::set_var("TZ", "Europe/Vienna");
    }

    let argumente: Vec<String> = std::env::args().skip(1).collect();
    match argumente.first().map(|s| s.as_str()) {
        Some("seed") => {
            if argumente.len() >= 7 {
                let z: Vec<f64> = argumente[1..7]
                    .iter()
                    .filter_map(|a| a.parse::<f64>().ok())
                    .collect();
                if z.len() == 6 {
                    saeen(&[(z[0], z[1], z[2], z[3], z[4] as u32, z[5] as u32)]);
                    return;
                }
            }
            saeen(&SAEPLAN);
        }
        Some("probe") => {
            // Fuer start.sh: laeuft schon einer? Frueher fragte das ein
            // Python-Einzeiler -- der ist mit dem Dienst weggefallen.
            let ziel = std::net::SocketAddr::from(([127, 0, 0, 1], PORT));
            let steht = std::net::TcpStream::connect_timeout(
                &ziel,
                std::time::Duration::from_millis(300),
            )
            .is_ok();
            std::process::exit(if steht { 0 } else { 1 });
        }
        Some("serve") | None => {
            let port = argumente
                .get(1)
                .and_then(|p| p.parse::<u16>().ok())
                .unwrap_or(PORT);
            if let Err(e) = dienst::bedienen_auf(port) {
                eprintln!("Dienst: {}", e);
                std::process::exit(1);
            }
        }
        Some(anderes) => {
            eprintln!("unbekannt: {}", anderes);
            eprintln!(
                "Aufruf: kartendienst [serve [port] | seed [lat0 lon0 lat1 lon1 zmin zmax] | probe]"
            );
            std::process::exit(1);
        }
    }
}

#[cfg(test)]
mod proben {
    use super::*;

    #[test]
    fn kachelrechnung_stimmt_mit_der_vorlage() {
        // Wien bei z=7, nachgerechnet wie in der Python-Fassung:
        // (16.3738 + 180) / 360 * 128 = 69.8 -> 69.
        assert_eq!(kachel_x(16.3738, 7), 69);
        assert_eq!(kachel_y(48.2082, 7), 44);
        // Raender werden nicht ueberschritten.
        assert_eq!(kachel_x(180.0, 3), 7);
        assert_eq!(kachel_y(-90.0, 3), 7);
    }
}
