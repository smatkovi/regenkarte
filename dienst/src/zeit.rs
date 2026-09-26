//! Zeitrechnung ohne Zeitkiste.
//!
//! Gebraucht werden drei Dinge: Ortszeit fuer die Beschriftung der Bilder,
//! UTC fuer die Abfragen an GeoSphere, und das Zurueckrechnen eines
//! ISO-Zeitstempels auf Sekunden. Dafuer genuegen `localtime_r`/`gmtime_r`
//! aus der libc und ein eigenes `timegm` -- eine Zeitzonenkiste waere
//! mehr Abhaengigkeit als Nutzen.

#![allow(deprecated)]

use std::time::{SystemTime, UNIX_EPOCH};

pub const H: i64 = 3600;

pub fn jetzt() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

fn zerlegen(sekunden: i64, ortszeit: bool) -> libc::tm {
    let mut t = sekunden as libc::time_t;
    let mut teile: libc::tm = unsafe { std::mem::zeroed() };
    unsafe {
        if ortszeit {
            libc::localtime_r(&mut t, &mut teile);
        } else {
            libc::gmtime_r(&mut t, &mut teile);
        }
    }
    teile
}

const WOCHENTAGE: [&str; 7] = ["So", "Mo", "Di", "Mi", "Do", "Fr", "Sa"];

/// "Mi 14:00" in Ortszeit -- so beschriftet die Oberflaeche ihre Bilder.
pub fn beschriftung(sekunden: i64) -> String {
    let t = zerlegen(sekunden, true);
    format!(
        "{} {:02}:{:02}",
        WOCHENTAGE[(t.tm_wday as usize) % 7],
        t.tm_hour,
        t.tm_min
    )
}

/// "2026-09-26T14:00" in UTC -- das Format, das GeoSphere erwartet.
pub fn utc_minuten(sekunden: i64) -> String {
    let t = zerlegen(sekunden, false);
    format!(
        "{:04}-{:02}-{:02}T{:02}:{:02}",
        t.tm_year + 1900,
        t.tm_mon + 1,
        t.tm_mday,
        t.tm_hour,
        t.tm_min
    )
}

/// Tage seit 1970-01-01 aus einem buergerlichen Datum (Howard Hinnant).
fn tage_seit_epoche(jahr: i64, monat: i64, tag: i64) -> i64 {
    let j = jahr - if monat <= 2 { 1 } else { 0 };
    let aera = if j >= 0 { j } else { j - 399 } / 400;
    let jahr_in_aera = j - aera * 400;
    let m = if monat > 2 { monat - 3 } else { monat + 9 };
    let tag_im_jahr = (153 * m + 2) / 5 + tag - 1;
    let tag_in_aera = jahr_in_aera * 365 + jahr_in_aera / 4 - jahr_in_aera / 100 + tag_im_jahr;
    aera * 146097 + tag_in_aera - 719468
}

/// Einen ISO-Zeitstempel auf Sekunden bringen. Ohne Zonenangabe gilt UTC --
/// genauso wie in der Python-Fassung, und GeoSphere liefert auch UTC.
pub fn stempel_zu_sekunden(text: &str) -> Option<i64> {
    let zahlen: Vec<i64> = text
        .split(|c: char| !c.is_ascii_digit())
        .filter(|s| !s.is_empty())
        .filter_map(|s| s.parse::<i64>().ok())
        .collect();
    if zahlen.len() < 5 {
        return None;
    }
    let (jahr, monat, tag, stunde, minute) =
        (zahlen[0], zahlen[1], zahlen[2], zahlen[3], zahlen[4]);
    let sekunde = *zahlen.get(5).unwrap_or(&0);
    let mut s = tage_seit_epoche(jahr, monat, tag) * 86400 + stunde * 3600 + minute * 60 + sekunde;
    // Ein angehaengtes "+02:00" oder "-05:00" wieder herausrechnen.
    if let Some(stelle) = text.rfind(['+', '-']) {
        if stelle > 10 {
            let zone = &text[stelle..];
            let vorzeichen = if zone.starts_with('-') { 1 } else { -1 };
            let felder: Vec<i64> = zone[1..]
                .split(':')
                .filter_map(|f| f.parse::<i64>().ok())
                .collect();
            if !felder.is_empty() {
                let versatz = felder[0] * 3600 + felder.get(1).unwrap_or(&0) * 60;
                s += vorzeichen * versatz;
            }
        }
    }
    Some(s)
}

#[cfg(test)]
mod proben {
    use super::*;

    #[test]
    fn stempel_ohne_zone_ist_utc() {
        assert_eq!(stempel_zu_sekunden("2026-09-26T14:00"), Some(1790431200));
        assert_eq!(
            stempel_zu_sekunden("2026-09-26T14:00:00+00:00"),
            Some(1790431200)
        );
    }

    #[test]
    fn stempel_mit_zone_wird_zurueckgerechnet() {
        // 16:00 in +02:00 ist 14:00 UTC.
        assert_eq!(
            stempel_zu_sekunden("2026-09-26T16:00:00+02:00"),
            Some(1790431200)
        );
    }

    #[test]
    fn epoche_selbst() {
        assert_eq!(stempel_zu_sekunden("1970-01-01T00:00"), Some(0));
    }
}
