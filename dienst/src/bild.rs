//! Ein PNG von Hand schreiben.
//!
//! Gebraucht wird genau eine Sorte: RGBA, 8 Bit, ohne Filter. Dafuer eine
//! Bildbibliothek einzubinden waere mehr Abhaengigkeit als Nutzen -- die
//! Python-Fassung tat es aus demselben Grund mit `zlib` und `struct`.

use std::io::Write;

use flate2::write::ZlibEncoder;
use flate2::Compression;

fn abschnitt(kennung: &[u8; 4], daten: &[u8]) -> Vec<u8> {
    let mut aus = Vec::with_capacity(12 + daten.len());
    aus.extend_from_slice(&(daten.len() as u32).to_be_bytes());
    aus.extend_from_slice(kennung);
    aus.extend_from_slice(daten);
    let mut pruefsumme = crc32fast::Hasher::new();
    pruefsumme.update(kennung);
    pruefsumme.update(daten);
    aus.extend_from_slice(&pruefsumme.finalize().to_be_bytes());
    aus
}

/// `rgba` ist zeilenweise, vier Bytes je Bildpunkt, ohne Filterbyte --
/// das setzt diese Funktion davor.
pub fn png(breite: usize, hoehe: usize, rgba: &[u8]) -> Vec<u8> {
    let zeilenlaenge = 4 * breite;
    let mut roh = Vec::with_capacity(hoehe * (1 + zeilenlaenge));
    for y in 0..hoehe {
        roh.push(0); // Filter "None"
        roh.extend_from_slice(&rgba[y * zeilenlaenge..(y + 1) * zeilenlaenge]);
    }
    let mut packer = ZlibEncoder::new(Vec::new(), Compression::new(6));
    let _ = packer.write_all(&roh);
    let gepackt = packer.finish().unwrap_or_default();

    let mut kopf = Vec::with_capacity(13);
    kopf.extend_from_slice(&(breite as u32).to_be_bytes());
    kopf.extend_from_slice(&(hoehe as u32).to_be_bytes());
    kopf.extend_from_slice(&[8, 6, 0, 0, 0]); // 8 Bit, RGBA, ohne Zwischenzeilen

    let mut aus = Vec::new();
    aus.extend_from_slice(&[0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a]);
    aus.extend_from_slice(&abschnitt(b"IHDR", &kopf));
    aus.extend_from_slice(&abschnitt(b"IDAT", &gepackt));
    aus.extend_from_slice(&abschnitt(b"IEND", b""));
    aus
}

/// Ein durchsichtiger Bildpunkt -- die Antwort, wenn es nichts zu zeigen gibt.
pub fn leeres_png() -> Vec<u8> {
    png(1, 1, &[0, 0, 0, 0])
}

#[cfg(test)]
mod proben {
    use super::*;

    #[test]
    fn kopf_und_ende_stimmen() {
        let p = png(2, 2, &[0u8; 16]);
        assert_eq!(&p[..8], &[0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a]);
        assert_eq!(&p[p.len() - 8..p.len() - 4], b"IEND");
        // IHDR-Laenge steht als erstes nach der Signatur und ist immer 13.
        assert_eq!(&p[8..12], &13u32.to_be_bytes());
    }

    #[test]
    fn leeres_ist_ein_bildpunkt() {
        let p = leeres_png();
        assert_eq!(&p[16..20], &1u32.to_be_bytes()); // Breite
        assert_eq!(&p[20..24], &1u32.to_be_bytes()); // Hoehe
    }
}
