//! The repository ships every release wallpaper under `assets/wallpapers`.

#[test]
fn every_codename_has_a_committed_png() {
    let folder = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/wallpapers");
    let missing: Vec<_> = tehuti::wallpaper::CODENAMES
        .iter()
        .filter(|name| !folder.join(format!("{name}.png")).is_file())
        .collect();
    assert!(missing.is_empty(), "missing wallpaper PNGs: {missing:?}");
}
