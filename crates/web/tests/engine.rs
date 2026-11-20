//! The engine, run the same way on the desktop and in WebAssembly: open,
//! plan, export, reopen. Each test also prints a one-line summary, so the
//! two can be compared (`cargo test --test engine -- --nocapture`, with
//! and without `--target wasm32-unknown-unknown`).

use zigzag::document::Document;
use zigzag::formats::write_format;

#[cfg(target_arch = "wasm32")]
use wasm_bindgen_test::wasm_bindgen_test;

fn say(line: &str) {
    #[cfg(target_arch = "wasm32")]
    wasm_bindgen_test::console_log!("{line}");
    #[cfg(not(target_arch = "wasm32"))]
    println!("{line}");
}

/// FNV-1a, to tell whether two builds wrote the same bytes.
fn hash(bytes: &[u8]) -> u64 {
    bytes.iter().fold(0xcbf29ce484222325, |h, b| {
        (h ^ *b as u64).wrapping_mul(0x100000001b3)
    })
}

/// Open, plan, write every format, read the PES back; says what came out.
fn round_trip(file_name: &str, bytes: &[u8]) {
    let doc = Document::open_bytes(file_name, bytes, None).expect("opens");
    let started = web_time::Instant::now();
    let design = doc.plan();
    // (Worth reading from a `--release` run only.)
    say(&format!(
        "TIME {file_name}: planned in {:.1} ms",
        started.elapsed().as_secs_f64() * 1000.0
    ));
    let stitches = design.stitch_count();
    assert!(stitches > 0, "{file_name}: nothing to sew");
    let b = design.bounds();
    let mut line = format!(
        "SUMMARY {file_name}: {stitches} stitches, {} blocks, {:.2} x {:.2} (0.1 mm);",
        design.blocks.len(),
        b.max.x - b.min.x,
        b.max.y - b.min.y,
    );
    for ext in ["pes", "dst", "exp", "jef", "vp3"] {
        let out = write_format(&design, ext).unwrap_or_else(|e| panic!("{file_name} → {ext}: {e:#}"));
        assert!(!out.is_empty());
        line += &format!(" {ext} {} bytes {:016x}", out.len(), hash(&out));
        if ext == "pes" {
            assert!(out.starts_with(b"#PES"));
            let back = Document::open_bytes("back.pes", &out, None).expect("its PES reopens");
            let again = back.plan().stitch_count();
            assert!(again > 0, "{file_name}: the PES read back empty");
            line += &format!(" (reads back as {again})");
        }
    }
    say(&line);
}

#[cfg_attr(target_arch = "wasm32", wasm_bindgen_test)]
#[cfg_attr(not(target_arch = "wasm32"), test)]
fn badge() {
    round_trip("badge.svg", include_bytes!("../../../examples/badge.svg"));
}

#[cfg_attr(target_arch = "wasm32", wasm_bindgen_test)]
#[cfg_attr(not(target_arch = "wasm32"), test)]
fn groups() {
    round_trip("groups.svg", include_bytes!("../../../examples/groups.svg"));
}

#[cfg_attr(target_arch = "wasm32", wasm_bindgen_test)]
#[cfg_attr(not(target_arch = "wasm32"), test)]
fn lettering() {
    round_trip("lettering.svg", include_bytes!("../../../examples/lettering.svg"));
}

#[cfg_attr(target_arch = "wasm32", wasm_bindgen_test)]
#[cfg_attr(not(target_arch = "wasm32"), test)]
fn bench_grid() {
    round_trip("bench-grid.svg", include_bytes!("../../../examples/bench-grid.svg"));
}

/// An image is traced into shapes (decoding, tracing, then the rest).
#[cfg_attr(target_arch = "wasm32", wasm_bindgen_test)]
#[cfg_attr(not(target_arch = "wasm32"), test)]
fn traces_an_image() {
    round_trip("dot.png", include_bytes!("fixtures/dot.png"));
}

/// What isn't a file ZigZag opens is refused, by name.
#[cfg_attr(target_arch = "wasm32", wasm_bindgen_test)]
#[cfg_attr(not(target_arch = "wasm32"), test)]
fn refuses_what_it_cannot_open() {
    let e = Document::open_bytes("design.vp3", &[0xff, 0xfe, 0x00, 0x80], None)
        .err()
        .expect("not text");
    assert!(format!("{e:#}").contains("design.vp3"), "{e:#}");
}

/// Lettering with a font handed over as bytes: in the browser there are
/// no system fonts, so this is the only way text gets set.
#[cfg_attr(target_arch = "wasm32", wasm_bindgen_test)]
#[cfg_attr(not(target_arch = "wasm32"), test)]
fn letters_with_a_font_given_as_bytes() {
    use zigzag::document::{TextSpec, font_families};
    // The font ZZZ comes with (assets/fonts, SIL Open Font License).
    let data = include_bytes!("../assets/fonts/Inter-Regular.ttf").to_vec();
    let generation = zigzag::svg::fonts_generation();
    assert_eq!(zigzag::svg::add_fonts([data]), 1);
    assert!(zigzag::svg::fonts_generation() > generation);
    let families = font_families();
    let family = families
        .iter()
        .find(|f| f.as_str() == "Inter")
        .unwrap_or_else(|| panic!("the font's family is listed: {families:?}"))
        .clone();
    let mut doc = Document::empty();
    doc.add_lettering(
        &TextSpec {
            text: "ZZZ".into(),
            font: family.clone(),
            bold: false,
            ..TextSpec::default()
        },
        (0.0, 0.0),
    )
    .expect("sets the text");
    let design = doc.plan();
    assert!(design.stitch_count() > 0, "the letters have stitches");
    say(&format!(
        "SUMMARY lettering in {family}: {} stitches",
        design.stitch_count()
    ));
}

