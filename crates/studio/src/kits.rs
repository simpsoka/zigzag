//! Thread kits: the sets of spools people actually buy. Making a palette
//! from one (the palette menu's New Palette from Thread Kit) gives you
//! exactly the threads in your box, and Match All then keeps a design to
//! the threads you have.
//!
//! Each kit is a list of catalog numbers of threads we have real colors for
//! (Brother's own thread list; the Janome chart), so a kit's swatches are
//! those threads, not guesses. Only kits whose contents the maker publishes
//! are listed.

use crate::threads::{Brand, brother_thread, chart};

pub struct Kit {
    pub brand: Brand,
    /// Shown in the menu, and the new palette's name.
    pub name: &'static str,
    /// A line on what's in it, for the menu's tooltip.
    pub about: &'static str,
    /// Catalog numbers, in the maker's order.
    pub numbers: &'static [&'static str],
}

/// Brother Embroidery thread sets, from Brother's own thread list for
/// Artspira ("Thread Color List (BROTHER EMBROIDERY)", which marks each
/// thread in the 40, 22 and 12 color sets).
const BROTHER_40: [&str; 40] = [
    "001", "005", "007", "010", "017", "019", "027", "030", "058", "070", "079", "085", "086",
    "107", "124", "205", "208", "209", "214", "307", "323", "328", "339", "348", "405", "420",
    "502", "507", "513", "515", "534", "612", "614", "620", "704", "707", "800", "810", "843",
    "900",
];

const BROTHER_22: [&str; 22] = [
    "126", "202", "206", "330", "333", "337", "348", "399", "406", "415", "509", "517", "519",
    "542", "607", "613", "804", "807", "808", "812", "817", "869",
];

const BROTHER_12: [&str; 12] = [
    "001", "017", "085", "124", "208", "214", "339", "405", "507", "612", "800", "900",
];

/// Both Brother sets together: every Brother Embroidery color (the 22 set
/// shares Khaki with the 40).
const BROTHER_ALL: [&str; 61] = [
    "001", "005", "007", "010", "017", "019", "027", "030", "058", "070", "079", "085", "086",
    "107", "124", "205", "208", "209", "214", "307", "323", "328", "339", "348", "405", "420",
    "502", "507", "513", "515", "534", "612", "614", "620", "704", "707", "800", "810", "843",
    "900", "126", "202", "206", "330", "333", "337", "399", "406", "415", "509", "517", "519",
    "542", "607", "613", "804", "807", "808", "812", "817", "869",
];

pub const KITS: &[Kit] = &[
    Kit {
        brand: Brand::Brother,
        name: "Brother 12 Colors",
        about: "Brother Embroidery thread, 12-spool starter set",
        numbers: &BROTHER_12,
    },
    Kit {
        brand: Brand::Brother,
        name: "Brother 40 Colors",
        about: "Brother Embroidery thread, 40-spool set",
        numbers: &BROTHER_40,
    },
    Kit {
        brand: Brand::Brother,
        name: "Brother 22 Colors",
        about: "Brother Embroidery thread, 22-spool set (adds to the 40)",
        numbers: &BROTHER_22,
    },
    Kit {
        brand: Brand::Brother,
        name: "Brother 40 + 22 Colors",
        about: "Both Brother sets: all 61 Brother Embroidery colors",
        numbers: &BROTHER_ALL,
    },
];

impl Kit {
    /// The kit's threads as colors, in its order.
    pub fn colors(&self) -> Vec<[u8; 3]> {
        self.numbers
            .iter()
            .filter_map(|n| thread(self.brand, n))
            .collect()
    }
}

/// A thread's color by catalog number.
fn thread(brand: Brand, number: &str) -> Option<[u8; 3]> {
    match brand {
        Brand::Brother => brother_thread(number).map(|t| t.0),
        Brand::Janome => chart(Brand::Janome)
            .into_iter()
            .find(|t| t.2 == Some(number))
            .map(|t| t.0),
    }
}

/// The brands that have kits, in menu order.
pub fn brands() -> Vec<Brand> {
    let mut out: Vec<Brand> = Vec::new();
    for k in KITS {
        if !out.contains(&k.brand) {
            out.push(k.brand);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_kit_thread_is_in_its_chart() {
        for kit in KITS {
            let colors = kit.colors();
            assert_eq!(
                colors.len(),
                kit.numbers.len(),
                "{}: a number isn't in the {:?} chart",
                kit.name,
                kit.brand
            );
            let mut seen = colors.clone();
            seen.sort();
            seen.dedup();
            assert_eq!(seen.len(), colors.len(), "{}: a thread twice", kit.name);
        }
    }

    #[test]
    fn the_brother_sets_add_up() {
        let all: std::collections::HashSet<_> = BROTHER_ALL.iter().collect();
        let both: std::collections::HashSet<_> =
            BROTHER_40.iter().chain(BROTHER_22.iter()).collect();
        assert_eq!(all, both, "40 + 22 is every color");
        assert_eq!(all.len(), 61);
        assert!(BROTHER_12.iter().all(|n| BROTHER_40.contains(n)), "12 ⊂ 40");
        assert!(all.iter().all(|n| brother_thread(n).is_some()));
    }

    /// The Brother chart's colors carry their thread numbers, including
    /// the two that are a rounding step off Brother's own colors.
    #[test]
    fn brother_chart_colors_have_their_numbers() {
        let chart = chart(Brand::Brother);
        let number = |name: &str| chart.iter().find(|t| t.1 == name).and_then(|t| t.2);
        assert_eq!(number("Salmon Pink"), Some("079"));
        assert_eq!(number("Amber Red"), Some("333"));
        assert_eq!(number("Black"), Some("900"));
        // Each number once.
        let mut nums: Vec<_> = chart.iter().filter_map(|t| t.2).collect();
        let n = nums.len();
        nums.sort();
        nums.dedup();
        assert_eq!(nums.len(), n);
    }
}
