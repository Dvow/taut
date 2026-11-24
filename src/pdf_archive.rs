//! PDF/A-2b: a file made to be kept, which shows the same in years to come. Everything it needs
//! is inside it (fonts already are), its colours are said to be sRGB by a colour profile made
//! here, its metadata says what it meets, and nothing in it depends on a viewer's choices
//! (pictures are not smoothed, CMYK photos go in as pixels). Only a program that asks for it
//! ([`Paper::archive`]) carries this.
//!
//! The profile is the ICC's version 2 layout of sRGB: the D50 colorants and white of IEC
//! 61966-2-1, and its tone curve as a table of 1,024 points.
use crate::paper::Paper;

impl Paper {
    /// Writes the file as PDF/A-2b, for keeping: its colours stated as sRGB, its metadata saying
    /// so. (Encryption, which PDF/A forbids, is left out.)
    pub fn archive(&mut self) {
        self.archive = Some(profile);
        self.lock = None;
    }
}

/// A number as ICC's s15Fixed16: sixteen bits of fraction.
fn fixed(number: f32) -> [u8; 4] {
    ((number * 65536.0).round() as i32).to_be_bytes()
}

/// An `XYZ ` tag of one value.
fn xyz(value: [f32; 3]) -> Vec<u8> {
    [&b"XYZ \0\0\0\0"[..], &fixed(value[0]), &fixed(value[1]), &fixed(value[2])].concat()
}

/// The sRGB profile, version 2: header, tag list, then the tags, each four-byte aligned. The
/// three tone curves share one tag.
pub(crate) fn profile() -> Vec<u8> {
    let name = b"sRGB IEC61966-2.1";
    // A `desc`: the name in ASCII with its terminator, then empty Unicode and ScriptCode names.
    let described = [&b"desc\0\0\0\0"[..], &(name.len() as u32 + 1).to_be_bytes(), name, &[0], &[0; 8], &[0; 3], &[0; 67]].concat();
    let copied = [&b"text\0\0\0\0"[..], b"No copyright, use freely", &[0]].concat();
    let points: Vec<u8> = (0..1024)
        .flat_map(|at| {
            let value = at as f32 / 1023.0;
            let light = if value <= 0.040_45 { value / 12.92 } else { ((value + 0.055) / 1.055).powf(2.4) };
            ((light * 65535.0).round() as u16).to_be_bytes()
        })
        .collect();
    let curve = [&b"curv\0\0\0\0"[..], &1024u32.to_be_bytes(), &points].concat();
    let tags: [(&[u8; 4], Vec<u8>); 7] = [
        (b"desc", described),
        (b"cprt", copied),
        (b"wtpt", xyz([0.9642, 1.0, 0.8249])),
        (b"rXYZ", xyz([0.436_074_7, 0.222_504_5, 0.013_932_2])),
        (b"gXYZ", xyz([0.385_064_9, 0.716_878_6, 0.097_104_5])),
        (b"bXYZ", xyz([0.143_080_4, 0.060_616_9, 0.714_173_3])),
        (b"rTRC", curve),
    ];
    let listed = 4 + 12 * (tags.len() + 2);
    let (mut list, mut body) = (((tags.len() + 2) as u32).to_be_bytes().to_vec(), Vec::new());
    for (name, holds) in &tags {
        list.extend([&name[..], &((128 + listed + body.len()) as u32).to_be_bytes(), &(holds.len() as u32).to_be_bytes()].concat());
        body.extend_from_slice(holds);
        body.resize(body.len().next_multiple_of(4), 0);
    }
    // (Green's and blue's curves are red's.)
    let red = list[list.len() - 8..].to_vec();
    list.extend([b"gTRC".as_slice(), &red].concat());
    list.extend([b"bTRC".as_slice(), &red].concat());
    let mut head = vec![0u8; 128];
    head[8..12].copy_from_slice(&[2, 0x10, 0, 0]);
    head[12..24].copy_from_slice(b"mntrRGB XYZ ");
    head[24..36].copy_from_slice(&[0x07, 0xea, 0, 1, 0, 1, 0, 0, 0, 0, 0, 0]);
    head[36..40].copy_from_slice(b"acsp");
    head[68..80].copy_from_slice(&[fixed(0.9642), fixed(1.0), fixed(0.8249)].concat());
    let mut all = [head, list, body].concat();
    let long = (all.len() as u32).to_be_bytes();
    all[..4].copy_from_slice(&long);
    all
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_profile_is_srgb_as_the_icc_lays_it_out() {
        let made = profile();
        assert_eq!(u32::from_be_bytes(made[..4].try_into().unwrap()) as usize, made.len(), "it says its own length");
        assert_eq!((&made[12..20], &made[36..40]), (&b"mntrRGB "[..], &b"acsp"[..]));
        // Read back as taut reads profiles: colours that are the screen's change nothing.
        #[cfg(feature = "color-profiles")]
        assert!(crate::color_profiles::Change::of_profile(&made).is_none(), "sRGB");
        let count = u32::from_be_bytes(made[128..132].try_into().unwrap()) as usize;
        let names: Vec<&[u8]> = (0..count).map(|n| &made[132 + 12 * n..136 + 12 * n]).collect();
        for name in [b"desc", b"cprt", b"wtpt", b"rXYZ", b"gXYZ", b"bXYZ", b"rTRC", b"gTRC", b"bTRC"] {
            assert!(names.contains(&&name[..]), "{}", String::from_utf8_lossy(name));
        }
    }
}
