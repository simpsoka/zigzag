//! Thread charts: which brand's thread a colour is, for naming a design's
//! threads (the studio's Threads panel and the web's pages).

use super::jef_data::JEF_PALETTE;
use super::pec_data::PEC_PALETTE;
use super::perceptual_distance;

/// Brother Embroidery thread numbers by color: the chart Artspira uses
/// (via Respira's BrotherColor.json, Apache-2.0, Copyright Jan-Henrik
/// Bruhn). The PEC palette's colors are these threads.
pub const BROTHER_NUMBERS: [([u8; 3], &str); 61] = [
    ([240, 240, 240], "001"),
    ([239, 227, 185], "843"),
    ([255, 255, 179], "010"),
    ([227, 243, 91], "027"),
    ([168, 221, 196], "542"),
    ([168, 222, 235], "017"),
    ([178, 175, 212], "804"),
    ([253, 217, 222], "124"),
    ([252, 187, 196], "079"),
    ([216, 204, 198], "399"),
    ([254, 227, 197], "307"),
    ([168, 168, 168], "005"),
    ([255, 240, 141], "812"),
    ([240, 249, 112], "202"),
    ([158, 214, 125], "502"),
    ([102, 186, 73], "509"),
    ([37, 132, 187], "019"),
    ([145, 95, 172], "612"),
    ([228, 154, 203], "810"),
    ([249, 147, 188], "085"),
    ([246, 74, 138], "086"),
    ([208, 166, 96], "348"),
    ([135, 135, 135], "817"),
    ([254, 179, 67], "126"),
    ([255, 255, 0], "205"),
    ([112, 188, 31], "513"),
    ([0, 135, 119], "534"),
    ([9, 91, 166], "420"),
    ([104, 106, 176], "607"),
    ([145, 54, 151], "620"),
    ([247, 56, 102], "807"),
    ([209, 84, 0], "339"),
    ([186, 152, 0], "328"),
    ([79, 85, 86], "704"),
    ([254, 158, 50], "209"),
    ([255, 217, 17], "206"),
    ([47, 126, 32], "515"),
    ([0, 103, 62], "507"),
    ([10, 85, 163], "405"),
    ([75, 107, 175], "070"),
    ([119, 1, 118], "869"),
    ([199, 1, 86], "107"),
    ([254, 55, 15], "030"),
    ([125, 111, 0], "330"),
    ([41, 49, 51], "707"),
    ([232, 169, 0], "214"),
    ([254, 186, 53], "208"),
    ([67, 86, 7], "517"),
    ([19, 74, 70], "415"),
    ([11, 61, 145], "406"),
    ([78, 41, 144], "614"),
    ([106, 28, 138], "613"),
    ([181, 76, 100], "333"),
    ([237, 23, 31], "800"),
    ([209, 92, 0], "337"),
    ([0, 0, 0], "900"),
    ([42, 19, 1], "058"),
    ([178, 118, 36], "323"),
    ([19, 43, 26], "519"),
    ([0, 56, 34], "808"),
    ([14, 31, 124], "007"),
];

/// The Brother Embroidery thread number for a chart color. (A step of
/// rounding apart counts: the PEC chart has Salmon Pink and Amber Red one
/// unit off Brother's own colors for them.)
pub fn brother_number(c: [u8; 3]) -> Option<&'static str> {
    let close = |k: &[u8; 3]| (0..3).all(|i| k[i].abs_diff(c[i]) <= 1);
    BROTHER_NUMBERS
        .iter()
        .find(|(k, _)| *k == c)
        .or_else(|| BROTHER_NUMBERS.iter().find(|(k, _)| close(k)))
        .map(|(_, n)| *n)
}

/// The nearest thread in each chart, by the studio's perceptual distance:
/// Brother's (the PEC palette's name, with its Brother number) and
/// Janome's (name, code). ((name, number), (name, code))
pub fn nearest_threads(c: [u8; 3]) -> ((&'static str, Option<&'static str>), (&'static str, &'static str)) {
    let d = |r: u8, g: u8, b: u8| perceptual_distance(c, [r, g, b]);
    let &(r, g, b, n) = PEC_PALETTE
        .iter()
        .skip(1)
        .min_by(|x, y| d(x.0, x.1, x.2).total_cmp(&d(y.0, y.1, y.2)))
        .expect("a chart");
    let &(_, _, _, jn, jk) = JEF_PALETTE
        .iter()
        .skip(1)
        .min_by(|x, y| d(x.0, x.1, x.2).total_cmp(&d(y.0, y.1, y.2)))
        .expect("a chart");
    ((n, brother_number([r, g, b])), (jn, jk))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_chart_colour_is_its_own_thread() {
        let ((name, number), (jname, _)) = nearest_threads([11, 61, 145]);
        assert_eq!(number, Some("406"), "{name}");
        assert!(!jname.is_empty());
    }
}