/// A PES from somewhere else, when one is at hand: put it at
/// `tests/fixtures/local/sample1.pes` (not kept in the repository) and
/// build with `RUSTFLAGS="--cfg zzz_local"`.
#[cfg(zzz_local)]
#[cfg_attr(target_arch = "wasm32", wasm_bindgen_test)]
#[cfg_attr(not(target_arch = "wasm32"), test)]
fn a_pes_from_elsewhere() {
    round_trip("sample1.pes", include_bytes!("fixtures/local/sample1.pes"));
}

/// Every font in the library ZZZ comes with (assets/fonts/library) sets
/// text that turns into stitches, in satin, as lettering starts in.
#[cfg_attr(target_arch = "wasm32", wasm_bindgen_test)]
#[cfg_attr(not(target_arch = "wasm32"), test)]
fn every_font_in_the_library_embroiders() {
    use zigzag::document::{TextSpec, font_families};
    macro_rules! library {
        ($($file:literal),* $(,)?) => {
            [$( ($file, include_bytes!(concat!("../assets/fonts/library/", $file)).as_slice()) ),*]
        };
    }
    let fonts = library![
        "abrilfatface/AbrilFatface-Regular.ttf",
        "alfaslabone/AlfaSlabOne-Regular.ttf",
        "anton/Anton-Regular.ttf",
        "archivoblack/ArchivoBlack-Regular.ttf",
        "bebasneue/BebasNeue-Regular.ttf",
        "blackopsone/BlackOpsOne-Regular.ttf",
        "bungee/Bungee-Regular.ttf",
        "caveat/Caveat-Variable.ttf",
        "cinzel/Cinzel-Variable.ttf",
        "courgette/Courgette-Regular.ttf",
        "dancingscript/DancingScript-Variable.ttf",
        "fredoka/Fredoka-Variable.ttf",
        "graduate/Graduate-Regular.ttf",
        "greatvibes/GreatVibes-Regular.ttf",
        "kaushanscript/KaushanScript-Regular.ttf",
        "lobster/Lobster-Regular.ttf",
        "lora/Lora-Variable.ttf",
        "montserrat/Montserrat-Variable.ttf",
        "nunito/Nunito-Variable.ttf",
        "oswald/Oswald-Variable.ttf",
        "pacifico/Pacifico-Regular.ttf",
        "permanentmarker/PermanentMarker-Regular.ttf",
        "playfairdisplay/PlayfairDisplay-Variable.ttf",
        "poppins/Poppins-Bold.ttf",
        "poppins/Poppins-Regular.ttf",
        "righteous/Righteous-Regular.ttf",
        "satisfy/Satisfy-Regular.ttf",
        "staatliches/Staatliches-Regular.ttf",
    ];
    // (The index the page reads lists the same files.)
    let index = include_str!("../assets/fonts/library/library.json");
    for (file, _) in &fonts {
        assert!(index.contains(file), "{file} is in library.json");
    }
    assert_eq!(index.matches("\"file\"").count(), fonts.len(), "and nothing else");

    // The library's families, read from its files (not by comparing the
    // list before and after: other tests add fonts at the same time).
    let mut own = usvg::fontdb::Database::new();
    for (_, bytes) in &fonts {
        own.load_font_data(bytes.to_vec());
    }
    let mut added: Vec<String> = own
        .faces()
        .filter_map(|f| f.families.first().map(|(n, _)| n.clone()))
        .collect();
    added.sort();
    added.dedup();
    zigzag::svg::add_fonts(fonts.iter().map(|(_, bytes)| bytes.to_vec()));
    let listed = font_families();
    for family in &added {
        assert!(listed.contains(family), "{family} is offered for lettering");
    }
    let mut thin = Vec::new();
    for family in &added {
        let mut doc = Document::empty();
        doc.add_lettering(
            &TextSpec {
                text: "Abc".into(),
                font: family.clone(),
                size_mm: 12.0,
                ..TextSpec::default()
            },
            (0.0, 0.0),
        )
        .unwrap_or_else(|e| panic!("{family}: {e:#}"));
        let n = doc.plan().stitch_count();
        assert!(n > 0, "{family} set no stitches");
        say(&format!("FONT {family}: {n} stitches"));
        if n < 100 {
            thin.push(family.clone());
        }
    }
    // Every file brought its family (Poppins is one family in two files).
    assert_eq!(added.len(), fonts.len() - 1, "families: {added:?}");
    say(&format!("FONTS {} families; few stitches: {thin:?}", added.len()));
}
