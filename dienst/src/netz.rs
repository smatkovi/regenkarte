//! Holen, was von draussen kommt.
//!
//! Warum ein eigener TLS-Stapel statt der Systembibliotheken: Harmattans
//! OpenSSL ist von 2011 und kommt an keinen der hier benutzten Dienste
//! mehr heran. Die Python-Fassung loeste das, indem sie unter einem
//! nachinstallierten Python 3.11 lief; hier bringt rustls alles mit, und
//! das Paket haengt an gar nichts mehr.

use std::io::Read;
use std::time::Duration;

const KENNUNG: &str = "regenkarte-n950/2.0 (personal use)";
const FRIST: Duration = Duration::from_secs(45);
const VERSUCHE: u32 = 3;

/// Holt eine Datei; bei Fehlern bis zu dreimal, wie in der Vorlage.
pub fn holen(url: &str) -> Result<Vec<u8>, String> {
    let mut letzter = String::new();
    for _ in 0..VERSUCHE {
        let klient = ureq::builder()
            .timeout(FRIST)
            .user_agent(KENNUNG)
            .build();
        match klient.get(url).call() {
            Ok(antwort) => {
                let mut inhalt = Vec::new();
                match antwort.into_reader().read_to_end(&mut inhalt) {
                    Ok(_) => return Ok(inhalt),
                    Err(e) => letzter = e.to_string(),
                }
            }
            Err(e) => letzter = e.to_string(),
        }
        std::thread::sleep(Duration::from_millis(1500));
    }
    Err(format!("holen gescheitert: {} ({})", url, letzter))
}

pub fn holen_json(url: &str) -> Result<serde_json::Value, String> {
    let rohdaten = holen(url)?;
    serde_json::from_slice(&rohdaten).map_err(|e| format!("{}: {}", url, e))
}
